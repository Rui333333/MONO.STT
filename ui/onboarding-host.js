async function openOnboarding(settings, mountDropdown) {
  const store = window.MonoStore;
  if (store.getItem('mono_onboarding_v1')) return settings;

  return new Promise((resolve, reject) => {
    const frame = document.createElement('iframe');
    frame.title = 'MONO.STT 首次设置';
    frame.style.cssText = 'display:block;width:100%;height:100vh;border:0';
    window.onboardingBridge = {
      settings,
      mountDropdown,
      model: store.getItem('inkvoice_active_model_v1'),
      invoke: desktop.invoke,
      scan: modelLibrary.scan,
      pick: modelLibrary.pick,
      download: modelLibrary.download,
      reveal: startup.ready,
      setTheme(theme) {
        document.documentElement.classList.toggle('dark', theme === 'dark');
        document.documentElement.classList.toggle('light', theme === 'light');
      },
      async finish(config, model) {
        const saved = {...settings, ...config};
        await desktop.invoke('complete_onboarding', {settings:saved,model});
        store.kv.inkvoice_settings_v1 = JSON.stringify(saved);
        store.kv.inkvoice_active_model_v1 = model;
        store.kv.mono_onboarding_v1 = '1';
        frame.remove();
        delete window.onboardingBridge;
        resolve(saved);
      }
    };
    frame.src = 'onboarding.html';
    frame.onerror = () => reject(Error('无法打开首次设置页面'));
    document.getElementById('root').append(frame);
  });
}
