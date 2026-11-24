/* Native services used by the supplied InkVoice interface. */
const desktop = (() => {
  const {invoke,convertFileSrc} = window.__TAURI__.core;
  function normalize(r) {
    if (!r.name) return r.audioPath ? {...r,audioUrl:convertFileSrc(r.audioPath)} : r;
    return {id:r.id,title:r.name.replace(/\.[^.]+$/,''),sourceFileName:r.name,
      fileSizeBytes:r.fileSizeBytes || 0,durationSec:r.duration,sampleRate:16000,
      createdAt:new Date(r.created).toISOString(),modelId:r.model,modelName:r.model.replace('faster-whisper-','Whisper '),
      language:r.language,starred:false,audioPath:r.audio,audioUrl:convertFileSrc(r.audio),
      waveform:r.waveform || Array(72).fill(.25),segments:r.segments.map((s,i)=>({id:r.id+'-'+i,startSec:s.start,endSec:s.end,text:s.text}))};
  }
  /* SQLite is the single source of truth; MonoStore owns the snapshot and
     all writes. normalize() is only kept for the one-shot legacy import. */
  async function prepare() { await invoke("init_store"); return window.MonoStore.init({normalize}); }
  function load(demos) {
    return window.MonoStore.load(demos).map(r => r.audioPath
      ? {...r,audioUrl:convertFileSrc(r.audioPath)} : r);
  }
  function save(records) { return window.MonoStore.save(records); }
  async function run(job,update) {
    const off=await window.__TAURI__.event.listen('progress',({payload})=>{
      const match=payload.message.match(/(\d+)%/);
      update({stage:payload.message.includes('音轨')?'decoding':'inferring',message:payload.message,measured:!!match,progress:match?Number(match[1]):0});
    });
    try {
      const r=await invoke('transcribe',{source:job.path,model:job.modelId,options:job.options});
      const record=normalize({...r,name:job.fileName,created:Date.now(),model:job.modelId});
      /* sourcePath lets the store remove the app-owned recording together
         with the WAV. External files are rejected by the Rust path guard. */
      return {...record,sourcePath:job.path,modelName:job.modelName};
    } finally {off();}
  }
  async function paths(files) {
    return Promise.all(files.map(async f=>typeof f==='string' ? {path:f,name:f.split(/[\\/]/).pop()} :
      typeof f.path==='string' ? {path:f.path,name:f.name} :
      {path:await invoke('store_recording',new Uint8Array(await f.arrayBuffer())),name:f.name}));
  }
  return {invoke,prepare,load,save,run,paths};
})();
/* Model library state comes from disk; only preferences are persisted by the UI. */
const modelLibrary = (() => {
  const {invoke} = desktop;
  let defaultDir;
  let startingModels = [];

  function settings(saved = {}) {
    const modelsDir = !saved.modelsDir || (!saved.modelDirectoryVersion &&
      saved.modelsDir.toLowerCase() === 'd:\\aimodel') ? '' : saved.modelsDir;
    return {
      ...saved, modelsDir, modelDirectoryVersion: 1,
      mirrorSource: saved.mirrorSource === 'huggingface' ? 'huggingface' : 'hf-mirror',
    };
  }

  async function prepare(saved) {
    defaultDir = await invoke('default_models_dir');
    try {
      startingModels = await scan(saved.modelsDir);
    } catch (error) {
      console.warn(error);
    }
  }

  function scan(modelsDir) {
    return invoke('list_local_models', {modelsDir});
  }

  function withInstalled(catalog, installed) {
    return catalog.map(model => {
      const isInstalled = installed.includes(model.id);
      return {...model, status: isInstalled ? 'downloaded' : 'not_downloaded',
        progress: isInstalled ? 100 : 0, downloadSpeedMBs: undefined};
    });
  }

  async function download({modelsDir, model, source}, update) {
    const requestId = crypto.randomUUID();
    const off = await window.__TAURI__.event.listen('model-download-progress', ({payload}) => {
      if (payload.requestId === requestId) update(payload);
    });
    try {
      return await invoke('download_model', {modelsDir, model, source, requestId});
    } finally {
      off();
    }
  }

  return {
    prepare, settings, scan, download, withInstalled,
    initial: catalog => withInstalled(catalog, startingModels),
    displayDir: modelsDir => modelsDir || defaultDir,
    pick: modelsDir => invoke('pick_models_dir', {modelsDir}),
    cancel: (modelsDir, model) => invoke('cancel_model_download', {modelsDir, model}),
  };
})();

function createGpuStatus(React, jsx) {
  return function GpuStatus() {
    const [status, setStatus] = React.useState(null);
    const [attempt, retry] = React.useState(0);
    React.useEffect(() => {
      let active = true;
      desktop.invoke('gpu_status').then(
        value => { if (active) setStatus(value); },
        error => { if (active) setStatus({adapters:[], reason:String(error), failed:true}); },
      );
      return () => { active = false; };
    }, [attempt]);

    const availability = !status ? '检测中…' : status.failed ? '检测失败' :
      status.cudaAvailable ? 'CUDA 可用' : 'CUDA 不可用';
    return jsx('section', {className:'gpu-status', 'aria-label':'显卡与 CUDA', children:[
      jsx('div', {className:'gpu-status-header', children:[
        jsx('div', {className:'gpu-status-title', children:'显卡与 CUDA'}),
        jsx('button', {
          type:'button', disabled:!status,
          onClick:() => { setStatus(null); retry(value => value + 1); },
          children:'重新检测',
        }),
      ]}),
      jsx('div', {role:'status', children:[
        jsx('div', {className:'gpu-status-summary', children:availability}),
        status?.adapters?.length ? jsx('ul', {children:status.adapters.map((adapter, index) =>
          jsx('li', {children:[
            jsx('span', {children:adapter.name}),
            adapter.memoryMiB && jsx('span', {className:'gpu-status-detail',
              children:(adapter.memoryMiB / 1024).toFixed(1) + ' GB 显存' +
                (adapter.capability ? ' · 计算能力 ' + adapter.capability : '')}),
          ]}, index),
        )}) : status && !status.failed && jsx('p', {className:'gpu-status-detail', children:'未识别到显卡型号'}),
        status?.cudaAvailable && jsx('p', {className:'gpu-status-detail',
          children:'推理精度 ' + status.computeTypes.join(' / ') + ' · 模型能否运行取决于可用显存'}),
        status?.reason && jsx('p', {className:'gpu-status-detail', children:status.reason}),
        status && !status.cudaAvailable && status.adapters?.some(adapter=>/nvidia/i.test(adapter.name)) &&
          jsx('button',{type:'button',onClick:()=>desktop.invoke('open_cuda_download').catch(error=>setStatus(previous=>({...previous,reason:String(error)}))),
            children:'打开 NVIDIA 官方 CUDA 12.9 下载页'}),

      ]}),
    ]});
  };
}
