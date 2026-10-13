/**
 * Create a starter GitHub repository when CodeRabbit lists none. CodeRabbit's
 * own GitHub login cannot create repositories, so the user authorizes once with
 * GitHub's device flow (the code is entered on github.com in the login window).
 * The GitHub token stays in memory only.
 */
import type { PluginHttp } from './engine.ts';

/** Public OAuth client of GitHub CLI; replace with a dedicated OAuth app if one is registered. */
export const GITHUB_CLIENT_ID = '178c6fc778ccc68e1d6a';

export interface DeviceCode { device_code: string; user_code: string; verification_uri: string; interval: number; expires_in: number }

function form(values: Record<string, string>): string { return new URLSearchParams(values).toString(); }
function json(body: string): any { try { return JSON.parse(body); } catch { throw new Error('GitHub returned an unexpected response'); } }

export async function startDeviceFlow(http: PluginHttp): Promise<DeviceCode> {
  const response = await http({ url: 'https://github.com/login/device/code', method: 'POST',
    headers: { accept: 'application/json', 'content-type': 'application/x-www-form-urlencoded' },
    body: form({ client_id: GITHUB_CLIENT_ID, scope: 'repo' }) });
  const data = json(response.body);
  if (response.status !== 200 || !data.device_code || !data.user_code) throw new Error('GitHub authorization could not start');
  return { device_code: data.device_code, user_code: data.user_code, verification_uri: data.verification_uri || 'https://github.com/login/device',
    interval: Math.max(5, Number(data.interval) || 5), expires_in: Number(data.expires_in) || 900 };
}

export async function pollDeviceToken(http: PluginHttp, code: DeviceCode, signal?: AbortSignal, sleep = (ms: number) => new Promise(r => setTimeout(r, ms))): Promise<string> {
  let interval = code.interval;
  const deadline = Date.now() + code.expires_in * 1000;
  while (Date.now() < deadline) {
    if (signal?.aborted) throw new Error('GitHub authorization cancelled');
    await sleep(interval * 1000);
    const response = await http({ url: 'https://github.com/login/oauth/access_token', method: 'POST',
      headers: { accept: 'application/json', 'content-type': 'application/x-www-form-urlencoded' },
      body: form({ client_id: GITHUB_CLIENT_ID, device_code: code.device_code, grant_type: 'urn:ietf:params:oauth:grant-type:device_code' }) });
    const data = json(response.body);
    if (data.access_token) return String(data.access_token);
    if (data.error === 'slow_down') interval += 5;
    else if (data.error !== 'authorization_pending') throw new Error(`GitHub authorization failed: ${data.error ?? response.status}`);
  }
  throw new Error('GitHub authorization expired');
}

/** A small random program so the repository is not empty. */
export function randomSource(random = Math.random): { path: string; content: string } {
  const words = ['alpha', 'bravo', 'delta', 'echo', 'kilo', 'lima', 'nova', 'orbit', 'pixel', 'quartz', 'tango', 'zephyr'];
  const pick = () => words[Math.floor(random() * words.length)];
  const name = `${pick()}_${pick()}`, values = Array.from({ length: 8 }, () => Math.floor(random() * 1000));
  const content = `"""${name}: generated starter module."""\n\nVALUES = ${JSON.stringify(values)}\n\n\ndef ${name}(values=VALUES):\n    total = 0\n    for index, value in enumerate(values):\n        total += value * (index + 1)\n    return total\n\n\nif __name__ == "__main__":\n    print(${name}())\n`;
  return { path: `src/${name}.py`, content };
}

function utf8Base64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = ''; for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export async function createStarterRepository(http: PluginHttp, token: string, random = Math.random): Promise<string> {
  const headers = { accept: 'application/vnd.github+json', authorization: `Bearer ${token}`, 'content-type': 'application/json', 'x-github-api-version': '2022-11-28' };
  const name = `seed-${Date.now().toString(36)}-${Math.floor(random() * 1e6).toString(36)}`;
  const created = await http({ url: 'https://api.github.com/user/repos', method: 'POST', headers,
    body: JSON.stringify({ name, private: true, auto_init: true, description: 'Starter repository for Coding Tools MCP seeds' }) });
  const repo = json(created.body);
  if (created.status !== 201 || !repo.full_name) throw new Error(`GitHub could not create a repository (HTTP ${created.status})`);
  const file = randomSource(random);
  const put = await http({ url: `https://api.github.com/repos/${repo.full_name}/contents/${file.path}`, method: 'PUT', headers,
    body: JSON.stringify({ message: 'Add starter code', content: utf8Base64(file.content) }) });
  if (put.status !== 201 && put.status !== 200) throw new Error(`GitHub could not add the starter file (HTTP ${put.status})`);
  return String(repo.full_name);
}
