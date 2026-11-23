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

function transferPage(jsx, Dropdown, state) {
  const {draft,jobs,review,model,onStart,onCancel,onRemove,onRetry,onResult,onLanguage,language}=state;
  const button=(text,click,primary=false)=>jsx('button',{type:'button',className:primary?'transfer-primary':'',onClick:click,children:text});
  const finished=jobs.filter(job=>job.stage==='completed').length;
  const failed=jobs.filter(job=>job.stage==='error').length;
  const done=jobs.length===finished+failed;
  const title=review?'确认转录':done?(failed?'部分任务需要处理':'转录完成'):'转录进度';
  return jsx('section',{className:'transfer-page',children:[
    jsx('header',{children:jsx('h2',{children:title})}),
    jsx('div',{className:'transfer-body',children:[
      jsx('p',{className:'transfer-meta',children:review?'待转录 · '+draft.length+' 个文件':finished+' / '+jobs.length+' 个文件已完成'}),
      review && jsx('div',{className:'transfer-settings',children:[
        jsx('span',{children:model?.name}),
        jsx('div',{className:'transfer-language',children:[jsx('span',{children:'语种'}),jsx(Dropdown,{label:'语种',value:language,onChange:onLanguage,items:[
          ['auto','自动识别'],['zh','中文'],['en','英语'],['ja','日语'],['zh-en','中英混合']
        ].map(([value,label])=>({value,label}))})]})
      ]}),
      ...(review?draft:jobs).map(item=>{
        const {stage}=item;
        const active=['inferring','decoding'].includes(stage);
        return jsx('div',{className:'transfer-row',children:[
          jsx('strong',{children:item.name||item.fileName}),
          review?jsx('small',{children:item.path}):jsx('small',{role:'status',children:stage==='error'?item.error:stage==='completed'?'已完成':stage==='queued'?'等待处理':item.message||'正在提取音轨'}),
          active && transferProgress(jsx,item.fileName,item.measured?item.progress:undefined),
          review && draft.length>1 && button('移除',()=>onRemove(item.path)),
          stage==='error' && button('重试',()=>onRetry(item.id)),
          stage==='completed' && button('查看逐字稿',()=>onResult(item.recordId))
        ]},item.id||item.path);
      }),
      review && jsx('div',{className:'transfer-actions',children:[button('取消',onCancel),button('开始转录',onStart,true)]})
    ]})
  ]});
}
