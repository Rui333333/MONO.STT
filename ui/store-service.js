/* SQLite-backed library and preferences. Browser storage is read only during migration. */
class MonoStoreService {
  records = [];
  kv = {};
  queue = Promise.resolve();
  deleting = new Set();
  directory = '';

  enqueue(task) {
    const result = this.queue.then(task);
    this.queue = result.catch(() => {});
    return result;
  }

  report(error) {
    console.error('保存失败', error);
    alert('保存失败：' + error);
  }

  async init({normalize}) {
    const {invoke} = window.__TAURI__.core;
    this.invoke = invoke;
    let snapshot = await invoke('store_load');
    const marker = 'mono_store_migrated_v1';
    if (!snapshot.kv[marker]) {
      const keys = ['inkvoice_settings_v1', 'inkvoice_active_model_v1', 'mono_gpu_setup_v1', 'libraryCreated'];
      const kv = Object.fromEntries(keys.map(key => [key, localStorage.getItem(key)]).filter(([,value]) => value !== null));
      const records = (await this.readLegacyRecords() ?? JSON.parse(localStorage.getItem('inkvoice_records_v1') || '[]'))
        .map(normalize).map(({audioUrl,...record}) => record);
      await invoke('store_import', {records, kv:{...kv, [marker]:'1'}});
      snapshot = await invoke('store_load');
    }
    this.records = snapshot.records;
    this.kv = snapshot.kv;
    this.directory = snapshot.directory;
  }

  async readLegacyRecords() {
    const db = await new Promise((resolve,reject) => {
      const req = indexedDB.open('inkvoice',2);
      req.onupgradeneeded = () => {
        if (!req.result.objectStoreNames.contains('records')) req.result.createObjectStore('records',{keyPath:'id'});
        if (!req.result.objectStoreNames.contains('library')) req.result.createObjectStore('library');
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    const read = (table,key) => new Promise((resolve,reject) => {
      const store = db.transaction(table).objectStore(table);
      const request = key ? store.get(key) : store.getAll();
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    try {
      const library = await read('library','records');
      if (library !== undefined) return library;
      const records = await read('records');
      return records.length ? records : undefined;
    } finally { db.close(); }
  }

  load(demos) {
    return this.records.length || this.kv.libraryCreated ? this.records : demos;
  }

  save(records) {
    const snapshot = records.filter(record => !this.deleting.has(record.id))
      .map(({audioUrl,...record}) => record);
    this.enqueue(async () => {
      const before = new Map(this.records.map(record => [record.id, JSON.stringify(record)]));
      const changed = snapshot.filter(record => !this.deleting.has(record.id) && before.get(record.id) !== JSON.stringify(record));
      if (changed.length) await this.invoke('store_save', {records:changed});
      const updated = new Map(this.records.map(record => [record.id, record]));
      for (const record of changed) updated.set(record.id,record);
      this.records = [...updated.values()];
      if (!this.kv.libraryCreated) {
        await this.invoke('store_kv_set',{key:'libraryCreated',value:'1'});
        this.kv.libraryCreated = '1';
      }
    }).catch(error => this.report(error));
  }

  saveTranscription({audioUrl, ...record}) {
    return this.enqueue(async () => {
      if (!this.kv.libraryCreated) {
        await this.invoke('store_kv_set', {key:'libraryCreated', value:'1'});
        this.kv.libraryCreated = '1';
      }
      await this.invoke('store_save', {records:[record]});
      this.records = [record, ...this.records.filter(item => item.id !== record.id)];
    });
  }

  async deleteRecords(ids) {
    if (ids.some(id => this.deleting.has(id))) throw Error('正在删除此记录');
    for (const id of ids) this.deleting.add(id);
    try {
      const result = await this.enqueue(() => this.invoke('store_delete_records',{ids}));
      this.records = this.records.filter(record => !this.deleting.has(record.id) || !ids.includes(record.id));
      return result;
    } finally {
      for (const id of ids) this.deleting.delete(id);
    }
  }

  getItem(key) { return this.kv[key] ?? null; }

  setItem(key, value) {
    this.kv[key] = String(value);
    this.enqueue(() => this.invoke('store_kv_set',{key,value:String(value)})).catch(error => this.report(error));
  }
}

window.MonoStore = new MonoStoreService();
