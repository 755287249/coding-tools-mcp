import assert from 'node:assert/strict';
import test from 'node:test';
import { buildChatPrompt } from '../src/lib/connect/chat-prompt.ts';
import { buildConnectionPrompt } from '../src/lib/connect/prompt.ts';

test('session target round-trips quotes and newlines as one JSON line', () => {
  const chat = 'session-"\n1', folder = 'folder-\\"\n2';
  const prompt = buildChatPrompt(chat, folder);
  const target = prompt.split('\n').find(line => line.startsWith('目标参数'));
  assert.deepEqual(JSON.parse(target.slice(target.indexOf('{'))), { chat_id: chat, workspace_folder_id: folder });
  assert.match(prompt, /persisted=true/);
  assert.match(prompt, /status=idle/);
  assert.match(prompt, /最多连续重试 3 次/);
  assert.match(prompt, /原 message_id 和原正文/);
  assert.match(prompt, /不能抢占/);
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
    assert.doesNotMatch(connection, /connect manually over HTTP|用 HTTP 手动连接/);
  }
});
