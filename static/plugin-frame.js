(() => {
  // Base styles are created here instead of an inline <style> in plugin-frame.html: the desktop
  // shell adds a nonce for inline HTML styles, which would disable 'unsafe-inline' and block the
  // <style> elements that plugins inject at runtime.
  const base = document.createElement('style');
  base.textContent = 'html,body{margin:0;min-height:100%;font:14px system-ui;color:#18212f;background:#fff}*{box-sizing:border-box}button,input,textarea,select{font:inherit}';
  document.head.append(base);
  let started = false;
  addEventListener('message', event => {
    const message = event.data;
    if (started || event.source !== parent || message?.channel !== 'ctmcp-host' || message.type !== 'load' || typeof message.code !== 'string') return;
    started = true;
    const url = URL.createObjectURL(new Blob([message.code], { type: 'text/javascript' }));
    const script = document.createElement('script');
    script.src = url;
    script.onload = () => {
      URL.revokeObjectURL(url);
      parent.postMessage({ channel: 'ctmcp-plugin-frame', type: 'loaded' }, '*');
    };
    script.onerror = () => {
      URL.revokeObjectURL(url);
      parent.postMessage({ channel: 'ctmcp-plugin-frame', type: 'error', error: 'Plugin code failed to load' }, '*');
    };
    document.body.append(script);
  });
  addEventListener('error', event => {
    parent.postMessage({ channel: 'ctmcp-plugin-frame', type: 'error', error: String(event.message || 'Plugin error') }, '*');
  });
  parent.postMessage({ channel: 'ctmcp-plugin-frame', type: 'ready' }, '*');
})();
