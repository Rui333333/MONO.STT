function createFeishuExport(React, jsx) {
  return function FeishuExport({recordId, title, exportText}) {
    const dialog = React.useRef(null);
    const [status, setStatus] = React.useState(null);
    const [settingsOpen, showSettings] = React.useState(false);
    const [appId, setAppId] = React.useState('');
    const [appSecret, setAppSecret] = React.useState('');
    const [folder, setFolder] = React.useState('');
    const [format, setFormat] = React.useState('text');
    const [recoveryLink, setRecoveryLink] = React.useState('');
    const [busy, setBusy] = React.useState('');
    const [error, setError] = React.useState('');
    const [notice, setNotice] = React.useState('');
    const call = desktop.invoke;

    async function readStatus() {
      const saved = await call('feishu_status', {recordId});
      setStatus(saved);
      return saved;
    }

    async function open() {
      setError('');
      setNotice('');
      setAppSecret('');
      setRecoveryLink('');
      dialog.current.showModal();
      setBusy('loading');
      try {
        const saved = await readStatus();
        setAppId(saved.appId);
        setFolder(saved.folder);
        setFormat(saved.format || 'text');
        showSettings(!saved.appId);
      } catch (failure) {
        setError(String(failure));
      } finally {
        setBusy('');
      }
    }

    async function save(event) {
      event.preventDefault();
      setError('');
      setNotice('');
      setBusy('save');
      try {
        await call('feishu_save_credentials', {appId, appSecret, folder});
        setAppSecret('');
        await readStatus();
        showSettings(false);
      } catch (failure) {
        setError(String(failure));
      } finally {
        setBusy('');
      }
    }

    async function publish(event) {
      event.preventDefault();
      setError('');
      setNotice('');
      setBusy('publish');
      try {
        const url = await call('feishu_publish', {
          recordId, title, format, content:exportText(format), recoveryLink:recoveryLink || null,
        });
        setStatus(current => ({...current, url, needsRecovery:false, format}));
        setRecoveryLink('');
        setNotice('飞书文档已写入。');
      } catch (failure) {
        setError(String(failure));
        try { await readStatus(); } catch { /* Preserve the publish error. */ }
      } finally {
        setBusy('');
      }
    }

    function field(label, value, onChange, props = {}) {
      return jsx('label', {className:'feishu-field', children:[
        jsx('span', {children:label}),
        jsx('input', {value, onChange:event => onChange(event.target.value), disabled:!!busy, ...props}),
      ]});
    }

    function action(label, props = {}) {
      return jsx('button', {type:'button', ...props, children:label});
    }

    return jsx(React.Fragment, {children:[
      action('飞书', {className:'feishu-export-trigger', onClick:open, title:'发布到飞书云文档'}),
      jsx('dialog', {
        ref:dialog, className:'feishu-dialog', 'aria-labelledby':'feishu-export-heading',
        onCancel:event => {
          event.stopPropagation();
          if (busy) event.preventDefault();
          else setAppSecret('');
        },
        onClose:() => setAppSecret(''),
        children:[
          jsx('header', {children:[
            jsx('h2', {id:'feishu-export-heading', children:'发布到飞书云文档'}),
            action('关闭', {onClick:() => dialog.current.close(), disabled:!!busy}),
          ]}),
          status && (settingsOpen ?
            jsx('form', {onSubmit:save, children:[
              jsx('p', {className:'feishu-hint', children:'使用已启用「创建及编辑新版文档」（docx:document）权限的飞书自建应用。'}),
              action('获取凭据 ↗', {
                className:'feishu-credentials-link', disabled:!!busy,
                onClick:async () => {
                  try { await call('open_feishu_credentials'); }
                  catch (failure) { setError(String(failure)); }
                },
              }),
              jsx('p', {className:'feishu-hint', children:'打开飞书开发者后台，选择自建应用，在「凭证与基础信息」中获取 App ID 和 App Secret。'}),
              field('App ID', appId, setAppId, {required:true, placeholder:'cli_…', autoComplete:'off'}),
              field('App Secret', appSecret, setAppSecret, {
                type:'password', required:!status.appId || appId.trim() !== status.appId,
                placeholder:status.appId && appId.trim() === status.appId ? '留空保留已保存的密钥' : '',
                autoComplete:'new-password',
              }),
              field('飞书目标文件夹（选填）', folder, setFolder, {placeholder:'文件夹链接或 token', autoComplete:'off'}),
              jsx('p', {className:'feishu-hint', children:'留空时写入应用云空间。若要用个人账号打开，请填写你可访问、且已授权给应用的目标文件夹。'}),
              jsx('div', {className:'feishu-actions', children:[
                status.appId && action('返回', {onClick:() => {setAppSecret(''); showSettings(false);}, disabled:!!busy}),
                action(busy === 'save' ? '验证中…' : '验证并保存', {type:'submit', className:'feishu-primary', disabled:!!busy}),
              ]}),
            ]}) :
            jsx('form', {onSubmit:publish, children:[
              jsx('p', {className:'feishu-title', children:title || '转录结果'}),
              jsx('fieldset', {disabled:!!busy, children:[
                jsx('legend', {children:'写入格式'}),
                ...[['text','无时间戳文字'],['srt','SRT 字幕']].map(([value,label]) =>
                  jsx('label', {className:'feishu-format', children:[
                    jsx('input', {type:'radio', name:'feishu-format', value, checked:format === value,
                      onChange:() => setFormat(value)}),
                    label,
                  ]}, value),
                ),
              ]}),
              status.url && jsx('p', {className:'feishu-hint', children:'再次写入会替换同一篇飞书文档的正文。'}),
              status.needsRecovery && jsx('div', {children:[
                jsx('p', {className:'feishu-hint', children:'上次创建结果未确认。请核对飞书中的文档，将链接填在下方以继续写入。'}),
                field('已创建的飞书文档链接', recoveryLink, setRecoveryLink, {required:true}),
              ]}),
              jsx('div', {className:'feishu-actions', children:[
                action('凭据设置', {onClick:() => {
                  setAppId(status.appId);
                  setFolder(status.folder);
                  setError('');
                  setNotice('');
                  showSettings(true);
                }, disabled:!!busy}),
                status.url && action('打开文档', {disabled:!!busy, onClick:async () => {
                  try { await call('feishu_open_document', {recordId}); }
                  catch (failure) { setError(String(failure)); }
                }}),
                action(busy === 'publish' ? '写入中…' : status.url ? '更新文档' : '创建并写入', {
                  type:'submit', className:'feishu-primary', disabled:!!busy,
                }),
              ]}),
            ]})),
          busy === 'loading' && jsx('p', {role:'status', children:'正在读取飞书设置…'}),
          error && jsx('p', {className:'feishu-feedback', role:'alert', children:error}),
          notice && jsx('p', {className:'feishu-feedback', role:'status', children:notice}),
        ],
      }),
    ]});
  };
}
