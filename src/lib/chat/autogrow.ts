/** Resize after input, draft restoration, mentions, mode changes and width changes. */
export function autoGrow(node: HTMLTextAreaElement, _value: unknown) {
  let frame = 0;
  let width = 0;
  const resize = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      const top = node.scrollTop;
      node.style.height = '0px';
      node.style.height = `${node.scrollHeight}px`;
      node.scrollTop = top;
    });
  };
  const observer = new ResizeObserver(entries => {
    const next = entries[0]?.contentRect.width ?? 0;
    if (next !== width) { width = next; resize(); }
  });
  observer.observe(node);
  resize();
  return { update: resize, destroy() { observer.disconnect(); cancelAnimationFrame(frame); } };
}
export function messageBytes(text: string): number { return new TextEncoder().encode(text).length; }
