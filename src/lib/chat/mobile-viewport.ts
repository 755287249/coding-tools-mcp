/** Keep the chat composer above mobile browser keyboards without blocking zoom. */
export function mobileViewport(node: HTMLElement) {
  const viewport = window.visualViewport;
  const media = window.matchMedia('(max-width:700px)');
  let frame = 0;
  let height = '', top = '';
  function update() {
    frame = 0;
    if (!media.matches || !viewport || viewport.scale !== 1) {
      if (height) node.style.removeProperty('--mobile-viewport-height');
      if (top) node.style.removeProperty('--mobile-viewport-top');
      height = ''; top = '';
    } else {
      const nextHeight = `${viewport.height}px`, nextTop = `${viewport.offsetTop}px`;
      if (nextHeight !== height) node.style.setProperty('--mobile-viewport-height', nextHeight);
      if (nextTop !== top) node.style.setProperty('--mobile-viewport-top', nextTop);
      height = nextHeight; top = nextTop;
    }
  }
  // Keyboard and browser chrome can send several resize/scroll events per frame.
  function schedule() { if (!frame) frame = requestAnimationFrame(update); }
  viewport?.addEventListener('resize', schedule, {passive:true});
  viewport?.addEventListener('scroll', schedule, {passive:true});
  media.addEventListener('change', schedule);
  update();
  return {destroy() {
    cancelAnimationFrame(frame);
    viewport?.removeEventListener('resize', schedule);
    viewport?.removeEventListener('scroll', schedule);
    media.removeEventListener('change', schedule);
    node.style.removeProperty('--mobile-viewport-height');
    node.style.removeProperty('--mobile-viewport-top');
  }};
}
