import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function loadModule() {
  const source = await readFile(path.join(root, 'src/lib/connect/cloudflare-token.ts'), 'utf8');
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  });
  return import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
}

// {"a":"1234567890abcdef","t":"5f3eb6e8-1234-4567-89ab-cdef01234567","s":"c2VjcmV0"}
const TOKEN =
  'eyJhIjoiMTIzNDU2Nzg5MGFiY2RlZiIsInQiOiI1ZjNlYjZlOC0xMjM0LTQ1NjctODlhYi1jZGVmMDEyMzQ1NjciLCJzIjoiYzJWamNtVjAifQ==';
const TUNNEL_ID = '5f3eb6e8-1234-4567-89ab-cdef01234567';

test('token field accepts the whole install command copied from the Cloudflare dashboard', async () => {
  const { normalizeCloudflareToken } = await loadModule();
  for (const raw of [
    TOKEN,
    `  ${TOKEN}\n`,
    `cloudflared.exe service install ${TOKEN}`,
    `sudo cloudflared service install ${TOKEN}`,
    `cloudflared tunnel run --token ${TOKEN}`,
    `cloudflared tunnel --no-autoupdate run --token="${TOKEN}"`,
    `--token=${TOKEN}`,
    `--token="${TOKEN}"`,
    `& 'C:\\Program Files\\cloudflared\\cloudflared.exe' service install '${TOKEN}'`,
  ]) {
    assert.equal(normalizeCloudflareToken(raw), TOKEN, `input: ${raw}`);
  }
  assert.equal(normalizeCloudflareToken(''), '');
});

test('tunnel id is decoded from the token and garbage is rejected', async () => {
  const { cloudflareTokenTunnelId } = await loadModule();
  assert.equal(cloudflareTokenTunnelId(`cloudflared.exe service install ${TOKEN}`), TUNNEL_ID);
  assert.equal(cloudflareTokenTunnelId(TOKEN.replace(/=+$/, '')), TUNNEL_ID);
  assert.equal(cloudflareTokenTunnelId('not-a-token'), null);
  assert.equal(cloudflareTokenTunnelId('cloudflared.exe service install'), null);
});

test('Rust and TypeScript normalizers are wired into every token save path', async () => {
  const [rust, secrets, supervisor, view, sessions] = await Promise.all([
    readFile(path.join(root, 'src-tauri/src/tunnel/cloudflare.rs'), 'utf8'),
    readFile(path.join(root, 'src-tauri/src/commands/secrets.rs'), 'utf8'),
    readFile(path.join(root, 'src-tauri/src/tunnel/supervisor.rs'), 'utf8'),
    readFile(path.join(root, 'src/lib/components/auto/AutoWorkspaceView.svelte'), 'utf8'),
    readFile(path.join(root, 'src/lib/stores/sessions.ts'), 'utf8'),
  ]);
  assert.match(rust, /pub fn normalize_cloudflare_token/);
  assert.match(rust, /ensure_cloudflared_current\(\)\.await/);
  assert.match(secrets, /normalize_cloudflare_token/);
  assert.match(supervisor, /normalize_cloudflare_token/);
  assert.match(view, /normalizeCloudflareToken/);
  assert.match(sessions, /cloudflared\.outdated/);
});
