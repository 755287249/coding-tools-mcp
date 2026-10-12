(() => {
  let started = false;
  addEventListener('message', event => {
    const message = event.data;
    if (started || event.source !== parent || message?.channel !== 'ctmcp-host' || message.type !== 'load' || typeof message.code !== 'string') return;
    started = true;
    const url = URL.createObjectURL(new Blob([message.code], { type: 'text/javascript' }));
    const script = document.createElement('script');
    script.src = url;
    script.onload = () => URL.revokeObjectURL(url);
    script.onerror = () => {
      URL.revokeObjectURL(url);
      parent.postMessage({ channel: 'ctmcp-plugin-frame', type: 'error', error: 'Plugin code failed to load' }, '*');
    };
    document.body.append(script);
  });
  parent.postMessage({ channel: 'ctmcp-plugin-frame', type: 'ready' }, '*');
})();
