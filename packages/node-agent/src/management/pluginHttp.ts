import type { IncomingMessage, ServerResponse } from 'node:http';
import { sendJson } from '../oauth.js';
import { readRequestBody } from '../server/http.js';

/**
 * Allow-listed HTTPS proxy for UI plugins (the CodeRabbit seeding plugin).
 * Only these endpoints and headers pass; redirects are never followed and
 * no cookies are kept, so the proxy cannot reach the local network.
 */
const RULES: { method: string; pattern: RegExp }[] = [
  { method: 'GET', pattern: /^https:\/\/app\.coderabbit\.ai\/trpc\/[A-Za-z][A-Za-z0-9.,]{0,200}\?batch=1(&input=[^#]*)?$/ },
  { method: 'POST', pattern: /^https:\/\/app\.coderabbit\.ai\/trpc\/[A-Za-z][A-Za-z0-9.,]{0,200}\?batch=1$/ },
  { method: 'POST', pattern: /^https:\/\/github\.com\/login\/device\/code$/ },
  { method: 'POST', pattern: /^https:\/\/github\.com\/login\/oauth\/access_token$/ },
  { method: 'POST', pattern: /^https:\/\/api\.github\.com\/user\/repos$/ },
  { method: 'PUT', pattern: /^https:\/\/api\.github\.com\/repos\/[A-Za-z0-9_.-]{1,100}\/[A-Za-z0-9_.-]{1,100}\/contents\/[A-Za-z0-9_./-]{1,200}$/ },
];
const HEADERS = new Set(['authorization', 'content-type', 'accept', 'x-trpc-source', 'x-clerk-git-provider', 'x-coderabbitai-organization', 'x-coderabbitai-workspace', 'x-github-api-version']);
const MAX_RESPONSE = 4 * 1024 * 1024;

export function pluginRequestAllowed(method: string, url: string): boolean {
  if (url.includes('..')) return false;
  return RULES.some(rule => rule.method === method && rule.pattern.test(url));
}

export async function pluginHttp(input: unknown, fetcher: typeof fetch = fetch): Promise<{ status: number; body: string }> {
  const request = input as { url?: unknown; method?: unknown; headers?: unknown; body?: unknown };
  const url = String(request?.url ?? ''), method = String(request?.method ?? 'GET');
  if (!pluginRequestAllowed(method, url)) throw new Error('Plugin request not allowed');
  const headers: Record<string, string> = { 'user-agent': 'Coding-Tools-MCP-Plugin/1.0' };
  for (const [key, value] of Object.entries((request.headers ?? {}) as Record<string, unknown>)) {
    const name = key.toLowerCase();
    if (HEADERS.has(name) && typeof value === 'string' && !/[\r\n]/.test(value)) headers[name] = value;
  }
  const body = typeof request.body === 'string' ? request.body : undefined;
  if (body && body.length > 256 * 1024) throw new Error('Plugin request body too large');
  const response = await fetcher(url, { method, headers, body: method === 'GET' ? undefined : body, redirect: 'manual', signal: AbortSignal.timeout(30000) });
  const text = await response.text();
  if (text.length > MAX_RESPONSE) throw new Error('Plugin response too large');
  return { status: response.status, body: text };
}

export async function handlePluginRoute(req: IncomingMessage, res: ServerResponse, pathname: string): Promise<boolean> {
  if (pathname !== '/admin/api/plugins/http') return false;
  if (req.method !== 'POST') { sendJson(res, 405, { error: { code: 'METHOD_NOT_ALLOWED', message: 'POST required' } }); return true; }
  try {
    const body = JSON.parse((await readRequestBody(req, 512 * 1024)).toString());
    sendJson(res, 200, await pluginHttp(body));
  } catch (error) {
    sendJson(res, 400, { error: { code: 'PLUGIN_REQUEST_FAILED', message: error instanceof Error ? error.message : 'Plugin request failed' } });
  }
  return true;
}
