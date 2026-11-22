function confirmRecording(file, seconds, modelName) {
  return new Promise(resolve => {
    const dialog = document.createElement('dialog');
    dialog.className = 'record-confirm';
    dialog.setAttribute('aria-labelledby', 'record-confirm-title');
    dialog.innerHTML = '<h2 id="record-confirm-title">录音完成</h2><p class="record-details"></p><audio controls preload="metadata"></audio><div class="record-actions"><button type="button">舍弃录音</button><button type="button" autofocus>开始转录</button></div>';
    const duration = Math.max(1, Math.round(seconds));
    const elapsed = Math.floor(duration / 60) + ':' + String(duration % 60).padStart(2, '0');
    dialog.querySelector('.record-details').textContent = elapsed + '　·　' + modelName;
    const player = dialog.querySelector('audio');
    const preview = URL.createObjectURL(file);
    player.src = preview;
    const [discard, confirm] = dialog.querySelectorAll('button');
    discard.onclick = () => dialog.close('discard');
    confirm.onclick = () => dialog.close('confirm');
    // Only the explicit discard action may throw away the finished recording.
    dialog.oncancel = event => event.preventDefault();
    dialog.onclose = () => {
      player.pause();
      dialog.remove();
      URL.revokeObjectURL(preview);
      resolve(dialog.returnValue === 'confirm');
    };
    document.body.append(dialog);
    dialog.showModal();
  });
}
