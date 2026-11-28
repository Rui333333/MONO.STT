
const ICONS = {
  check: `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>`,
  refresh: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>`,
  sun: `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>`,
  moon: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/></svg>`,
  arrow: `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12"/><polyline points="12 5 19 12 12 19"/></svg>`
};

const STAGES = ['运行设备', '语音模型', '试转录', '完成'];

const HEADERS = [
  { title: '选择运行设备', desc: '决定本地语音转录所使用的计算后端，后续可在偏好设置中随时更改。' },
  { title: '选择语音模型', desc: '下载所需模型权重，或直接关联本机已有的模型目录。' },
  { title: '验证本地转录', desc: '使用一段 内置普通话音频，确认硬件与模型可以正常协同工作。' },
  { title: '配置已完成', desc: '所有环境检查均已通过，点击下方按钮即可进入 MONO.STT。' }
];

const MODELS = [
  {
    id: 'whisper-large-v3-turbo',
    name: 'Whisper large-v3-turbo',
    size: '1.62 GB',
    rec: true,
    desc: '速度与精度均衡，适合中英文日常转录',
  },
  {
    id: 'whisper-large-v3',
    name: 'Whisper large-v3',
    size: '3.09 GB',
    rec: false,
    desc: '完整规模，多语种与复杂口音识别率最高',
  }
];

const bridge = parent.onboardingBridge;
const macOS = /Macintosh/.test(navigator.userAgent);
const supported = new Map([
  ['whisper-large-v3-turbo', 'faster-whisper-large-v3-turbo'],
  ['whisper-large-v3', 'faster-whisper-large-v3'],
]);
const escapeText = value => String(value ?? '').replace(/[&<>"']/g, char =>
  ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'})[char]);
const state = {
  step: 0,
  device: macOS || bridge.settings.computeDevice === 'cpu' ? 'CPU' : 'GPU',
  model: [...supported].find(([,id])=>id===bridge.model)?.[0] || 'whisper-large-v3-turbo',
  readyModels: {},
  tested: false,
  busy: false,
  busyAction: '',
  progress: 0,
  note: '',
  theme: bridge.settings.theme === 'dark' ? 'dark' : 'light',
  hardware: null, modelsDir: bridge.settings.modelsDir || '', defaultDir: '',
  source: bridge.settings.mirrorSource || 'hf-mirror', bytes: 0, verifying: false, result: null
};

const $ = id => document.getElementById(id);
const currentModel = () => MODELS.find(m => m.id === state.model) || MODELS[0];
const isModelReady = (id = state.model) => Boolean(state.readyModels[id]);

function viewDevice() {
  const gpu = state.hardware?.adapters?.find(item => /nvidia/i.test(item.name)) || state.hardware?.adapters?.[0];
  const detecting = state.busy && state.busyAction === 'detect';
  return `
    <div class="info-strip">
      <div class="info-strip-main">
        <span class="dot-ok" aria-hidden="true"></span>
        <div>
          <span class="info-strip-title">${escapeText(gpu?.name || "本机运行设备")}</span>
          <span class="info-strip-sub">${gpu?.memoryMiB ? (gpu.memoryMiB / 1024).toFixed(1) + " GB · " : ""}${detecting ? "正在检测" : state.hardware?.cudaAvailable ? "CUDA 可用" : "CPU 模式可选"}</span>
        </div>
      </div>
      <button type="button" class="btn btn-ghost" data-action="detect" ${state.busy ? 'disabled' : ''}>
        <span class="${detecting ? 'spin' : ''}" style="display:inline-flex">${ICONS.refresh}</span>
        ${detecting ? '检测中' : '重新检测'}
      </button>
    </div>

    <div class="tile-grid" role="radiogroup" aria-label="转录设备">
      ${macOS ? '' : `<button type="button" class="tile" data-action="device-GPU" aria-pressed="${state.device === 'GPU'}" ${(state.busy || !state.hardware?.cudaAvailable) ? 'disabled' : ''}>
        <div>
          <div class="tile-top">
            <span class="tile-title">GPU 加速</span>
            <span class="radio-dot" aria-hidden="true"></span>
          </div>
          <div class="tile-desc">使用 NVIDIA CUDA 核心加速推理，长音频处理速度最快。</div>
        </div>
        <div class="tile-meta">
          <span>CUDA</span>
          <strong>推荐</strong>
        </div>
      </button>`}

      <button type="button" class="tile" data-action="device-CPU" aria-pressed="${state.device === 'CPU'}" ${state.busy ? 'disabled' : ''}>
        <div>
          <div class="tile-top">
            <span class="tile-title">CPU 模式</span>
            <span class="radio-dot" aria-hidden="true"></span>
          </div>
          <div class="tile-desc">纯处理器运行，不占用显存，适合后台低负载或无独显环境。</div>
        </div>
        <div class="tile-meta">
          <span>通用兼容</span>
          <span>0 显存占用</span>
        </div>
      </button>
    </div>
    ${state.hardware?.reason && gpu && /nvidia/i.test(gpu.name) ? `<div class="onboarding-path">${escapeText(state.hardware.reason)} <button class="btn btn-ghost" data-action="cuda">打开 CUDA 下载页</button></div>` : ''}
  `;
}

function viewModel() {
  const selected = currentModel();
  const ready = isModelReady(selected.id);
  const busyModel = state.busy && (state.busyAction === 'download' || state.busyAction === 'local');

  let barLeft = '';
  if (busyModel) {
    const status = state.busyAction === 'local' ? '正在校验本地模型' :
      state.verifying ? '正在校验模型文件完整性' : '正在下载当前文件 · ' + (state.bytes / 1048576).toFixed(1) + ' MB';
    barLeft = `<span>${status}</span>`;
  } else if (ready) {
    barLeft = `<span class="ready-flag"><span class="dot-ok"></span>${selected.name} 已校验就绪</span>`;
  } else {
    barLeft = `<span>${selected.size} · 尚未配置</span>`;
  }

  return `
    <div class="list-box" role="radiogroup" aria-label="语音模型列表">
      ${MODELS.map(m => {
        const active = state.model === m.id;
        const mReady = isModelReady(m.id);
        return `
          <button type="button" class="model-row" data-action="model-${m.id}" aria-pressed="${active}" ${state.busy ? 'disabled' : ''}>
            <div class="model-row-left">
              <span class="radio-dot" aria-hidden="true"></span>
              <div class="model-row-info">
                <div class="model-row-name">
                  <span>${m.name}</span>
                  ${m.rec ? `<span class="tag-rec">推荐</span>` : ''}
                </div>
                <div class="model-row-desc">${m.desc}</div>
              </div>
            </div>
            <div class="model-row-right">
              ${mReady ? `<span class="ready-flag"><span class="dot-ok"></span>已就绪</span>` : ''}
              <span>${m.size}</span>
            </div>
          </button>
        `;
      }).join('')}
    </div>

    <div class="setup-options">
      <span class="onboarding-path">${escapeText(state.modelsDir || state.defaultDir)}</span>
      <div class="setup-source"><span>下载源</span><div id="setup-source"></div></div>
    </div>
    <div class="model-bar">
      <div class="model-bar-status">${barLeft}</div>
      <div class="btn-group">
        <button type="button" class="btn btn-outline" data-action="local" ${state.busy ? 'disabled' : ''}>导入本地</button>
        <button type="button" class="btn btn-solid" data-action="download" ${(state.busy || ready) ? 'disabled' : ''}>
          ${ready ? '已下载' : '下载模型'}
        </button>
      </div>
    </div>
  `;
}

const sampleAudio = new Audio('sample_zh.wav');
sampleAudio.preload = 'metadata';

function syncSamplePlayer() {
  const button = document.querySelector('[data-action="preview-sample"]');
  const time = $('sample-time');
  if (!button || !time) return;
  button.textContent = sampleAudio.paused ? '播放示例' : '暂停示例';
  time.textContent = '00:' + String(Math.floor(sampleAudio.currentTime)).padStart(2,'0');
}
for (const event of ['play','pause','ended','timeupdate']) sampleAudio.addEventListener(event,syncSamplePlayer);

function viewTest() {
  const testing = state.busy && state.busyAction === 'test';
  const bars = [14, 24, 38, 28, 16, 12, 32, 42, 36, 20, 28, 40, 24, 14, 10, 22, 38, 42, 30, 16, 24, 34, 18, 12, 26, 36, 22, 14, 28, 18];
  const waveCls = testing ? 'wave is-active' : (state.tested ? 'wave is-done' : 'wave');

  return `
    <div class="audio-stage">
      <div class="audio-top">
        <div>
          <div class="info-strip-title">sample_zh.wav</div>
          <div class="audio-meta">00:07 · 16 kHz · 普通话示例</div>
        </div>
        <div class="btn-group">
          <span class="audio-meta" id="sample-time">00:00</span>
          <button type="button" class="btn btn-outline" data-action="preview-sample">播放示例</button>
          <button type="button" class="btn btn-solid" data-action="test" ${state.busy ? 'disabled' : ''}>
          ${testing ? '正在转录…' : (state.tested ? '重新试转录' : '运行试转录')}
          </button>
        </div>
      </div>
      <div class="${waveCls}" aria-hidden="true">
        ${bars.map((h, i) => `<i style="height:${h}px; animation-delay:${(i % 7) * 0.07}s"></i>`).join('')}
      </div>
    </div>

    ${state.tested ? `
      <div class="transcript">
        <div class="transcript-meta">
          <span>${currentModel().name} · ${state.device}</span>
          <span>耗时 ${state.result.elapsed.toFixed(2)}s</span>
        </div>
        ${state.result.segments.map(segment => `<div class="transcript-line">
          <span class="ts">00:${segment.start.toFixed(2).padStart(5,'0')}</span>
          <span>${escapeText(segment.text)}</span>
        </div>`).join('')}
      </div>
    ` : ''}
  `;
}

function viewDone() {
  return `
    <div class="spec-table">
      <div class="spec-row">
        <span class="spec-key">运行设备</span>
        <span class="spec-val"><span class="dot-ok"></span>${state.device === 'GPU' ? escapeText(state.hardware?.adapters?.find(x=>/nvidia/i.test(x.name))?.name || 'NVIDIA GPU 加速') : '本机处理器 (CPU 模式)'}</span>
      </div>
      <div class="spec-row">
        <span class="spec-key">语音模型</span>
        <span class="spec-val"><span class="dot-ok"></span>${currentModel().name}</span>
      </div>
      <div class="spec-row">
        <span class="spec-key">试转录验证</span>
        <span class="spec-val"><span class="dot-ok"></span>已通过 (${state.result.elapsed.toFixed(2)}s)</span>
      </div>
    </div>
  `;
}

function updateTheme() {
  document.documentElement.setAttribute('data-theme', state.theme);
  document.documentElement.classList.toggle('dark', state.theme === 'dark');
  bridge.setTheme(state.theme);
  $('themeToggle').innerHTML = state.theme === 'light' ? ICONS.moon : ICONS.sun;
}

let sourceDropdown;
function render() {
  sourceDropdown?.unmount();
  sourceDropdown = null;
  updateTheme();

  $('steps').innerHTML = STAGES.map((name, i) => {
    const active = state.step === i;
    const done = i < state.step;
    return `
      <li class="${done ? 'is-done' : ''}" ${active ? 'aria-current="step"' : ''}>
        <span class="step-num" aria-hidden="true">${done ? ICONS.check : `0${i + 1}`}</span>
        <span>${name}</span>
      </li>
    `;
  }).join('');

  const views = [viewDevice, viewModel, viewTest, viewDone];
  const h = HEADERS[state.step];

  $('content').innerHTML = `
    <div class="step-panel">
      <div class="header">
        <h1>${h.title}</h1>
        <p class="desc">${h.desc}</p>
      </div>
      ${views[state.step]()}
      <div class="notice" role="status">${escapeText(state.note)}</div>
    </div>
  `;

  syncSamplePlayer();
  const source = $('setup-source');
  if (source) {
    sourceDropdown = bridge.mountDropdown(source);
    const refresh = () => sourceDropdown.update({
      label:'下载源', value:state.source, align:'end', minWidth:164, disabled:state.busy,
      items:[{value:'hf-mirror',label:'HF 镜像'},{value:'huggingface',label:'Hugging Face'}],
      onChange:value => { state.source = value; refresh(); }
    });
    refresh();
  }

  const ready = isModelReady();
  const blocked = state.busy || (state.step === 1 && !ready) || (state.step >= 2 && !state.tested);
  const hint = state.step === 1 && !ready
    ? '需先下载或导入所选模型'
    : (state.step === 2 && !state.tested ? '试转录通过后可继续' : '');

  const backBtn = state.step > 0
    ? `<button type="button" class="btn btn-ghost" data-action="back" ${state.busy ? 'disabled' : ''}>上一步</button>`
    : '';
  const nextLabel = state.step === 3 ? '进入 MONO.STT' : '继续';

  $('actions').innerHTML = `
    <span class="footer-hint">${hint}</span>
    <div class="footer-actions">
      ${backBtn}
      <button type="button" class="btn btn-solid" data-action="next" ${blocked ? 'disabled' : ''}>
        ${nextLabel} ${ICONS.arrow}
      </button>
    </div>
  `;
}


async function perform(action, note, operation) {
  state.busy = true;
  state.busyAction = action;
  state.note = note;
  render();
  try { await operation(); }
  catch (error) { state.note = String(error); }
  finally {
    state.busy = false;
    state.busyAction = '';
    render();
  }
}

async function scanDirectory(directory) {
  const installed = await bridge.scan(directory);
  state.modelsDir = directory;
  state.readyModels = Object.fromEntries([...supported].map(([name,id]) => [name, installed.includes(id)]));
}

async function detect() {
  await perform('detect', '正在检测设备…', async () => {
    state.hardware = await bridge.invoke('gpu_status');
    if (!state.hardware.cudaAvailable) state.device = 'CPU';
    state.tested = false;
    state.note = state.hardware.cudaAvailable ? 'CUDA 可用，也可以选择 CPU。' : '可选择 CPU，然后通过试转录确认。';
  });
}

$('themeToggle').addEventListener('click', event => {
  const theme = state.theme === 'light' ? 'dark' : 'light';
  themeTransition.run(theme, event, () => { state.theme = theme; updateTheme(); });
});

$('window').addEventListener('click', async event => {
  const button = event.target.closest('[data-action]');
  if (!button || button.disabled) return;
  const action = button.dataset.action;
  if (action === 'preview-sample') {
    if (!sampleAudio.paused) { sampleAudio.pause(); return; }
    try { await sampleAudio.play(); }
    catch { state.note = '无法播放示例音频，请重试。'; render(); }
    return;
  }
  if (state.busy) return;
  if (state.step === 2 && ['next','back'].includes(action)) sampleAudio.pause();
  state.note = '';
  const model = supported.get(state.model);
  const options = () => ({
    modelsDir:state.modelsDir, computeDevice:state.device === 'GPU' ? 'cuda' : 'cpu',
    cpuThreads:bridge.settings.cpuThreads, language:'zh', vadFilter:false,
  });

  if (action === 'detect') return detect();
  if (action === 'cuda') return perform('cuda', '', () => bridge.invoke('open_cuda_download'));
  if (action === 'local') return perform('local', '正在选择已有模型…', async () => {
    const folder = await bridge.pick(state.modelsDir);
    if (folder) {
      const suffix = /[\\/](faster-whisper-large-v3(?:-turbo)?)[\\/]?$/i;
      await scanDirectory(folder.replace(suffix,''));
      state.tested = false;
      state.note = isModelReady() ? '已找到所选模型。' : '该目录没有通过校验的所选模型，请检查文件是否与内置版本一致。';
    } else state.note = '';
  });
  if (action === 'download') return perform('download', '正在下载模型…', async () => {
    state.bytes = 0;
    state.verifying = false;
    state.tested = false;
    const ok = await bridge.download({model,modelsDir:state.modelsDir,source:state.source},
      progress => {
        state.verifying = progress.phase === 'verifying';
        if (Number.isFinite(progress.bytes)) state.bytes = progress.bytes;
        render();
      });
    await scanDirectory(state.modelsDir);
    state.note = ok && isModelReady() ? '模型下载完成。' : '模型下载未完成。';
  });
  if (action === 'test') return perform('test', '正在运行示例音频转录…', async () => {
    state.tested = false;
    state.result = await bridge.invoke('onboarding_sample',{model,options:options()});
    state.tested = true;
    state.note = '试转录通过。';
  });
  if (action === 'next') {
    if (state.step === 3 && state.tested && isModelReady()) {
      return perform('finish','正在保存配置…',() => bridge.finish({
        theme:state.theme, modelsDir:state.modelsDir, modelDirectoryVersion:1,
        computeDevice:options().computeDevice, mirrorSource:state.source,
      },model));
    }
    if (state.step === 1 && !isModelReady() || state.step === 2 && !state.tested) return;
    if (state.step < 3) state.step++;
  } else if (action === 'back') state.step--;
  else if (action.startsWith('device-')) {
    state.tested = false;
    state.device = action.slice(7);
  } else if (action.startsWith('model-')) {
    state.tested = false;
    state.model = action.slice(6);
  }
  render();
});

render();
bridge.reveal();
(async () => {
  await detect();
  try {
    state.defaultDir = await bridge.invoke('default_models_dir');
    await scanDirectory(state.modelsDir);
  } catch (error) {
    state.note = String(error);
  }
  render();
})();
