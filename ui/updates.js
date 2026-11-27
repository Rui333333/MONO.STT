const MonoUpdates = (() => {
  const {invoke, Channel} = window.__TAURI__.core;
  let checking;
  let versionInfo;
  let available;
  let installing = false;
  let busy = false;
  let dialog;

  async function check() {
    if (checking) return checking;
    checking = (async () => {
      versionInfo = await invoke('app_update_info');
      available = versionInfo?.enabled ? await invoke('check_app_update') : null;
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
    if (busy) {
      status('请等转录队列结束，再安装更新。');
      return;
    }
    const close = dialog.querySelector('[data-close]');
    const action = dialog.querySelector('[data-install]');
    const bar = dialog.querySelector('progress');
    installing = close.disabled = action.disabled = true;
    bar.hidden = false;
    bar.removeAttribute('value');
    status('正在下载更新…');
    try {
      await window.MonoStore.queue;
      const progress = new Channel(event => {
        if (event.installing) {
          status('正在验证更新并启动安装程序…');
        } else if (event.total > 0) {
          bar.value = Math.min(100, Math.floor(event.downloaded * 100 / event.total));
          status('正在下载更新… ' + bar.value + '%');
        }
      });
      await invoke('install_app_update', {version: available.version, progress});
      status('安装程序已启动。');
    } catch (error) {
      status(String(error));
    } finally {
      installing = close.disabled = action.disabled = false;
      bar.hidden = true;
    }
  }

  function start() {
    setTimeout(async () => {
      try {
        if (!await check() || busy || document.querySelector('dialog[open], [aria-modal="true"]')) return;
        ensureDialog();
        showResult();
        dialog.showModal();
      } catch {
        // Connectivity errors are reported when the user checks manually.
      }
    }, 2500);
  }

  return {open, start, setBusy: value => { busy = value; }};
})();
