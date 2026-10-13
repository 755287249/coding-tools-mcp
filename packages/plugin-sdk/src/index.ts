export interface PluginPermissions { hosts?: string[]; seedLibrary?: boolean; progress?: boolean; storage?: boolean }
export interface PluginManifest { id: string; name: string; version: string; description?: string; permissions?: PluginPermissions }
export interface PluginPackage { format: 1; manifest: PluginManifest; code: string }
export interface PluginContext { pluginId: string; workspaceId?: string; folderId?: string; publicMcpUrl?: string; locale?: string; theme?: string }
export interface HttpRequest { url: string; method?: 'GET'|'POST'|'PUT'|'PATCH'|'DELETE'; headers?: Record<string,string>; body?: string }
export interface HttpResponse { status: number; headers: Record<string,string|string[]>; body: string }
export interface Progress { total: number; done: number; status?: 'running'|'done'|'error'|'stopped'; message?: string; detail?: string; minimized?: boolean }

type EventHandler = (value: unknown) => void;
let sequence = 0;
const pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>();
const handlers = new Map<string, Set<EventHandler>>();
let contextValue: PluginContext | null = null;

function send(method: string, params?: unknown): Promise<any> {
  const id = ++sequence;
  parent.postMessage({ channel: 'ctmcp-plugin', type: 'request', id, method, params }, '*');
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

if (typeof addEventListener === 'function') addEventListener('message', event => {
  const message = event.data;
  if (event.source !== parent || message?.channel !== 'ctmcp-host') return;
  if (message.type === 'response') {
    const item = pending.get(message.id); if (!item) return; pending.delete(message.id);
    if (message.error) item.reject(new Error(String(message.error))); else item.resolve(message.result);
  } else if (message.type === 'event') {
    if (message.name === 'context') contextValue = message.value as PluginContext;
    handlers.get(message.name)?.forEach(handler => handler(message.value));
  }
});

export const plugin = {
  ready: () => send('ready'),
  context: async (): Promise<PluginContext> => contextValue ?? await send('context'),
  http: (request: HttpRequest): Promise<HttpResponse> => send('http', request),
  storage: {
    get: <T extends Record<string, unknown> = Record<string, unknown>>(): Promise<T> => send('storage.get'),
    set: (data: Record<string, unknown>): Promise<void> => send('storage.set', data),
  },
  seedLibrary: (args: Record<string, unknown>): Promise<any> => send('seedLibrary', args),
  progress: (value: Progress | null): Promise<void> => send('progress', value),
  on(name: 'stop'|'context'|string, handler: EventHandler): () => void {
    const set = handlers.get(name) ?? new Set(); set.add(handler); handlers.set(name, set); return () => set.delete(handler);
  },
};

interface Cookie { name: string; value: string; domain: string; path: string; secure: boolean; expires?: number; hostOnly: boolean }
export class CookieJar {
  private cookies: Cookie[] = [];
  set(urlValue: string, values: string|string[]|undefined): void {
    const url = new URL(urlValue);
    for (const line of typeof values === 'string' ? [values] : values ?? []) {
      const parts = line.split(';').map(value => value.trim()), pair = parts.shift()?.split('=');
      if (!pair?.[0]) continue;
      const cookie: Cookie = { name: pair.shift()!, value: pair.join('='), domain: url.hostname.toLowerCase(), path: '/', secure: false, hostOnly: true };
      let accepted = true;
      for (const attribute of parts) {
        const [rawName, ...rest] = attribute.split('='), name = rawName.toLowerCase(), value = rest.join('=');
        if (name === 'domain') { cookie.domain = value.replace(/^\./, '').toLowerCase(); cookie.hostOnly = false; accepted = url.hostname === cookie.domain || url.hostname.endsWith('.' + cookie.domain); }
        else if (name === 'path') cookie.path = value || '/'; else if (name === 'secure') cookie.secure = true;
        else if (name === 'max-age') cookie.expires = Date.now() + Number(value) * 1000;
        else if (name === 'expires') { const time = Date.parse(value); if (Number.isFinite(time)) cookie.expires = time; }
      }
      if (!accepted) continue;
      this.cookies = this.cookies.filter(item => !(item.name === cookie.name && item.domain === cookie.domain && item.path === cookie.path));
      if (!cookie.expires || cookie.expires > Date.now()) this.cookies.push(cookie);
    }
  }
  header(urlValue: string): string {
    const url = new URL(urlValue), now = Date.now(); this.cookies = this.cookies.filter(cookie => !cookie.expires || cookie.expires > now);
    return this.cookies.filter(cookie => (!cookie.secure || url.protocol === 'https:') && (cookie.hostOnly ? url.hostname === cookie.domain : url.hostname === cookie.domain || url.hostname.endsWith('.' + cookie.domain)) && url.pathname.startsWith(cookie.path))
      .map(cookie => `${cookie.name}=${cookie.value}`).join('; ');
  }
  toJSON(): unknown { return this.cookies; }
  static fromJSON(value: unknown): CookieJar { const jar = new CookieJar(); if (Array.isArray(value)) jar.cookies = value.filter(cookie => cookie && typeof cookie.name === 'string') as Cookie[]; return jar; }
  async fetch(request: HttpRequest): Promise<HttpResponse> {
    const cookie = this.header(request.url), response = await plugin.http({ ...request, headers: { ...request.headers, ...(cookie ? { cookie } : {}) } });
    this.set(request.url, response.headers['set-cookie']); return response;
  }
}
