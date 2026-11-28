const MonoUpdates = (() => {
  const {invoke, Channel} = window.__TAURI__.core;
  let checking;
  let versionInfo;
  let available;
  let installing = false;
  let busy = false;
  let dialog;

  let button;
  let started = false;
  let message = '';
  let percent = null;

  function renderButton() {
    if (!button) return;
    button.hidden = !available;
    button.disabled = installing || busy;
    button.setAttribute('aria-busy', String(installing));
    button.dataset.progress = !installing ? 'idle' : percent === null ? 'unknown' : 'known';
    button.style.setProperty('--update-progress', (percent ?? 0) + '%');
    const label = installing ? message : busy ? '转录队列结束后可更新' :
      '更新至 ' + (available?.version || '') + ' 并重启';
    button.title = label;
    button.setAttribute('aria-label', label);
  }

  function bindButton(node) {
    button = node;
    renderButton();
  }

  async function check() {
    if (checking) return checking;
    checking = (async () => {
      versionInfo = await invoke('app_update_info');
      available = versionInfo?.enabled ? await invoke('check_app_update') : null;
      renderButton();
      return available;
    })();
    try {
      return await checking;
    } finally {
      checking = null;
    }
  }

  function ensureDialog() {
    if (dialog) return;
    dialog = document.createElement('dialog');
    dialog.className = 'update-dialog';
    dialog.setAttribute('aria-labelledby', 'mono-update-title');
    dialog.innerHTML = '<h2 id="mono-update-title">软件更新</h2>' +
      '<p class="update-version"></p><p class="update-status" role="status" aria-live="polite"></p>' +
      '<div class="update-notes" hidden></div>' +
      '<progress aria-label="更新下载进度" max="100" hidden></progress>' +
      '<div class="update-actions"><button type="button" data-close autofocus>关闭</button>' +
      '<button type="button" data-install hidden>下载并安装</button></div>';
    dialog.querySelector('[data-close]').onclick = () => dialog.close();
    dialog.querySelector('[data-install]').onclick = install;
    dialog.oncancel = event => { if (installing) event.preventDefault(); };
    document.body.append(dialog);
  }

  function status(message) {
    dialog.querySelector('.update-status').textContent = message;
  }

  function showResult() {
    const version = dialog.querySelector('.update-version');
    version.textContent = versionInfo?.version ? '当前版本 ' + versionInfo.version : '';
    const notes = dialog.querySelector('.update-notes');
    notes.textContent = available?.notes || '';
    notes.hidden = !notes.textContent;
    dialog.querySelector('[data-install]').hidden = !available;
    dialog.querySelector('[data-close]').textContent = available ? '稍后' : '关闭';
    if (!versionInfo?.enabled) status('此构建未启用在线更新。');
    else if (!available) status('当前已是最新版本。');
    else status('发现新版本 ' + available.version + '。安装时应用会关闭。');
  }

  async function open() {
    ensureDialog();
    if (dialog.open) return;
    if (installing) { dialog.showModal(); return; }
    dialog.querySelector('[data-install]').hidden = true;
    dialog.querySelector('[data-close]').textContent = '关闭';
    dialog.querySelector('.update-notes').hidden = true;
    status('正在检查更新…');
    dialog.showModal();
    try {
      await check();
      showResult();
    } catch (error) {
      status(String(error));
    }
  }

  async function install() {
    if (!available || installing) return;
    ensureDialog();
    if (busy) {
      status('请等转录队列结束，再安装更新。');
      return;
    }
    const close = dialog.querySelector('[data-close]');
    const action = dialog.querySelector('[data-install]');
    const bar = dialog.querySelector('progress');
    installing = close.disabled = action.disabled = true;
    percent = null;
    bar.hidden = false;
    bar.removeAttribute('value');
    function report(text) {
      message = text;
      status(text);
      renderButton();
    }
    report('正在下载更新…');
    try {
      await window.MonoStore.queue;
      const progress = new Channel(event => {
        if (event.installing) {
          percent = bar.value = 100;
          report('正在验证更新，安装后将自动重启…');
        } else if (event.total > 0) {
          percent = bar.value = Math.min(100, Math.floor(event.downloaded * 100 / event.total));
          report('正在下载更新… ' + bar.value + '%');
        }
      });
      await invoke('install_app_update', {version: available.version, progress});
      report('正在安装并重启…');
    } catch (error) {
      showResult();
      status(String(error));
      if (!dialog.open) dialog.showModal();
      // Recheck before retrying if the release changed during the download.
      try { await check(); action.hidden = !available; } catch {}
    } finally {
      installing = close.disabled = action.disabled = false;
      bar.hidden = true;
      renderButton();
    }
  }

  function start() {
    if (started) return;
    started = true;
    const refresh = () => {
      if (!installing && !dialog?.open) check().catch(() => {});
    };
    setTimeout(refresh, 2500);
    setInterval(refresh, 4 * 60 * 60 * 1000);
    window.addEventListener('online', refresh);
  }

  return {open, start, install, bindButton, setBusy(value) { busy = value; renderButton(); }};
})();
