function confirmRecording(previewUrl, seconds, modelName) {
  return new Promise(resolve => {
    const dialog = document.createElement('dialog');
    dialog.className = 'record-confirm';
    dialog.setAttribute('aria-labelledby', 'record-confirm-title');
    dialog.innerHTML = '<h2 id="record-confirm-title">录音完成</h2><p class="record-details"></p><audio controls preload="metadata"></audio><div class="record-actions"><button type="button">舍弃录音</button><button type="button" autofocus>开始转录</button></div>';
    const duration = Math.max(1, Math.round(seconds));
    const elapsed = Math.floor(duration / 60) + ':' + String(duration % 60).padStart(2, '0');
    dialog.querySelector('.record-details').textContent = elapsed + '　·　' + modelName;
    const player = dialog.querySelector('audio');
    player.src = previewUrl;
    const [discard, confirm] = dialog.querySelectorAll('button');
    discard.onclick = () => dialog.close('discard');
    confirm.onclick = () => dialog.close('confirm');
    // Only the explicit discard action may throw away the finished recording.
    dialog.oncancel = event => event.preventDefault();
    dialog.onclose = () => {
      player.pause();
      player.removeAttribute('src');
      player.load();
      dialog.remove();
      resolve(dialog.returnValue === 'confirm');
    };
    document.body.append(dialog);
    dialog.showModal();
  });
}

function useNativeRecording(react, {model, onRecordingStateChange, onEnqueueFiles}) {
  const {invoke, convertFileSrc} = window.__TAURI__.core;
  const [recording, setRecording] = react.useState(false);
  const [busy, setBusy] = react.useState(false);
  const [seconds, setSeconds] = react.useState(0);
  const [error, setError] = react.useState(null);
  const [waveform, setWaveform] = react.useState(() => Array(32).fill(.12));
  const session = react.useRef(null);
  const changing = react.useRef(false);
  const mounted = react.useRef(true);
  const meterTimer = react.useRef(null);

  async function stop(keep = true, captureError = null) {
    const current = session.current;
    if (!current || changing.current) return;
    session.current = null;
    changing.current = true;
    clearTimeout(meterTimer.current);
    setBusy(true);
    try {
      const result = await invoke('stop_native_recording', {id: current.id, keep});
      if (!mounted.current) {
        if (keep) await invoke('discard_native_recording', {id: current.id});
        return;
      }
      setRecording(false);
      setSeconds(0);
      setWaveform(Array(32).fill(.12));
      onRecordingStateChange(false);
      if (captureError) setError(captureError);
      if (keep) {
        const confirmed = await confirmRecording(convertFileSrc(result.path), result.duration, current.modelName);
        if (confirmed) {
          await onEnqueueFiles([{path: result.path, name: current.name}]);
        } else {
          await invoke('discard_native_recording', {id: current.id});
        }
      }
    } catch (cause) {
      if (mounted.current) setError(String(captureError || cause));
    } finally {
      changing.current = false;
      if (mounted.current) {
        setRecording(false);
        setBusy(false);
        setSeconds(0);
        setWaveform(Array(32).fill(.12));
        onRecordingStateChange(false);
      }
    }
  }

  function scheduleMeter(current) {
    meterTimer.current = setTimeout(async () => {
      try {
        const peak = await invoke('native_recording_level', {id: current.id});
        if (session.current !== current) return;
        setSeconds(Math.floor((performance.now() - current.started) / 1000));
        setWaveform(previous => [...previous.slice(1), Math.max(.12, Math.min(1, peak * 2))]);
        scheduleMeter(current);
      } catch (cause) {
        if (session.current === current) await stop(false, String(cause));
      }
    }, 120);
  }

  async function start() {
    if (changing.current || session.current) return;
    setError(null);
    if (!model) {
      setError('尚未下载本地识别模型，请先打开模型库下载。');
      return;
    }
    changing.current = true;
    setBusy(true);
    onRecordingStateChange(true);
    try {
      const id = await invoke('start_native_recording');
      if (!mounted.current) {
        await invoke('stop_native_recording', {id, keep: false});
        return;
      }
      const now = new Date();
      const pad = value => String(value).padStart(2, '0');
      const date = now.getFullYear() + pad(now.getMonth() + 1) + pad(now.getDate());
      const time = pad(now.getHours()) + pad(now.getMinutes()) + pad(now.getSeconds());
      session.current = {id, name: `录音_${date}_${time}.wav`, modelName: model.name, started: performance.now()};
      setRecording(true);
      scheduleMeter(session.current);
    } catch (cause) {
      if (mounted.current) {
        setError(String(cause));
        onRecordingStateChange(false);
      }
    } finally {
      changing.current = false;
      if (mounted.current) setBusy(false);
    }
  }

  react.useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      clearTimeout(meterTimer.current);
      const current = session.current;
      session.current = null;
      if (current) invoke('stop_native_recording', {id: current.id, keep: false}).catch(console.error);
      onRecordingStateChange(false);
    };
  }, []);

  return {recording, busy, seconds, error, waveform, start, stop: () => stop()};
}
