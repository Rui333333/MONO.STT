/* Reveal the next theme from the pointer or the keyboard-activated control. */
const themeTransition = (() => {
  let running = false;

  async function run(theme, event, commit) {
    const root = document.documentElement;
    if (running || root.classList.contains('dark') === (theme === 'dark')) return;
    if (!document.startViewTransition || !root.animate ||
        matchMedia('(prefers-reduced-motion: reduce)').matches) {
      commit();
      return;
    }

    const rect = event?.currentTarget?.getBoundingClientRect();
    const {innerWidth:width, innerHeight:height} = window;
    const pointer = event?.detail > 0;
    const x = Math.max(0, Math.min(width, pointer ? event.clientX : rect ? rect.left + rect.width / 2 : width / 2));
    const y = Math.max(0, Math.min(height, pointer ? event.clientY : rect ? rect.top + rect.height / 2 : height / 2));
    const radius = Math.ceil(Math.hypot(Math.max(x, width - x), Math.max(y, height - y)));
    const circles = [0, radius].map(size => 'circle(' + size + 'px at ' + x + 'px ' + y + 'px)');
    const expand = theme === 'dark';
    let committed = false;
    const apply = () => {
      if (committed) return;
      committed = true;
      commit();
    };

    running = true;
    root.dataset.themeTransition = expand ? 'expand' : 'contract';
    let transition, animation;
    try {
      transition = document.startViewTransition(apply);
      const finished = transition.finished.catch(() => {});
      await transition.ready;
      animation = root.animate({clipPath: expand ? circles : circles.reverse()}, {
        duration: 560,
        easing: 'cubic-bezier(.4,0,.2,1)',
        fill: 'forwards',
        pseudoElement: expand ? '::view-transition-new(root)' : '::view-transition-old(root)',
      });
      await animation.finished;
      await finished;
    } catch {
      apply();
      transition?.skipTransition();
      await transition?.finished.catch(() => {});
    } finally {
      animation?.cancel();
      delete root.dataset.themeTransition;
      running = false;
    }
  }

  return {run};
})();
