/* Native services used by the supplied InkVoice interface. */
const desktop = (() => {
  const {invoke,convertFileSrc} = window.__TAURI__.core;
  let store, initial, migrated;
  function normalize(r) {
    if (!r.name) return r.audioPath ? {...r,audioUrl:convertFileSrc(r.audioPath)} : r;
    return {id:r.id,title:r.name.replace(/\.[^.]+$/,''),sourceFileName:r.name,
      fileSizeBytes:r.fileSizeBytes || 0,durationSec:r.duration,sampleRate:16000,
      createdAt:new Date(r.created).toISOString(),modelId:r.model,modelName:r.model.replace('faster-whisper-','Whisper '),
      language:r.language,starred:false,audioPath:r.audio,audioUrl:convertFileSrc(r.audio),
      waveform:r.waveform || Array(72).fill(.25),segments:r.segments.map((s,i)=>({id:r.id+'-'+i,startSec:s.start,endSec:s.end,text:s.text}))};
  }
  async function prepare() {
    store = await new Promise((resolve,reject)=>{
      const req=indexedDB.open('inkvoice',2);
      req.onupgradeneeded=()=>{if(!req.result.objectStoreNames.contains('records'))req.result.createObjectStore('records',{keyPath:'id'});req.result.createObjectStore('library');};
      req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);
    });
    const read = (table,key)=>new Promise((resolve,reject)=>{
      const tableRef=store.transaction(table).objectStore(table);
      const req=key?tableRef.get(key):tableRef.getAll();
      req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);
    });
    initial=await read('library','records');
    migrated = !initial;
    if(!initial) initial=(await read('records')).sort((a,b)=>b.created-a.created).map(normalize);
    else initial=initial.map(normalize);
  }
  function load(demos) {return migrated ? [...demos,...initial] : initial;}
  function save(records) {
    const tx=store.transaction('library','readwrite');
    tx.objectStore('library').put(records.map(({audioUrl,...r})=>r),'records');
    tx.oncomplete=()=>{localStorage.libraryCreated='1';};
    tx.onabort=()=>alert('转写记录保存失败：'+tx.error);
  }
  async function run(job,update) {
    const off=await window.__TAURI__.event.listen('progress',({payload})=>{
      const match=payload.message.match(/(\d+)%/);
      update({stage:payload.message.includes('音轨')?'decoding':'inferring',message:payload.message,measured:!!match,progress:match?Number(match[1]):0});
    });
    try {
      const r=await invoke('transcribe',{source:job.path,model:job.modelId,options:job.options});
      return {...normalize({...r,name:job.fileName,created:Date.now(),model:job.modelId}),modelName:job.modelName};
    } finally {off();}
  }
  async function paths(files) {
    return Promise.all(files.map(async f=>typeof f==='string' ? {path:f,name:f.split(/[\\/]/).pop()} :
      {path:await invoke('store_recording',new Uint8Array(await f.arrayBuffer())),name:f.name}));
  }
  return {invoke,prepare,load,save,run,paths};
})();