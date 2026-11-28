function createBatchExport(React, jsx, {renderSrt, renderTxt, renderJson, exportOptions}) {
  const formats = [['srt', 'SRT 字幕'], ['txt', 'TXT 文本'], ['json', '逐字 JSON']];
  const pageSize = 30;

  return function BatchExport({records}) {
    const dialog = React.useRef(null);
    const list = React.useRef(null);
    const [opened, setOpened] = React.useState(false);
    const [search, setSearch] = React.useState('');
    const [page, setPage] = React.useState(1);
    const [selected, setSelected] = React.useState(() => new Set());
    const [chosenFormats, setChosenFormats] = React.useState(() => new Set(['srt']));
    const [busy, setBusy] = React.useState(false);
    const [notice, setNotice] = React.useState(null);
    const keyword = search.trim().toLowerCase();
    const matches = React.useMemo(() => keyword ? records.filter(record =>
      [record.title, record.sourceFileName].some(name => name?.toLowerCase().includes(keyword))) : records,
      [records, keyword]);
    const selectedCount = opened ? records.filter(record => selected.has(record.id)).length : 0;
    const allSelected = opened && matches.length > 0 && matches.every(record => selected.has(record.id));
    const pageCount = Math.max(1, Math.ceil(matches.length / pageSize));
    const currentPage = Math.min(page, pageCount);
    const start = (currentPage - 1) * pageSize;

    React.useLayoutEffect(() => {
      if (list.current) list.current.scrollTop = 0;
    }, [keyword, currentPage]);

    function toggle(set, value) {
      set(current => {
        const next = new Set(current);
        if (next.has(value)) next.delete(value);
        else next.add(value);
        return next;
      });
      setNotice(null);
    }

    function open() {
      setSearch('');
      setPage(1);
      setSelected(new Set());
      setNotice(null);
      setOpened(true);
      dialog.current.showModal();
    }

    async function save(event) {
      event.preventDefault();
      if (busy || !selectedCount || !chosenFormats.size) return;
      setNotice(null);
      setBusy(true);
      try {
        const date = new Date();
        const timestamp = [date.getFullYear(), date.getMonth() + 1, date.getDate(),
          date.getHours(), date.getMinutes(), date.getSeconds()]
          .map(part => String(part).padStart(2, '0')).join('');
        const options = exportOptions();
        const extensions = formats.map(([value]) => value).filter(value => chosenFormats.has(value));
        const render = {srt:renderSrt, txt:renderTxt, json:renderJson};
        const entries = records.filter(record => selected.has(record.id)).flatMap(record =>
          extensions.map(ext => [record.title, ext, render[ext](record, options)]));
        const saved = await desktop.invoke('export_zip', {name:timestamp + '.zip', entries});
        if (saved) setNotice({text:`已将 ${entries.length} 个文件保存到 ZIP。`});
      } catch (error) {
        setNotice({text:`导出失败：${error}`, error:true});
      } finally {
        setBusy(false);
      }
    }

    const checkbox = (label, checked, onChange, className) => jsx('label', {className, children:[
      jsx('input', {type:'checkbox', checked, onChange, disabled:busy}),
      jsx('span', {children:label}),
    ]});

    return jsx(React.Fragment, {children:[
      jsx('button', {type:'button', className:'batch-export-trigger', onClick:open, disabled:!records.length,
        children:'批量导出'}),
      jsx('dialog', {ref:dialog, className:'batch-export-dialog', 'aria-labelledby':'batch-export-title',
        onCancel:event => {event.stopPropagation(); if (busy) event.preventDefault();},
        onClose:() => setOpened(false),
        children:opened && jsx('form', {onSubmit:save, children:[
          jsx('header', {children:[
            jsx('h2', {id:'batch-export-title', children:'批量导出转录'}),
            jsx('button', {type:'button', onClick:() => dialog.current.close(), disabled:busy, children:'关闭'}),
          ]}),
          jsx('input', {type:'search', autoFocus:true, className:'batch-export-search',
            'aria-label':'搜索转录记录', placeholder:'搜索标题或音频文件名', value:search, disabled:busy,
            onChange:event => {setSearch(event.target.value); setPage(1);},
            onKeyDown:event => {if (event.key === 'Enter') event.preventDefault();},
          }),
          jsx('div', {className:'batch-export-selection', children:[
            jsx('button', {type:'button', disabled:busy || !matches.length, onClick:() => {
              setSelected(current => {
                const next = new Set(current);
                for (const {id} of matches) {
                  if (allSelected) next.delete(id);
                  else next.add(id);
                }
                return next;
              });
              setNotice(null);
            }, children:(allSelected ? '取消全选' : '全选') + (keyword ? '搜索结果' : '')}),
            jsx('span', {'aria-live':'polite', children:`已选 ${selectedCount} / ${records.length} 条`}),
          ]}),
          matches.length ? jsx('ul', {ref:list, className:'batch-export-records', 'aria-label':'转录记录', children:matches.slice(start, start + pageSize).map(record =>
            jsx('li', {children:checkbox(record.title || '未命名转录', selected.has(record.id),
              () => toggle(setSelected, record.id), 'batch-export-record')}, record.id))}) :
            jsx('p', {className:'batch-export-empty', role:'status', children:keyword ? '没有匹配的转录记录' : '暂无转录记录'}),
          pageCount > 1 && jsx('nav', {className:'batch-export-pagination', 'aria-label':'导出列表分页', children:[
            jsx('button', {type:'button', disabled:busy || currentPage === 1,
              onClick:() => setPage(currentPage - 1), children:'上一页'}),
            jsx('span', {'aria-live':'polite', children:`${currentPage} / ${pageCount} 页`}),
            jsx('button', {type:'button', disabled:busy || currentPage === pageCount,
              onClick:() => setPage(currentPage + 1), children:'下一页'}),
          ]}),
          jsx('fieldset', {className:'batch-export-formats', children:[
            jsx('legend', {children:'文件格式'}),
            ...formats.map(([value, label]) => jsx(React.Fragment, {children:
              checkbox(label, chosenFormats.has(value), () => toggle(setChosenFormats, value)),
            }, value)),
          ]}),
          notice && jsx('p', {role:notice.error ? 'alert' : 'status', children:notice.text}),
          jsx('footer', {children:jsx('button', {type:'submit', className:'batch-export-save',
            disabled:busy || !selectedCount || !chosenFormats.size, children:busy ? '正在导出…' : '保存 ZIP'})}),
        ]}),
      }),
    ]});
  };
}
