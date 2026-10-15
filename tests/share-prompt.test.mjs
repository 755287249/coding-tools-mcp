import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

const moduleUrl = source => 'data:text/javascript;base64,' + Buffer.from(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText).toString('base64');
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const { buildSharePrompt, shareUrl, isPublicEndpoint } = await import(moduleUrl(read('../src/lib/connect/share-prompt.ts')));
const key = 'a'.repeat(64);

test('share prompt is the original seed sentence with only the share URL', () => {
  const url = shareUrl('https://x.trycloudflare.com/mcp', 'folder-1', key);
  assert.equal(url, `https://x.trycloudflare.com/mcp/share/folder-1/${key}`);
  assert.equal(buildSharePrompt(url, 'zh-CN'), `请连接我的工具 ${url} 。快速连接这个 sandbox 的 MCP（URL），明确使用规则，熟悉可用工具，做好处理接下来一系列操作的准备。工具在 MCP 内部，而非你的客户端内部，连上后使用，然后等待信息。`);
  for (const locale of ['en', 'zh-CN', 'zh-TW', 'ja']) {
    const text = buildSharePrompt(url, locale);
    assert.ok(text.includes(url));
    assert.doesNotMatch(text, /Authorization|Bearer|OAuth|switch_workspace_folder|folder_id|只读|Read-only/);
  }
});

test('share URL keeps a configured path prefix and rejects unsafe input', () => {
  assert.equal(shareUrl('https://host.test/clients/a/mcp/', 'f', key), `https://host.test/clients/a/mcp/share/f/${key}`);
  assert.equal(shareUrl('http://127.0.0.1:8765/mcp', 'f', key), `http://127.0.0.1:8765/mcp/share/f/${key}`);
  for (const [endpoint, folder, value] of [
    ['http://host.test/mcp', 'f', key],
    ['https://host.test/other', 'f', key],
    ['https://user:pw@host.test/mcp', 'f', key],
    ['https://host.test/mcp?x=1', 'f', key],
    ['https://host.test/mcp', '../x', key],
    ['https://host.test/mcp', 'f', 'short'],
    ['not a url', 'f', key],
  ]) assert.throws(() => shareUrl(endpoint, folder, value), `${endpoint} ${folder} ${value}`);
});

test('public endpoint detection rejects loopback and private hosts', () => {
  for (const url of ['http://127.0.0.1:8765/mcp', 'http://localhost/mcp', 'http://192.168.1.2/mcp', 'http://10.0.0.1/mcp', 'http://172.20.0.1/mcp', 'http://[::1]/mcp', 'http://box.local/mcp', 'http://[::]/mcp', 'http://[::ffff:127.0.0.1]/mcp', 'http://[fd00::1]/mcp', 'http://[fe80::1]/mcp', 'bad'])
    assert.equal(isPublicEndpoint(url), false, url);
  for (const url of ['https://mcp.example.test/mcp', 'https://abc.trycloudflare.com/mcp', 'http://8.8.8.8/mcp', 'http://[2001:db8::1]/mcp'])
    assert.equal(isPublicEndpoint(url), true, url);
});

test('project navigator places the share button before the details menu', () => {
  const source = read('../src/lib/components/chat/ProjectNavigator.svelte');
  const share = source.indexOf('class="project-share-trigger"');
  const info = source.indexOf('class="project-info-trigger"');
  assert.ok(share > 0 && share < info);
  assert.match(source, /<ShareWorkspaceDialog workspace=\{sharedProfile\}/);
});

test('share dialog uses server share links, not the global credential', () => {
  const source = read('../src/lib/components/chat/ShareWorkspaceDialog.svelte');
  assert.match(source, /action: 'share_create', access/);
  assert.match(source, /action: 'share_revoke'/);
  assert.match(source, /runtime\.state !== 'running'/);
  assert.doesNotMatch(source, /loadMcpAuthSecrets|bearer_token|oauth_password/);
});
