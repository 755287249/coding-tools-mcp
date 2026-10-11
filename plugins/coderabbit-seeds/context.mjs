/** Capture only routing identifiers from the page's own same-origin tRPC calls. */
export function routingContext(url, headers, origin = 'https://app.coderabbit.ai') {
  let target;
  try { target = new URL(url, origin); } catch { return null; }
  if (target.origin !== origin || !target.pathname.startsWith('/trpc/')) return null;
  let h;
  try { h = new Headers(headers); } catch { return null; }
  const organization = h.get('x-coderabbitai-organization')?.trim();
  const workspace = h.get('x-coderabbitai-workspace')?.trim() || '';
  const valid = value => /^[A-Za-z0-9_-]{1,200}$/.test(value);
  if (!organization || !valid(organization) || (workspace && !valid(workspace))) return null;
  return {organization, workspace};
}
export function observeRouting(page, onContext) {
  const originalFetch = page.fetch;
  page.fetch = function(input, init) {
    try {
      const context = routingContext(typeof input === 'string' || input instanceof URL ? String(input) : input.url,
        init?.headers !== undefined ? init.headers : input?.headers);
      if (context) onContext(context);
    } catch { /* Observation must never break the page's request. */ }
    return Reflect.apply(originalFetch, this, arguments);
  };
  const prototype = page.XMLHttpRequest?.prototype;
  if (prototype) {
    const requests = new WeakMap(), open = prototype.open, header = prototype.setRequestHeader, send = prototype.send;
    prototype.open = function(method, url) {
      requests.set(this, {url: String(url), headers: new Headers()});
      return Reflect.apply(open, this, arguments);
    };
    prototype.setRequestHeader = function(name, value) {
      const key = String(name).toLowerCase();
      if (key === 'x-coderabbitai-organization' || key === 'x-coderabbitai-workspace') requests.get(this)?.headers.append(name, value);
      return Reflect.apply(header, this, arguments);
    };
    prototype.send = function() {
      try { const request = requests.get(this); const context = request && routingContext(request.url, request.headers); if (context) onContext(context); } catch {}
      return Reflect.apply(send, this, arguments);
    };
  }
  // Use the original transport for our requests, so manual overrides cannot masquerade as page observations.
  return originalFetch.bind(page);
}
