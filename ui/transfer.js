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

function transferReview(jsx, Dropdown, {draft, icons, onAdd, onStart, onCancel, onRemove, onLanguage, language}) {
  const icon = type => jsx(icons[type], {'aria-hidden':true, size:17, strokeWidth:1.7});
  const languages = [
    ['auto','自动识别'], ['zh','中文'], ['en','英语'], ['ja','日语'], ['zh-en','中英混合'],
  ].map(([value,label]) => ({value,label}));
  const audioExtensions = /^(mp3|wav|m4a|aac|flac|ogg|opus|wma|m4b)$/i;
  return jsx('section', {className:'transfer-page transfer-review', 'aria-labelledby':'transfer-review-heading', children:[
    jsx('header', {children:jsx('span', {children:'确认转录'})}),
    jsx('div', {className:'transfer-review-body', children:[
      jsx('div', {className:'transfer-review-title', children:[
        jsx('h2', {id:'transfer-review-heading', children:'待转录文件'}),
        jsx('p', {'aria-live':'polite', children:draft.length + ' 个文件'}),
      ]}),
      jsx('div', {className:'transfer-review-tools', children:[
        jsx('h3', {id:'transfer-file-heading', children:'文件清单'}),
        jsx('div', {className:'transfer-review-controls', children:[
          jsx('div', {className:'transfer-language', children:[
            jsx('span', {'aria-hidden':true, children:'语种'}),
            jsx(Dropdown, {label:'语种', value:language, onChange:onLanguage, items:languages}),
          ]}),
          jsx('button', {type:'button', className:'transfer-review-button transfer-review-add',
            onClick:onAdd, children:[icon('add'), '添加文件']}),
        ]}),
      ]}),
      draft.length ? jsx('ul', {className:'transfer-file-list', 'aria-labelledby':'transfer-file-heading',
        children:draft.map(({name,path}) => {
          const dot = name.lastIndexOf('.');
          const extension = dot > 0 ? name.slice(dot + 1) : '';
          const separator = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
          const directory = separator < 0 ? path : path.slice(0, separator + 1);
          return jsx('li', {className:'transfer-file', children:[
            jsx('span', {className:'transfer-file-icon', children:icon(audioExtensions.test(extension) ? 'audio' : 'video')}),
            jsx('div', {className:'transfer-file-detail', children:[
              jsx('strong', {title:name, children:name}),
              jsx('span', {title:path, children:directory}),
            ]}),
            jsx('span', {className:'transfer-file-extension', 'aria-hidden':true, children:extension.toUpperCase()}),
            jsx('button', {type:'button', className:'transfer-review-button transfer-file-remove',
              title:'移除文件', 'aria-label':'移除 ' + name, onClick:event=>{
                const row = event.currentTarget.closest('li');
                const next = row.nextElementSibling || row.previousElementSibling;
                const fallback = row.closest('.transfer-review').querySelector('.transfer-review-add');
                (next?.querySelector('button') || fallback).focus();
                onRemove(path);
              }, children:icon('remove')}),
          ]}, path);
        })}) : jsx('p', {className:'transfer-review-empty', children:'清单为空，请添加音频或视频文件。'}),
    ]}),
    jsx('footer', {className:'transfer-review-footer', children:
      jsx('div', {className:'transfer-review-actions', children:[
        jsx('button', {type:'button', className:'transfer-review-button', onClick:onCancel, children:'取消'}),
        jsx('button', {type:'button', className:'transfer-review-button transfer-review-start',
          disabled:!draft.length, onClick:onStart, children:'开始转录'}),
      ]}),
    }),
  ]});
}

function transferPage(jsx, Dropdown, state) {
  if (state.review) return transferReview(jsx, Dropdown, state);
  const {jobs,onRetry,onResult}=state;
  const button=(text,click)=>jsx('button',{type:'button',onClick:click,children:text});
  const finished=jobs.filter(job=>job.stage==='completed').length;
  const failed=jobs.filter(job=>job.stage==='error').length;
  const done=jobs.length===finished+failed;
  const title=done?(failed?'部分任务需要处理':'转录完成'):'转录进度';
  return jsx('section',{className:'transfer-page',children:[
    jsx('header',{children:jsx('h2',{children:title})}),
    jsx('div',{className:'transfer-body',children:[
      jsx('p',{className:'transfer-meta',children:finished+' / '+jobs.length+' 个文件已完成'}),
      ...jobs.map(item=>{
        const {stage}=item;
        const active=['inferring','decoding'].includes(stage);
        return jsx('div',{className:'transfer-row',children:[
          jsx('strong',{children:item.name||item.fileName}),
          jsx('small',{role:'status',children:stage==='error'?item.error:stage==='completed'?'已完成':stage==='queued'?'等待处理':item.message||'正在提取音轨'}),
          active && transferProgress(jsx,item.fileName,item.measured?item.progress:undefined),
          stage==='error' && button('重试',()=>onRetry(item.id)),
          stage==='completed' && button('查看逐字稿',()=>onResult(item.recordId))
        ]},item.id||item.path);
      }),
    ]})
  ]});
}
