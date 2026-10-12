import type { IncomingMessage, ServerResponse } from 'node:http';
import { lookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import { mkdir, readFile, readdir, rm, writeFile, rename } from 'node:fs/promises';
import path from 'node:path';
import { sendJson } from '../oauth.js';
import { readRequestBody } from '../server/http.js';
import { defaultDataDir } from '../config.js';

/**
 * Host services for UI plugins:
 * - an HTTPS proxy to public hosts (the UI bridge limits each plugin to the
 *   hosts its manifest declares). Redirects are not followed; Set-Cookie and
 *   Location come back so the plugin SDK keeps its own cookie jar.
 * - installed plugin packages and per-plugin data in the app data directory.
 */
const METHODS = new Set(['GET', 'POST', 'PUT', 'PATCH', 'DELETE']);
const BLOCKED_HEADERS = new Set(['host', 'content-length', 'connection', 'transfer-encoding', 'upgrade', 'keep-alive', 'proxy-authorization', 'te', 'trailer']);
const MAX_RESPONSE = 4 * 1024 * 1024;
const PLUGIN_ID = /^[a-z][a-z0-9-]{1,63}$/;

/** Loopback, private, link-local and other non-public addresses. */
export function privateAddress(address: string): boolean {
  const v4 = address.toLowerCase().startsWith('::ffff:') ? address.slice(7) : address;
  if (isIP(v4) === 4) {
    const [a, b] = v4.split('.').map(Number);
    return a === 0 || a === 10 || a === 127 || (a === 100 && b >= 64 && b <= 127) || (a === 169 && b === 254)
      || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 198 && (b === 18 || b === 19)) || a >= 224;
  }
  const v6 = address.toLowerCase();
  return v6 === '::' || v6 === '::1' || v6.startsWith('fc') || v6.startsWith('fd') || v6.startsWith('fe8') || v6.startsWith('fe9')
    || v6.startsWith('fea') || v6.startsWith('feb') || v6.startsWith('ff');
}

type Resolver = (host: string) => Promise<string[]>;
const systemResolver: Resolver = async host => {
  const rows = await lookup(host, { all: true });
  return rows.map(row => row.address);
};

/** HTTPS only, no credentials in the URL, and every resolved address public. */
export async function assertPublicUrl(raw: string, resolve: Resolver = systemResolver): Promise<URL> {
  let url: URL;
  try { url = new URL(raw); } catch { throw new Error('Plugin request URL is invalid'); }
  if (url.protocol !== 'https:' || url.username || url.password) throw new Error('Plugin requests must use HTTPS');
  const host = url.hostname.replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local') || host.endsWith('.internal')) throw new Error('Plugin request host not allowed');
  const addresses = isIP(host) ? [host] : await resolve(host);
  if (!addresses.length || addresses.some(privateAddress)) throw new Error('Plugin request host not allowed');
  return url;
}

export interface PluginHttpResult { status: number; headers: Record<string, string | string[]>; body: string }

export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  description?: string;
  permissions?: { hosts?: string[]; seedLibrary?: boolean; progress?: boolean; storage?: boolean };
}
export interface PluginPackage { format: 1; manifest: PluginManifest; code: string }

function pluginsDir(): string { return path.join(defaultDataDir, 'plugins'); }
function packagePath(id: string): string { return path.join(pluginsDir(), `${id}.ctplugin.json`); }
function dataPath(id: string): string { return path.join(pluginsDir(), `${id}.data.json`); }

export function validatePluginPackage(input: unknown): PluginPackage {
  const value = input as Partial<PluginPackage>;
  const manifest = value?.manifest as PluginManifest | undefined;
  if (value?.format !== 1 || !manifest || !PLUGIN_ID.test(String(manifest.id ?? ''))) throw new Error('Invalid plugin package');
  if (typeof manifest.name !== 'string' || !manifest.name.trim() || typeof manifest.version !== 'string' || !manifest.version.trim()) throw new Error('Invalid plugin manifest');
  if (typeof value.code !== 'string' || !value.code.trim() || value.code.length > 4 * 1024 * 1024) throw new Error('Invalid plugin code');
  const hosts = manifest.permissions?.hosts ?? [];
  if (!Array.isArray(hosts) || hosts.some(host => typeof host !== 'string' || !/^[a-z0-9.-]+(?::\d+)?$/i.test(host) || host.includes('..'))) throw new Error('Invalid plugin hosts');
  return { format: 1, manifest: { ...manifest, id: manifest.id, name: manifest.name.trim(), version: manifest.version.trim(), permissions: { ...manifest.permissions, hosts: [...new Set(hosts.map(host => host.toLowerCase()))] } }, code: value.code };
}

export async function listPlugins(): Promise<PluginPackage[]> {
  await mkdir(pluginsDir(), { recursive: true });
  const files = (await readdir(pluginsDir())).filter(file => file.endsWith('.ctplugin.json'));
  const packages = await Promise.all(files.map(async file => { try { return validatePluginPackage(JSON.parse(await readFile(path.join(pluginsDir(), file), 'utf8'))); } catch { return null; } }));
  return packages.filter((value): value is PluginPackage => value !== null);
}

export async function installPlugin(input: unknown): Promise<PluginPackage> {
  const plugin = validatePluginPackage(input);
  await mkdir(pluginsDir(), { recursive: true });
  const target = packagePath(plugin.manifest.id), temporary = `${target}.${process.pid}.tmp`;
  await writeFile(temporary, JSON.stringify(plugin), { encoding: 'utf8', mode: 0o600 });
  await rename(temporary, target);
  return plugin;
}

export async function removePlugin(id: string): Promise<void> {
  if (!PLUGIN_ID.test(id)) throw new Error('Invalid plugin ID');
  await rm(packagePath(id), { force: true });
  await rm(dataPath(id), { force: true });
}

export async function getPluginData(id: string): Promise<Record<string, unknown>> {
  if (!PLUGIN_ID.test(id)) throw new Error('Invalid plugin ID');
  try { const value = JSON.parse(await readFile(dataPath(id), 'utf8')); return value && typeof value === 'object' && !Array.isArray(value) ? value : {}; }
  catch (error: any) { if (error?.code === 'ENOENT') return {}; throw error; }
}

export async function setPluginData(id: string, value: unknown): Promise<void> {
  if (!PLUGIN_ID.test(id) || !value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid plugin data');
  const text = JSON.stringify(value);
  if (text.length > 1024 * 1024) throw new Error('Plugin data too large');
  await mkdir(pluginsDir(), { recursive: true });
  const target = dataPath(id), temporary = `${target}.${process.pid}.tmp`;
  await writeFile(temporary, text, { encoding: 'utf8', mode: 0o600 });
  await rename(temporary, target);
}

export async function pluginHttp(input: unknown, fetcher: typeof fetch = fetch, resolve: Resolver = systemResolver): Promise<PluginHttpResult> {
  const request = input as { url?: unknown; method?: unknown; headers?: unknown; body?: unknown };
  const method = String(request?.method ?? 'GET').toUpperCase();
  if (!METHODS.has(method)) throw new Error('Plugin request method not allowed');
  const url = await assertPublicUrl(String(request?.url ?? ''), resolve);
  const headers: Record<string, string> = { 'user-agent': 'Coding-Tools-MCP-Plugin/1.0' };
  for (const [key, value] of Object.entries((request.headers ?? {}) as Record<string, unknown>)) {
    const name = key.toLowerCase();
    if (!BLOCKED_HEADERS.has(name) && !name.startsWith('proxy-') && !name.startsWith('sec-') && typeof value === 'string' && !/[\r\n]/.test(value)) headers[name] = value;
  }
  const body = typeof request.body === 'string' ? request.body : undefined;
  if (body && body.length > 512 * 1024) throw new Error('Plugin request body too large');
  const response = await fetcher(url, { method, headers, body: method === 'GET' ? undefined : body, redirect: 'manual', signal: AbortSignal.timeout(30000) });
  const text = await response.text();
  if (text.length > MAX_RESPONSE) throw new Error('Plugin response too large');
  const out: Record<string, string | string[]> = {};
  response.headers.forEach((value, key) => { if (key !== 'set-cookie') out[key] = value; });
  const cookies = typeof response.headers.getSetCookie === 'function' ? response.headers.getSetCookie() : [];
  if (cookies.length) out['set-cookie'] = cookies;
  return { status: response.status, headers: out, body: text };
}

export async function handlePluginRoute(req: IncomingMessage, res: ServerResponse, pathname: string): Promise<boolean> {
  if (!pathname.startsWith('/admin/api/plugins')) return false;
  try {
    if (pathname === '/admin/api/plugins/http' && req.method === 'POST') sendJson(res, 200, await pluginHttp(JSON.parse((await readRequestBody(req, 1024 * 1024)).toString('utf8'))));
    else if (pathname === '/admin/api/plugins' && req.method === 'GET') sendJson(res, 200, await listPlugins());
    else if (pathname === '/admin/api/plugins' && req.method === 'POST') sendJson(res, 200, await installPlugin(JSON.parse((await readRequestBody(req, 5 * 1024 * 1024)).toString('utf8'))));
    else {
      const match = pathname.match(/^\/admin\/api\/plugins\/([a-z][a-z0-9-]{1,63})(\/data)?$/);
      if (!match) return false;
      if (!match[2] && req.method === 'DELETE') { await removePlugin(match[1]); sendJson(res, 200, { ok: true }); }
      else if (match[2] && req.method === 'GET') sendJson(res, 200, await getPluginData(match[1]));
      else if (match[2] && req.method === 'PUT') { await setPluginData(match[1], JSON.parse((await readRequestBody(req, 1024 * 1024)).toString('utf8'))); sendJson(res, 200, { ok: true }); }
      else return false;
    }
  } catch (error) { sendJson(res, 400, { error: { code: 'PLUGIN_ERROR', message: error instanceof Error ? error.message : String(error) } }); }
  return true;
}
