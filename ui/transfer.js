function transferProgress(jsx, name, value, compact = false) {
  const measured = Number.isFinite(value);
  const progress = measured ? Math.max(0, Math.min(100, value)) : undefined;
  return jsx('div', {
    role:'progressbar', 'aria-label':name + ' 转录进度', 'aria-valuenow':progress,
    className:'transfer-progress' + (compact ? ' transfer-progress-compact' : ''),
    children:jsx('div', {
      className:'transfer-progress-fill' + (measured ? '' : ' transfer-progress-pulse'),
      style:measured ? {width:progress + '%'} : undefined,
    }),
  });
}

function TransferTermsDialog({React, jsx, file, onSave, onClose}) {
  const dialog = React.useRef(null);
  const addedInput = React.useRef(null);
  const focusNew = React.useRef(false);
  const nextId = React.useRef(file.terms?.length || 1);
  const [rows, setRows] = React.useState(() => file.terms?.length
    ? file.terms.map((term, id) => ({...term, id}))
    : [{id:0, from:'', to:''}]);
  const [error, setError] = React.useState('');
  React.useLayoutEffect(() => {
    const panel = dialog.current;
    panel.showModal();
    panel.querySelector('input').focus();
    return () => panel.close();
  }, []);
  React.useLayoutEffect(() => {
    if (focusNew.current) {
      focusNew.current = false;
      addedInput.current?.focus();
    }
  }, [rows.length]);

  function update(id, field, value) {
    setRows(items => items.map(row => row.id === id ? {...row, [field]:value} : row));
    setError('');
  }
  function save(event) {
    event.preventDefault();
    const terms = rows.map(({from,to}) => ({from:from.trim(), to:to.trim()}))
      .filter(({from,to}) => from || to);
    if (terms.some(({to}) => !to)) return setError('请为已填写的词语输入正确写法。');
    const originals = terms.map(({from}) => from).filter(Boolean);
    if (new Set(originals).size !== originals.length) return setError('同一个易错词只需填写一次。');
    dialog.current.close();
    onSave(file.path, terms);
  }
  function dismiss() {
    dialog.current.close();
    onClose();
  }
  return jsx('dialog', {
    ref:dialog, className:'transfer-terms', 'aria-labelledby':'transfer-terms-title',
    onCancel:event => {event.preventDefault(); dismiss();},
    children:jsx('form', {onSubmit:save, children:[
      jsx('header', {children:[
        jsx('h2', {id:'transfer-terms-title', children:'词语纠正'}),
        jsx('p', {className:'transfer-terms-file', title:file.name, children:file.name}),
        jsx('p', {children:'词语仅用于此文件。只填正确写法时，该词将作为识别提示。'}),
      ]}),
      jsx('div', {className:'transfer-terms-body', children:[
        jsx('div', {className:'transfer-term-columns', 'aria-hidden':true, children:[
          jsx('span', {children:'易错词（选填）'}),
          jsx('span', {children:'正确写法'}),
        ]}),
        !rows.length && jsx('p', {className:'transfer-terms-empty', children:'暂无词语'}),
        ...rows.map((row, index) => jsx('div', {className:'transfer-term-row', children:[
          jsx('input', {ref:index===rows.length-1 ? addedInput : undefined, 'aria-label':'易错词（选填）',
            value:row.from, placeholder:index===0 ? '例如 爱菲尔铁塔' : '', onChange:event=>update(row.id,'from',event.target.value)}),
          jsx('input', {'aria-label':'正确写法', value:row.to,
            placeholder:index===0 ? '例如 埃菲尔铁塔' : '', onChange:event=>update(row.id,'to',event.target.value)}),
          jsx('button', {
            type:'button', className:'transfer-review-button', 'aria-label':`移除第 ${index + 1} 条词语`,
            onClick:()=>{setRows(items=>items.filter(item=>item.id!==row.id)); setError('');}, children:'×',
          }),
        ]}, row.id)),
      ]}),
      error && jsx('p', {className:'transfer-terms-error', role:'alert', children:error}),
      jsx('footer', {children:[
        jsx('button', {type:'button', className:'transfer-review-button transfer-terms-add',
          onClick:()=>{
            const id = nextId.current++;
            focusNew.current = true;
            setRows(items=>[...items,{id,from:'',to:''}]);
          }, children:'＋ 添加词语'}),
        jsx('button', {type:'button', className:'transfer-review-button', onClick:dismiss, children:'取消'}),
        jsx('button', {type:'submit', className:'transfer-review-button transfer-review-start', children:'保存'}),
      ]}),
    ]}),
  });
}

function transferReview(jsx, Dropdown, {draft, icons, onAdd, onStart, onCancel, onRemove, onFileOptions, termsEditor, onEditTerms}) {
  const icon = type => jsx(icons[type], {'aria-hidden':true, size:17, strokeWidth:1.7});
  const smallIcon = path => jsx('svg', {width:14, height:14, viewBox:'0 0 24 24',
    fill:'none', stroke:'currentColor', strokeWidth:1.7, strokeLinecap:'round',
    strokeLinejoin:'round', 'aria-hidden':true, children:jsx('path',{d:path})});
  const sparkle = smallIcon('M9.9 18.4 12 22l2.1-3.6a8 8 0 0 1 4.3-4.3L22 12l-3.6-2.1a8 8 0 0 1-4.3-4.3L12 2 9.9 5.6a8 8 0 0 1-4.3 4.3L2 12l3.6 2.1a8 8 0 0 1 4.3 4.3Z');
  const braces = smallIcon('M8 3H7a2 2 0 0 0-2 2v4a3 3 0 0 1-3 3 3 3 0 0 1 3 3v4a2 2 0 0 0 2 2h1m8-18h1a2 2 0 0 1 2 2v4a3 3 0 0 0 3 3 3 3 0 0 0-3 3v4a2 2 0 0 1-2 2h-1');
  const languages = [
    ['auto','自动识别'], ['zh','中文'], ['en','English'], ['ja','日语'], ['zh-en','中英混合'],
  ].map(([value,label]) => ({value,label}));
  const audioExtensions = /^(mp3|wav|m4a|aac|flac|ogg|opus|wma|m4b)$/i;
  return jsx('section', {className:'transfer-page transfer-review', 'aria-labelledby':'transfer-review-heading', children:[
    jsx('header', {children:jsx('span', {children:'确认转录'})}),
    jsx('div', {className:'transfer-review-body', children:[
      jsx('div', {className:'transfer-review-title', children:[
        jsx('h2', {id:'transfer-review-heading', children:'待转录文件'}),
        jsx('p', {'aria-live':'polite', children:draft.length + ' 个文件待处理'}),
      ]}),
      jsx('div', {className:'transfer-review-tools', children:[
        jsx('h3', {id:'transfer-file-heading', children:'文件清单'}),
        jsx('div', {className:'transfer-review-controls', children:[
          jsx('button', {type:'button', className:'transfer-review-button transfer-review-add',
            onClick:onAdd, children:[icon('add'), '添加文件']}),
        ]}),
      ]}),
      draft.length ? jsx('ul', {className:'transfer-file-list', 'aria-labelledby':'transfer-file-heading',
        children:draft.map(({name,path,terms,options}) => {
          const dot = name.lastIndexOf('.');
          const extension = dot > 0 ? name.slice(dot + 1) : '';
          const separator = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
          const directory = separator < 0 ? path : path.slice(0, separator + 1);
          return jsx('li', {className:'transfer-file', children:[
            jsx('span', {className:'transfer-file-icon', children:icon(audioExtensions.test(extension) ? 'audio' : 'video')}),
            jsx('div', {className:'transfer-file-detail', children:[
              jsx('div', {className:'transfer-file-name', children:[
                jsx('strong', {title:name, children:name}),
                extension && jsx('span', {className:'transfer-file-extension', children:extension.toUpperCase()}),
              ]}),
              jsx('span', {className:'transfer-file-path', title:path, children:directory}),
            ]}),
            jsx('div', {className:'transfer-file-controls', children:[
              jsx(Dropdown, {label:'语种 · ' + name, value:options.language, items:languages,
                trigger:[
                  jsx('span',{className:'transfer-language-label','aria-hidden':true,children:'语种'}),
                  jsx('span',{className:'mono-dropdown-label',children:languages.find(item => item.value === options.language)?.label || '自动识别'}),
                ],
                minWidth:120, onChange:language=>onFileOptions(path,{language})}),
              jsx('button', {type:'button', className:'transfer-review-button transfer-file-terms',
                'aria-label':'词语纠正 ' + name, 'aria-haspopup':'dialog', onClick:()=>onEditTerms(path),
                children:[sparkle, terms?.length ? `词语纠正 · ${terms.length}` : '词语纠正']}),
              jsx('button', {type:'button', className:'transfer-review-button transfer-file-json',
                'aria-pressed':options.characterJson, 'aria-label':'生成逐字 JSON · ' + name,
                title:'转录完成后为此文件保存逐字 JSON',
                onClick:()=>onFileOptions(path,{characterJson:!options.characterJson}),
                children:[braces, '逐字 JSON']}),
            ]}),
            jsx('button', {type:'button', className:'transfer-review-button transfer-file-remove',
              title:'移除文件', 'aria-label':'移除 ' + name, onClick:event=>{
                const row = event.currentTarget.closest('li');
                const next = row.nextElementSibling || row.previousElementSibling;
                const fallback = row.closest('.transfer-review').querySelector('.transfer-review-add');
                (next?.querySelector('.transfer-file-remove') || fallback).focus();
                onRemove(path);
              }, children:icon('remove')}),
          ]}, path);
        })}) : jsx('p', {className:'transfer-review-empty', children:'清单为空，请添加音频或视频文件。'}),
    ]}),
    jsx('footer', {className:'transfer-review-footer', children:
      jsx('div', {className:'transfer-review-actions', children:[
        jsx('button', {type:'button', className:'transfer-review-button', onClick:onCancel, children:'取消'}),
        jsx('button', {type:'button', className:'transfer-review-button transfer-review-start',
          disabled:!draft.length, onClick:onStart, children:draft.length ? `开始转录 (${draft.length})` : '开始转录'}),
      ]}),
    }),
    termsEditor,
  ]});
}

function transferJobAction(jsx, job, onCancel, onRetry) {
  const retry = queueRetryStages.has(job.stage);
  if (!retry && !['queued', 'decoding', 'inferring'].includes(job.stage)) return null;
  const label = retry ? '恢复选项' : '取消';
  return jsx('button', {
    type:'button', className:'queue-action', 'aria-label':label + ' ' + job.fileName,
    onClick:() => (retry ? onRetry : onCancel)(job.id), children:label,
  });
}

function transferJobCard(jsx, job, {selected, onOpen, onCancel, onRemove, CloseIcon}) {
  const active = queueActiveStages.has(job.stage);
  return jsx('div', {className:'queue-card', 'data-selected':selected || undefined, children:[
    jsx('div', {className:'queue-card-top', children:[
      jsx('button', {
        type:'button', className:'queue-card-name', title:job.fileName, 'aria-current':selected || undefined,
        onClick:() => onOpen(job.id), children:job.fileName,
      }),
      (job.stage === 'completed' || queueRetryStages.has(job.stage)) && jsx('button', {
        type:'button', className:'queue-remove', title:'移出队列',
        'aria-label':'移出队列 ' + job.fileName, onClick:() => onRemove(job.id),
        children:jsx(CloseIcon, {size:14, 'aria-hidden':true}),
      }),
    ]}),
    jsx('div', {className:'queue-card-status', children:[
      jsx('span', {children:queueLabels[job.stage]}),
      job.measured && active && jsx('span', {className:'font-mono tabular-nums', children:job.progress + '%'}),
      transferJobAction(jsx, job, onCancel, onOpen),
    ]}),
    active && transferProgress(jsx, job.fileName, job.measured ? job.progress : undefined, true),
  ]}, job.id);
}

function transferLiveStatus(jsx, job, onCancel, onRetry, draft, notify) {
  const {page, go, tail, error} = draft;
  const exportDraft = async format => {
    try {
      const title = job.fileName.replace(/\.[^.]+$/, '');
      if (await desktop.invoke('draft_export', {jobId:job.id, attempt:page.attempt, title, format})) {
        notify('部分稿已导出');
      }
    } catch (error) {
      notify('导出失败：' + error);
    }
  };
  const active = queueActiveStages.has(job.stage);
  const recoverable = queueRetryStages.has(job.stage);
  const label = job.stage === 'error' ? job.error
        : ['decoding', 'inferring'].includes(job.stage) ? job.message || queueLabels[job.stage]
    : queueLabels[job.stage];
  return jsx('div', {className:'queue-live-status', children:[
    jsx('div', {className:'queue-live-message', children:[
      jsx('span', {role:active ? undefined : 'status', children:label}),
      recoverable
        ? jsx('div', {className:'queue-draft-tools', children:[
          page.canResume && jsx('button', {type:'button', className:'queue-action', onClick:() => onRetry(job.id, true), children:'继续转录'}),
          jsx('button', {type:'button', className:'queue-action', onClick:() => onRetry(job.id), children:'从头转录'}),
        ]})
        : transferJobAction(jsx, job, onCancel),
    ]}),
    error && jsx('p', {role:'alert', children:'读取已保存的转录稿失败：' + error}),
    recoverable && page.resumeReason && jsx('div', {className:'queue-draft-tools', children:page.resumeReason}),
    page.total > 0 && jsx('div', {className:'queue-draft-tools', children:[
      jsx('span', {children:'已保存至 ' + draftTime(page.savedUntil)}),
      jsx('button', {type:'button', className:'queue-action', onClick:() => exportDraft('srt'), children:'导出部分 SRT'}),
      jsx('button', {type:'button', className:'queue-action', onClick:() => exportDraft('txt'), children:'导出部分 TXT'}),
    ]}),
    (page.start > 0 || page.start + page.segments.length < page.total || (!tail && active)) &&
      jsx('div', {className:'queue-draft-tools', children:[
        jsx('span', {children:`第 ${page.start + 1}–${page.start + page.segments.length} 段，共 ${page.total} 段`}),
        jsx('button', {type:'button', className:'queue-action', disabled:page.start === 0,
          onClick:() => go(Math.max(0, page.start - page.segments.length)), children:'看更早'}),
        jsx('button', {type:'button', className:'queue-action', disabled:page.start + page.segments.length >= page.total,
          onClick:() => go(page.start + page.segments.length), children:'看后续'}),
        !tail && jsx('button', {type:'button', className:'queue-action', onClick:() => go(null), children:'回到最新'}),
      ]}),
    (active || job.stage === 'queued') && transferProgress(jsx, job.fileName,
      job.stage === 'queued' ? 0 : job.measured ? job.progress : undefined, true),
  ]});
}

function draftTime(seconds) {
  const time = Math.floor(seconds);
  const part = value => String(value).padStart(2, '0');
  const minutes = part(Math.floor(time / 60) % 60);
  const hours = Math.floor(time / 3600);
  return (hours ? part(hours) + ':' : '') + minutes + ':' + part(time % 60);
}
