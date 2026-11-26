const monoMacOS = /Macintosh/.test(navigator.userAgent);
document.documentElement.classList.toggle('mono-macos', monoMacOS);

function monoWindowControls(React, jsx) {
  if (monoMacOS) return null;
  const win = window.__TAURI__.window.getCurrentWindow();
  const [maximized, setMaximized] = React.useState(false);
  React.useEffect(() => {
    let active = true;
    const refresh = async () => {
      const value = await win.isMaximized();
      if (active) setMaximized(value);
    };
    const unsubscribe = win.onResized(refresh);
    refresh();
    return () => { active = false; unsubscribe.then(stop => stop()); };
  }, []);
  const actions = [
    ['min', '最小化', () => win.minimize()],
    [maximized ? 'restore' : 'max', maximized ? '还原' : '最大化', () => win.toggleMaximize()],
    ['close', '关闭', () => win.close()]
  ];
  return jsx('div', {className:'mono-window-controls', children:actions.map(([icon,label,action]) =>
    jsx('button', {type:'button',className:'mono-control '+icon,title:label,'aria-label':label,
      onClick:()=>action().catch(err=>console.error(label,err)),children:jsx('i',{'aria-hidden':true})},icon))});
}
