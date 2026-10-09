import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { buildChatPrompt } from '../src/lib/connect/chat-prompt.ts';
import { buildConnectionPrompt, buildClientConfigJson, MANUAL_OAUTH_REDIRECT_URI } from '../src/lib/connect/prompt.ts';

test('session target round-trips quotes and newlines as one JSON line', () => {
  const chat = 'session-"\n1', folder = 'folder-\\"\n2';
  const prompt = buildChatPrompt(chat, folder);
  const target = prompt.split('\n').find(line => line.startsWith('目标参数'));
  assert.deepEqual(JSON.parse(target.slice(target.indexOf('{'))), { chat_id: chat, workspace_folder_id: folder });
  assert.match(prompt, /chat_open/);
  assert.match(prompt, /attachment_id/);
  assert.match(prompt, /skill.text/);
  assert.match(prompt, /chat_wait → 工作 → chat_reply → 再等待/);
  assert.ok(prompt.length < 400, 'copied session guidance should delegate details to the skill');
});

test('one copied prompt combines authentication, verified setup and the session loop', () => {
  const info = { workspaceName: 'test', endpoint: 'https://example.test/mcp', authType: 'bearer', clientId: '', password: '', bearerToken: 'synthetic-token', folders: ['/test'] };
  for (const locale of ['en', 'zh-CN', 'zh-TW', 'ja']) {
    const connection = buildConnectionPrompt(info, locale);
    const prompt = buildChatPrompt('chat-1', 'folder-1', connection);
    assert.ok(prompt.startsWith(connection));
    assert.match(prompt, /https:\/\/example.test\/mcp/);
    assert.match(prompt, /Authorization: Bearer synthetic-token/);
    assert.match(connection, /list_workspace_folders/);
    assert.match(prompt, /chat_open/);
    assert.match(prompt, /chat_wait/);
    assert.match(connection, /User-Agent/);
    assert.match(connection, /Cloudflare/);
    assert.match(connection, /HTTP/);
    assert.match(connection, /PKCE/);
    assert.match(connection, /notifications\/initialized/);
    assert.match(connection, /tools\/list/);
  }
});


test('manual OAuth includes an explicit callback while fixed-token config bypasses OAuth', () => {
  const info = { workspaceName: 'test', endpoint: 'https://example.test/mcp', authType: 'oauth', clientId: 'existing-client', password: 'synthetic-password', bearerToken: 'synthetic-token', folders: ['/test'] };
  for (const locale of ['en', 'zh-CN', 'zh-TW', 'ja']) {
    const oauth = buildConnectionPrompt(info, locale);
    assert.ok(oauth.includes(`redirect_uri=${MANUAL_OAUTH_REDIRECT_URI}`));
    for (const field of ['response_type=code', 'code_challenge_method=S256', 'Location', 'state', 'verifier', 'grant_type=authorization_code', 'code_verifier']) assert.ok(oauth.includes(field));
    const fixed = buildConnectionPrompt({ ...info, authType: 'bearer' }, locale);
    assert.ok(fixed.includes('Authorization: Bearer synthetic-token'));
    assert.ok(!fixed.includes('synthetic-password'));
    assert.ok(!fixed.includes('redirect_uri='));
  }
  const fixedConfig = JSON.parse(buildClientConfigJson({ ...info, authType: 'bearer' })).mcpServers['coding-tools'];
  assert.ok(fixedConfig.headers['User-Agent']);
  assert.equal(fixedConfig.headers.Authorization, 'Bearer synthetic-token');
  const oauthConfig = JSON.parse(buildClientConfigJson(info)).mcpServers['coding-tools'];
  assert.ok(oauthConfig.headers['User-Agent']);
  assert.equal(oauthConfig.headers.Authorization, undefined);
  assert.ok(!JSON.stringify(oauthConfig).includes('synthetic-password'));
});


test('desktop and Node expose the same startup and chat guidance', () => {
  const read = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8');
  const rust = read('src-tauri/src/mcp/server.rs').match(/const SERVER_INSTRUCTIONS: &str = ("[^\n]+");/)[1];
  const node = read('packages/node-agent/src/server/mcp/dispatcher.ts').match(/const SERVER_INSTRUCTIONS = ("[^\n]+");/)[1];
  assert.equal(JSON.parse(rust), JSON.parse(node));
  assert.ok(JSON.parse(rust).length < 1900);
  assert.doesNotMatch(JSON.parse(rust), /ChatGPT/);
  const rustChat = [...read('src-tauri/src/tools/chat.rs').matchAll(/"instruction":"([^"]+)"/g)].map(m => m[1]);
  const nodeChat = [...read('packages/node-agent/src/chat/store.ts').matchAll(/instruction: '([^']+)'/g)].map(m => m[1]);
  assert.equal(rustChat.length, 3);
  assert.deepEqual(nodeChat, rustChat);
});


test('compact chat connection retains authentication bootstrap without the manual tutorial', () => {
  const info = { workspaceName: 'test', endpoint: 'https://example.test/mcp', authType: 'oauth', clientId: 'test-client', password: 'x'.repeat(64), bearerToken: 'x'.repeat(64), folders: ['/test'] };
  for (const locale of ['en', 'zh-CN', 'zh-TW', 'ja']) {
    for (const authType of ['oauth', 'bearer']) {
      const prompt = buildConnectionPrompt({ ...info, authType }, locale, true);
      assert.match(prompt, /User-Agent/);
      assert.match(prompt, /notifications\/initialized/);
      assert.match(prompt, /list_workspace_folders/);
      assert.ok(prompt.length < buildConnectionPrompt({ ...info, authType }, locale).length);
      if (authType === 'oauth') assert.ok(prompt.includes(MANUAL_OAUTH_REDIRECT_URI));
      else assert.doesNotMatch(prompt, /redirect_uri/);
    }
  }
});

test('generated Node skill matches the single packaged Markdown source', () => {
  const read = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8').replaceAll('\r\n', '\n');
  const generated = read('packages/node-agent/src/rustCatalog.generated.ts').match(/export const localChatSkill = (\{[\s\S]*?\}) as const;/);
  assert.ok(generated);
  const skill = JSON.parse(generated[1]);
  assert.equal(skill.text, read('skills/local-chat/SKILL.md'));
  assert.equal(skill.uri, 'coding-tools://skills/local-chat');
  assert.match(read('src-tauri/src/tools/chat.rs'), /include_str!\("\.\.\/\.\.\/\.\.\/skills\/local-chat\/SKILL.md"\)/);
});
