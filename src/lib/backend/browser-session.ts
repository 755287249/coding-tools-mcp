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
async function responseData(response: Response): Promise<Record<string, unknown>> {
  const data: unknown = await response.json().catch(() => null);
  const record = data && typeof data === 'object' && !Array.isArray(data) ? data as Record<string, unknown> : null;
  if (!response.ok) throw new Error(typeof record?.error === 'string' ? record.error : `Browser request failed (HTTP ${response.status})`);
  if (!record) throw new Error('Invalid browser response');
  return record;
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
  const response = await fetch('/browser/invoke', { method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` }, body: JSON.stringify({ command, args }), cache: 'no-store' });
  if (requestEpoch !== epoch) throw new Error('Browser session changed; retry the request');
  // A proxy may return HTML instead of JSON. Expiry must still clear local auth.
  if (response.status === 401) forgetBrowserSession();
  const data = await responseData(response);
  if (requestEpoch !== epoch) throw new Error('Browser session changed; retry the request');
  if (!Object.hasOwn(data, 'value')) throw new Error('Invalid browser response');
  return data.value as T;
}
