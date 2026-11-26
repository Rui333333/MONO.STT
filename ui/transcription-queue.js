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

  function change(next, persist = true) {
    current.current = next;
    setJobs(next);
    if (!persist) return;
    const snapshot = next.map(({segments, measured, message, ...job}) => job);
    store.setItem(queueStorageKey, JSON.stringify(snapshot));
  }

  function patch(id, changes, persist) {
    change(current.current.map(job => job.id === id ? {...job,...changes} : job), persist);
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
    patch(job.id, {stage:'decoding'});
    (async () => {
      let changes;
      try {
        const record = await desktop.run(job, ({stage, ...data}, segments, reset) => {
          const active = current.current.find(item => item.id === handle.id);
          if (stage) handle.stage = stage;
          const nextStage = active.stage === 'cancelling' ? active.stage : handle.stage;
          const previous = reset ? [] : active.segments || [];
          patch(job.id, {...data, stage:nextStage,
            segments:segments.length ? previous.concat(segments) : previous}, false);
        }, () => {
          handle.ready = true;
          if (current.current.find(item => item.id === handle.id)?.stage === 'cancelling') {
            void cancelRunning(handle);
          }
        });
        await store.saveTranscription(record);
        onComplete(record);
        changes = {stage:'completed', progress:100, segments:undefined, error:undefined};
        notify('转录完成：' + record.title);
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

  function retry(id) {
    if (!current.current.some(job => job.id === id && queueRetryStages.has(job.stage))) return;
    patch(id, {stage:'queued', progress:0, error:undefined, message:undefined, measured:false, segments:undefined});
  }

  function remove(id) {
    change(current.current.filter(job => job.id !== id || (job.stage === 'queued' || queueActiveStages.has(job.stage))));
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
    segments:job.segments || emptyLiveSegments,
  };
}
