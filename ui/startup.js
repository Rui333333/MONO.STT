function startupFrame() {
  return new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
}

const startup = (() => {
  const panel = document.querySelector('.startup');
  const root = document.querySelector('#root');
  const core = window.__TAURI__?.core;
  let shownAt;
  let finish;
  const finished = new Promise(resolve => { finish = resolve; });
  // A hidden WebView may throttle animation frames until the window is shown.
  const visible = Promise.resolve(core?.invoke('show_startup_window')).then(() => {
    shownAt = performance.now();
  });
  visible.catch(console.error);

  let revealing;
  const ready = () => revealing ??= reveal();

  async function reveal() {
    await visible;
    await startupFrame();
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const timeLeft = reduced ? 0 : Math.max(0, 450 - (performance.now() - shownAt));
    if (timeLeft) await new Promise(resolve => setTimeout(resolve, timeLeft));
    panel.classList.add('startup-leaving');
    if (!reduced) await new Promise(resolve => setTimeout(resolve, 230));
    panel.remove();
    root.inert = false;
    finish();
  }

  function fail(error) {
    console.error(error);
    panel.classList.add('startup-failed');
    panel.querySelector('[role="status"]').textContent = '启动失败';
    panel.querySelector('.startup-error').textContent = String(error);
    panel.querySelector('button').hidden = false;
    panel.querySelector('button').onclick = () => location.reload();
  }

  return {ready,finished,fail};
})();
