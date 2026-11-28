const queueStorageKey = 'mono_transcription_queue_v1';
const queueActiveStages = new Set(['decoding', 'inferring', 'cancelling']);
const queueRetryStages = new Set(['error', 'cancelled', 'interrupted']);
const queueLabels = {
  queued:'等待转录', decoding:'正在提取音轨', inferring:'正在转录',
  cancelling:'正在取消', cancelled:'已取消', interrupted:'上次转录中断',
  completed:'已完成', error:'转录失败',
};

function loadTranscriptionQueue(store) {
  const records = new Set(store.records.map(record => record.id));
  try {
    return JSON.parse(store.getItem(queueStorageKey) || '[]').flatMap(job => {
      if (records.has(job.id)) return [{...job, stage:'completed', progress:100}];
      if (job.stage === 'completed') return [];
      return [{...job, stage:queueActiveStages.has(job.stage) ? 'interrupted' : job.stage}];
    });
  } catch (error) {
    console.error('读取转录队列失败', error);
    return [];
  }
}

function useTranscriptionQueue(React, notify, onComplete) {
  const store = window.MonoStore;
  const [jobs, setJobs] = React.useState(() => loadTranscriptionQueue(store));
  const current = React.useRef(jobs);
  const worker = React.useRef(null);
  const removing = React.useRef(new Set());

  function change(next, persist = true) {
    current.current = next;
    setJobs(next);
    if (!persist) return;
    const snapshot = next.map(({draftAttempt, measured, message, ...job}) => job);
    return store.setItem(queueStorageKey, JSON.stringify(snapshot));
  }

  function patch(id, changes, persist) {
    return change(current.current.map(job => job.id === id ? {...job,...changes} : job), persist);
  }

  async function cancelRunning(handle) {
    try {
      const cancelled = await desktop.invoke('cancel_transcription', {jobId:handle.id});
      if (cancelled || worker.current !== handle) return;
      patch(handle.id, {stage:handle.stage});
    } catch (error) {
      if (worker.current !== handle) return;
      patch(handle.id, {stage:handle.stage});
      notify(String(error));
    }
  }

  React.useEffect(() => {
    const job = current.current.find(item => item.stage === 'queued');
    if (!job || worker.current) return;
    const handle = {id:job.id, stage:'decoding', ready:false};
    worker.current = handle;
    const savedJob = patch(job.id, {stage:'decoding'});
    (async () => {
      let changes;
      try {
        await savedJob;
        const record = await desktop.run(job, ({stage, ...data}) => {
          const active = current.current.find(item => item.id === handle.id);
          if (stage) handle.stage = stage;
          const nextStage = active.stage === 'cancelling' ? active.stage : handle.stage;
          patch(job.id, {...data, stage:nextStage}, false);
        }, () => {
          handle.ready = true;
          if (current.current.find(item => item.id === handle.id)?.stage === 'cancelling') {
            void cancelRunning(handle);
          }
        });
        store.rememberTranscription(record);
        onComplete(record);
        changes = {stage:'completed', progress:100, draftAttempt:undefined, error:undefined};
        if (job.options.characterJson) {
          try {
            const path = await desktop.invoke('save_character_json', {
              id:record.id, title:record.title, content:renderCharacterJson(record),
            });
            notify('逐字 JSON 已保存至 ' + path);
          } catch (error) {
            notify('转录已完成，逐字 JSON 保存失败：' + error);
          }
        } else {
          notify('转录完成：' + record.title);
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        changes = message === 'TRANSCRIPTION_CANCELLED' ? {stage:'cancelled'} : {stage:'error', error:message};
      } finally {
        worker.current = null;
        patch(job.id, changes);
      }
    })();
  }, [jobs]);

  function cancel(id) {
    const job = current.current.find(item => item.id === id);
    if (job?.stage === 'queued') {
      patch(id, {stage:'cancelled'});
    } else if (worker.current?.id === id && ['decoding', 'inferring'].includes(job?.stage)) {
      const handle = worker.current;
      patch(id, {stage:'cancelling'});
      if (handle.ready) void cancelRunning(handle);
    }
  }

  function retry(id, resume = false) {
    if (removing.current.has(id) || !current.current.some(job => job.id === id && queueRetryStages.has(job.stage))) return;
    patch(id, {stage:'queued', resume, progress:0, error:undefined, message:undefined, measured:false, draftAttempt:undefined});
  }

  async function remove(id) {
    const job = current.current.find(item => item.id === id);
    if (!job || removing.current.has(id) || job.stage === 'queued' || queueActiveStages.has(job.stage)) return;
    removing.current.add(id);
    try {
      if (queueRetryStages.has(job.stage)) {
        const draft = await desktop.invoke('draft_page', {jobId:id});
        if ((draft.total || draft.audioPath) && !confirm('移出队列会删除这条任务的部分稿和已保存音轨，确定继续？')) return;
        await desktop.invoke('draft_discard', {jobId:id});
      }
      change(current.current.filter(item => item.id !== id));
    } catch (error) {
      notify('移出队列失败：' + error);
    } finally {
      removing.current.delete(id);
    }
  }

  return {jobs, enqueue:items => change([...current.current,...items]), cancel, retry, remove};
}

const emptyLiveSegments = [];
function liveTranscriptionRecord(job) {
  return {
    id:job.id,
    title:job.fileName.replace(/\.[^.]+$/, ''),
    sourceFileName:job.fileName,
    modelName:job.modelName,
    fileSizeBytes:job.fileSizeBytes || 0,
    createdAt:job.createdAt,
    segments:emptyLiveSegments,
  };
}

const emptyDraftPage = {segments:emptyLiveSegments, start:0, total:0, savedUntil:0};

function useTranscriptionDraft(React, job) {
  const id = job?.id;
  const active = queueActiveStages.has(job?.stage);
  const attempt = active ? job.draftAttempt : undefined;
  const [requested, setRequested] = React.useState(null);
  const [snapshot, setSnapshot] = React.useState(null);
  const start = requested?.id === id ? requested?.start : undefined;
  const page = (snapshot?.id === id ? snapshot?.page : null) ?? emptyDraftPage;
  const go = (position) => setRequested(position == null ? null : {id, start:position});
  React.useEffect(() => {
    if (!id) return;
    let closed = false;
    let timer;
    let known;
    async function read() {
      try {
        const page = await desktop.invoke('draft_page', {jobId:id, start, attempt, known});
        if (closed || !page) return;
        known = [page.attempt, page.start, page.total];
        setSnapshot({id,page:{...page, audioUrl:page.audioPath ? window.__TAURI__.core.convertFileSrc(page.audioPath) : undefined}});
      } catch (error) {
        known = undefined;
        if (!closed) setSnapshot(previous => ({id, page:previous?.id === id ? previous.page : emptyDraftPage, error:String(error)}));
      } finally {
        if (active && !closed) timer = setTimeout(read, 450);
      }
    }
    read();
    return () => { closed = true; clearTimeout(timer); };
  }, [id, start, attempt, active]);
  return {page, go, tail:start == null, pin:() => { if (start == null && page.total) go(page.start); },
    error:snapshot?.id === id ? snapshot?.error : undefined};
}
