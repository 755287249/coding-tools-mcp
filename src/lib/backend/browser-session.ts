import { writable } from 'svelte/store';
const key = 'ctmcp-browser-session';
function saved(): string { try { return sessionStorage.getItem(key) ?? ''; } catch { return ''; } }
let token = typeof window === 'undefined' ? '' : saved();
let epoch = 0;
export const browserAuthenticated = writable(!!token);
export function forgetBrowserSession(): void {
  epoch++;
  token = ''; try { sessionStorage.removeItem(key); } catch { /* In-memory fallback. */ }
  browserAuthenticated.set(false);
}
class BrowserResponseError extends Error {
  readonly retryable: boolean;
  constructor(message: string, retryable: boolean) { super(message); this.retryable = retryable; }
}
async function responseData(response: Response): Promise<Record<string, unknown>> {
  const data: unknown = await response.json().catch(() => null);
  const record = data && typeof data === 'object' && !Array.isArray(data) ? data as Record<string, unknown> : null;
  const type = (response.headers.get('content-type') ?? 'unknown').split(';')[0].slice(0,80);
  const context = `HTTP ${response.status}, ${type}`;
  if (!response.ok) throw new BrowserResponseError(typeof record?.error === 'string' ? record.error : `Browser request failed (${context}). Check the browser sharing connection.`, [502,503,504,520,522,524].includes(response.status));
  if (!record) throw new BrowserResponseError(`Invalid browser response (${context}): expected JSON. Check the proxy route and refresh the page.`, true);
  return record;
}
/** Explicit read allowlist. Unknown commands and every mutation are sent exactly once. */
function retryableRead(command: string, args: Record<string, unknown>): boolean {
  if (['list_workspaces','list_frp_profiles','list_software','get_proxy','get_last_workspace_id','get_runtime_status','get_actions_runtime_status'].includes(command)) return true;
  if (command !== 'local_chat' || !args.args || typeof args.args !== 'object') return false;
  return ['list','read','discussion_list','discussion_read'].includes(String((args.args as Record<string, unknown>).action));
}
export async function browserLogin(password: string): Promise<void> {
  const requestEpoch = ++epoch;
  const response = await fetch('/browser/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ password }), cache: 'no-store' });
  const data = await responseData(response);
  if (requestEpoch !== epoch) throw new Error('Browser session changed; retry the request');
  if (typeof data.token !== 'string' || !data.token.trim()) throw new Error('Invalid browser response');
  token = data.token;
  try { sessionStorage.setItem(key, token); } catch { /* In-memory fallback. */ }
  browserAuthenticated.set(true);
}
export async function browserLogout(): Promise<void> {
  const previous = token;
  // Clear immediately; a slow logout must not erase a subsequent login.
  forgetBrowserSession();
  if (previous) await fetch('/browser/logout', { method: 'POST', headers: { authorization: `Bearer ${previous}` } });
}
export async function browserInvoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!token) throw new Error('Browser login required');
  const requestEpoch = epoch;
  const read = retryableRead(command, args);
  for (let attempt = 0; ; attempt++) {
    if (requestEpoch !== epoch) throw new Error('Browser session changed; retry the request');
    try {
      const response = await fetch('/browser/invoke', { method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` }, body: JSON.stringify({ command, args }), cache: 'no-store', ...(read ? {signal: AbortSignal.timeout(15000)} : {}) });
      if (requestEpoch !== epoch) throw new Error('Browser session changed; retry the request');
      // A proxy may return HTML instead of JSON. Expiry must still clear local auth.
      if (response.status === 401) forgetBrowserSession();
      const data = await responseData(response);
      if (requestEpoch !== epoch) throw new Error('Browser session changed; retry the request');
      if (!Object.hasOwn(data, 'value')) throw new BrowserResponseError('Invalid browser response: missing result. Refresh the page and check client/server versions.', false);
      return data.value as T;
    } catch (error) {
      const transient = error instanceof BrowserResponseError ? error.retryable : error instanceof TypeError || (error instanceof DOMException && error.name === 'TimeoutError');
      if (!read || attempt > 0 || requestEpoch !== epoch || !transient) throw error;
      await new Promise(resolve => setTimeout(resolve, 250));
    }
  }
}
