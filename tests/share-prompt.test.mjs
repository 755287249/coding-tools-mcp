import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

const moduleUrl = source => 'data:text/javascript;base64,' + Buffer.from(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText).toString('base64');
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const prompt = moduleUrl(read('../src/lib/connect/prompt.ts'));
const { buildSharePrompt, isPublicEndpoint } = await import(moduleUrl(read('../src/lib/connect/share-prompt.ts').replace('"./prompt"', JSON.stringify(prompt))));

const base = { endpoint: 'https://mcp.example.test/mcp', authType: 'bearer', clientId: '', password: '', bearerToken: 'synthetic-token', folderId: 'folder-"1\n', folderName: 'Docs', access: 'readonly' };

test('share prompt targets one folder without chat tools', () => {
  for (const locale of ['en', 'zh-CN', 'zh-TW', 'ja']) {
    const text = buildSharePrompt(base, locale);
    assert.match(text, /https:\/\/mcp\.example\.test\/mcp/);
    assert.match(text, /Authorization: Bearer synthetic-token/);
    assert.match(text, /switch_workspace_folder/);
    assert.match(text, /User-Agent: Coding-Tools-MCP\/1\.0/);
    assert.doesNotMatch(text, /chat_open|chat_wait|seed_/);
    const line = text.split('\n').find(item => item.includes('"folder_id"'));
    assert.deepEqual(JSON.parse(line.slice(line.indexOf('{'))), { folder_id: 'folder-"1\n', name: 'Docs' });
    assert.ok(text.length < 700, `${locale} share prompt should stay short`);
  }
});

test('access mode changes only the permission rule', () => {
  const readonly = buildSharePrompt(base, 'zh-CN');
  const full = buildSharePrompt({ ...base, access: 'full' }, 'zh-CN');
  assert.match(readonly, /只读/);
  assert.match(readonly, /read_file/);
  assert.doesNotMatch(readonly, /执行命令。删除/);
  assert.match(full, /读写文件和执行命令/);
  assert.match(full, /先询问我/);
});

test('oauth and no-auth connections render the right credential line', () => {
  const oauth = buildSharePrompt({ ...base, authType: 'oauth', clientId: 'client-1', password: 'pw-1', bearerToken: '' }, 'en');
  assert.match(oauth, /Client ID: client-1/);
  assert.match(oauth, /pw-1/);
  assert.match(oauth, /PKCE/);
  assert.doesNotMatch(oauth, /Bearer synthetic/);
  const none = buildSharePrompt({ ...base, authType: 'none', bearerToken: '' }, 'en');
  assert.doesNotMatch(none, /Authorization|OAuth/);
});

test('invalid endpoints and missing folders are rejected', () => {
  assert.throws(() => buildSharePrompt({ ...base, endpoint: 'not a url' }, 'en'));
  assert.throws(() => buildSharePrompt({ ...base, endpoint: 'ftp://x.test/mcp' }, 'en'));
  assert.throws(() => buildSharePrompt({ ...base, endpoint: 'https://user:pw@x.test/mcp' }, 'en'));
  assert.throws(() => buildSharePrompt({ ...base, folderId: '' }, 'en'));
});

test('public endpoint detection rejects loopback and private hosts', () => {
  for (const url of ['http://127.0.0.1:8765/mcp', 'http://localhost/mcp', 'http://192.168.1.2/mcp', 'http://10.0.0.1/mcp', 'http://172.20.0.1/mcp', 'http://[::1]/mcp', 'http://box.local/mcp', 'bad'])
    assert.equal(isPublicEndpoint(url), false, url);
  for (const url of ['https://mcp.example.test/mcp', 'https://abc.trycloudflare.com/mcp', 'http://8.8.8.8/mcp'])
    assert.equal(isPublicEndpoint(url), true, url);
});

test('project navigator places the share button before the details menu', () => {
  const source = read('../src/lib/components/chat/ProjectNavigator.svelte');
  const share = source.indexOf('class="project-share-trigger"');
  const info = source.indexOf('class="project-info-trigger"');
  assert.ok(share > 0 && share < info);
  assert.match(source, /<ShareWorkspaceDialog workspace=\{sharedProfile\}/);
});
