type DropOptions = {
  enabled: () => boolean;
  onFiles: (files: File[]) => void | Promise<void>;
  onActive: (active: boolean) => void;
  onError: (reason: 'directory' | 'upload') => void;
};

/** HTML file drops use the same bounded uploader as the file picker. */
export function fileDrop(node: HTMLElement, options: DropOptions) {
  const document = node.ownerDocument;
  let depth = 0;
  const hasFiles = (event: DragEvent) => Array.from(event.dataTransfer?.types ?? []).includes('Files');
  const reset = () => { depth = 0; options.onActive(false); };
  function enter(event: DragEvent) {
    if (!hasFiles(event)) return;
    event.preventDefault();
    depth++;
    options.onActive(options.enabled());
  }
  function over(event: DragEvent) {
    if (!hasFiles(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = options.enabled() ? 'copy' : 'none';
  }
  function leave(event: DragEvent) {
    if (!hasFiles(event) && !depth) return;
    if (--depth <= 0) reset();
  }
  function drop(event: DragEvent) {
    if (!hasFiles(event)) return;
    event.preventDefault();
    reset();
    if (!options.enabled() || !event.dataTransfer) return;
    // A directory cannot be uploaded as a regular file; do not silently lose its contents.
    if (Array.from(event.dataTransfer.items ?? []).some(item => item.webkitGetAsEntry?.()?.isDirectory)) {
      options.onError('directory');
      return;
    }
    const files = Array.from(event.dataTransfer.files);
    if (files.length) {
      const onError = options.onError;
      try { Promise.resolve(options.onFiles(files)).catch(() => onError('upload')); }
      catch { onError('upload'); }
    }
  }
  function guard(event: DragEvent) {
    if (!hasFiles(event)) return;
    // Dropping outside the composer must not navigate away from an unsent draft.
    event.preventDefault();
    if (event.type === 'drop') reset();
    else if (event.dataTransfer && !node.contains(event.target as Node)) event.dataTransfer.dropEffect = 'none';
  }
  node.addEventListener('dragenter', enter);
  node.addEventListener('dragover', over);
  node.addEventListener('dragleave', leave);
  node.addEventListener('drop', drop);
  document.addEventListener('dragover', guard);
  document.addEventListener('drop', guard);
  document.addEventListener('dragend', reset);
  return {
    update(next: DropOptions) { options = next; if (!options.enabled()) reset(); },
    destroy() {
      node.removeEventListener('dragenter', enter);
      node.removeEventListener('dragover', over);
      node.removeEventListener('dragleave', leave);
      node.removeEventListener('drop', drop);
      document.removeEventListener('dragover', guard);
      document.removeEventListener('drop', guard);
      document.removeEventListener('dragend', reset);
    },
  };
}
