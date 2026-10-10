/** Keep the chat composer above mobile browser keyboards without blocking zoom. */
export function mobileViewport(node: HTMLElement) {
  const viewport = window.visualViewport;
  const media = window.matchMedia('(max-width:700px)');
  function update() {
    if (!media.matches || !viewport || viewport.scale !== 1) {
      node.style.removeProperty('--mobile-viewport-height');
      node.style.removeProperty('--mobile-viewport-top');
      return;
    }
    node.style.setProperty('--mobile-viewport-height', `${viewport.height}px`);
    node.style.setProperty('--mobile-viewport-top', `${viewport.offsetTop}px`);
  }
  viewport?.addEventListener('resize', update);
  viewport?.addEventListener('scroll', update);
  media.addEventListener('change', update);
  update();
  return {destroy() {
    viewport?.removeEventListener('resize', update);
    viewport?.removeEventListener('scroll', update);
    media.removeEventListener('change', update);
    node.style.removeProperty('--mobile-viewport-height');
    node.style.removeProperty('--mobile-viewport-top');
  }};
}
