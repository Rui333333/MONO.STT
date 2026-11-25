const {invoke, Channel} = window.__TAURI__.core;
const element = id => document.getElementById(id);
const activeArea = element('installer');
const artwork = activeArea.querySelector('.art');
const preflight = element('preflight');
const action = element('action');
const path = element('path');
const phase = element('phase');
const title = element('headline');
const subtitle = element('subtitle');
let info;
let busy = false;
let stage = 'loading';
let elevationDirectory = null;

function show(next) {
  stage = next;
  artwork.dataset.state = next;
  const small = ['loading','confirm','cancelled','uninstall'].includes(next);
  preflight.hidden = !small;
  activeArea.hidden = small;
  for (const [panel, route] of [['loader','loading'],['confirm','confirm'],['cancelled','cancelled'],['uninstall','uninstall']]) {
    element(panel).hidden = next !== route;
  }
  const installing = next === 'progress';
  for (const close of document.querySelectorAll('[data-close]')) close.disabled = installing;
  element('directory').hidden = next !== 'start';
  element('progress').hidden = !installing;
  action.hidden = element('footer-note').hidden = installing;
  if (!small) {
    const complete = next === 'complete';
    title.textContent = installing ? '正在安装 MONO.STT' : complete ? '安装完成' : '安装 MONO.STT';
    subtitle.textContent = installing ? '正在将应用和本地转录所需组件安装到电脑。' :
      complete ? 'MONO.STT 已准备就绪。' : '本地音视频转文字';
    action.textContent = complete ? '打开 MONO.STT' : '开始安装';
    element('footer-note').textContent = complete ? '可以开始转录音频或视频' : '为当前 Windows 用户安装';
    title.focus({preventScroll:true});
  }
  const size = next === 'cancelled' ? 'confirm' : next;
  return invoke('set_page',{page:size});
}

function percent(group, value) {
  const loading = group === 'load';
  const bar = element(loading ? 'load-progress' : 'progress-line');
  const fill = element(loading ? 'load-fill' : 'fill');
  const text = element(loading ? 'load-percent' : 'percent');
  if (value == null) {
    bar.removeAttribute('aria-valuenow');
    fill.style.width = '0%';
    text.textContent = '';
  } else {
    bar.setAttribute('aria-valuenow',value);
    fill.style.width = value + '%';
    text.textContent = value + '%';
  }
}

function fail(error, directory = null, chooseAgain = false) {
  const needsElevation = error === 'ELEVATION_REQUIRED';
  elevationDirectory = directory;
  element('elevate').hidden = !needsElevation;
  element('choose-again').hidden = !chooseAgain;
  element('load-error').textContent = needsElevation ?
    '这个安装位置需要管理员权限。请用当前 Windows 账号授权。' : String(error);
  element('cancelled-title').textContent = '安装未完成';
  return show('cancelled');
}

function installationDirectory() {
  const location = path.value.trim();
  if (location === info?.default_directory || !/[\\/]/.test(location)) return location;
  const directory = location.replace(/[\\/]+$/, '');
  return /[\\/]MONO\.STT$/i.test(directory) ? directory : directory + '\\MONO.STT';
}

async function runInstall(overwrite) {
  if (busy) return;
  const directory = overwrite ? null : installationDirectory();
  busy = true;
  try {
    await show('progress');
    phase.textContent = '正在确认安装位置…';
    percent('install',0);
    let lastPercent = 0;
    const channel = new Channel(({phase:step,percent:value}) => {
      // Some stages report status only. Keep the amount already completed.
      if (Number.isFinite(value) && value >= lastPercent) {
        lastPercent = value;
        percent('install',lastPercent);
      }
      phase.textContent = ({
        copy:'正在复制应用和转录组件…',
        close:'正在关闭已运行的 MONO.STT…',
        shortcuts:'正在注册应用和创建快捷方式…',
        complete:'安装完成',
      })[step] || '正在安装…';
    });
    await invoke('install',{
      directory,
      desktop: element(overwrite ? 'overwrite-desktop' : 'fresh-desktop').checked,
      overwrite, progress:channel,
    });
    await show('complete');
  } catch(error) {
    await fail(error, directory, !overwrite && !info?.automatic);
  } finally {
    busy = false;
  }
}

async function prepare() {
  try {
    const uninstallPath = await invoke('uninstall_details');
    if (uninstallPath) {
      element('uninstall-path').textContent = uninstallPath;
      await show('uninstall');
      await waitForFirstPaint(element('uninstall').querySelector('img'));
      await invoke('reveal_loading');
      element('cancel-uninstall').focus();
      return;
    }
    await show('loading');
    await waitForFirstPaint(element('loader').querySelector('img'));
    await invoke('reveal_loading');
    const channel = new Channel(event => percent('load',event.percent));
    info = await invoke('prepare',{progress:channel});
    path.value = info.default_directory;
    if (info.automatic) {
      await runInstall(Boolean(info.installed));
    } else if (info.installed) {
      element('versions').textContent = (info.installed.version || '已安装版本') + ' → ' + info.version;
      const detail = element('versions').nextElementSibling;
      detail.textContent = '沿用原安装目录，保留转录记录、设置和模型。正在运行的 MONO.STT 会关闭。';
      await show('confirm');
    } else {
      await show('start');
    }
  } catch(error) {
    await fail(error);
    await invoke('reveal_loading');
  }
}

async function waitForFirstPaint(image) {
  await Promise.race([
    image.decode().catch(() => {}),
    new Promise(resolve => setTimeout(resolve, 600)),
  ]);
  // A hidden WebView can throttle animation callbacks.
  await Promise.race([
    new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    new Promise(resolve => setTimeout(resolve, 160)),
  ]);
}

async function runUninstall() {
  if (busy) return;
  busy = true;
  const remove = element('remove-app');
  const cancel = element('cancel-uninstall');
  const dismiss = element('uninstall').querySelector('[data-close]');
  const error = element('uninstall-error');
  for (const button of [remove, cancel, dismiss]) button.disabled = true;
  error.hidden = true;
  remove.textContent = '正在卸载…';
  try {
    await invoke('uninstall_app');
    element('uninstall-title').textContent = '卸载完成';
    element('uninstall-detail').textContent = '转录记录、设置和模型已保留。';
    cancel.hidden = true;
    remove.textContent = '完成';
    remove.onclick = () => close();
  } catch (failure) {
    error.textContent = String(failure);
    error.hidden = false;
    remove.textContent = '重试卸载';
  } finally {
    busy = false;
    for (const button of [remove, cancel, dismiss]) button.disabled = false;
    remove.focus();
  }
}

function close(launch = false) {
  if (!busy) invoke('finish',{launch}).catch(fail);
}
for(const button of document.querySelectorAll('[data-close]')) button.onclick = () => close();
element('cancel-overwrite').onclick = () => close();
element('cancel-uninstall').onclick = () => close();
element('remove-app').onclick = runUninstall;
element('choose-again').onclick = async () => {
  await show('start');
  element('directory').open = true;
  path.focus();
};
element('overwrite').onclick = () => runInstall(true);
element('elevate').onclick = async () => {
  try {
    await invoke('elevate',{directory:elevationDirectory});
  } catch(error) {
    element('load-error').textContent = String(error);
  }
};
element('browse').onclick = async () => {
  try {
    const chosen = await invoke('choose_directory');
    if (chosen) path.value = chosen;
  } catch(error) {
    await fail(error);
  }
};
action.onclick = () => stage === 'complete' ? close(true) : runInstall(false);
prepare();
