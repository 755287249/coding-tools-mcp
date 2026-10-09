import assert from 'node:assert/strict';
import test from 'node:test';
import { buildChatPrompt } from '../src/lib/connect/chat-prompt.ts';
import { buildConnectionPrompt } from '../src/lib/connect/prompt.ts';

test('session target round-trips quotes and newlines as one JSON line', () => {
  const chat = 'session-"\n1', folder = 'folder-\\"\n2';
  const prompt = buildChatPrompt(chat, folder);
  const target = prompt.split('\n').find(line => line.startsWith('目标参数'));
  assert.deepEqual(JSON.parse(target.slice(target.indexOf('{'))), { chat_id: chat, workspace_folder_id: folder });
  assert.match(prompt, /所有回复、提问、进度和成果都通过 chat_reply/);
  assert.match(prompt, /awaiting_user:true/);
  assert.match(prompt, /旧 schema 不支持/);
  assert.match(prompt, /persisted=true/);
  assert.match(prompt, /status=idle/);
  assert.ok(prompt.length < 1800, 'session instructions should stay concise');
  assert.match(prompt, /不设次数上限/);
  assert.match(prompt, /此后每 30 秒重试/);
  assert.doesNotMatch(prompt, /最多连续重试 3 次/);
  assert.match(prompt, /status=idle 后立即再次调用，无论多少次都继续/);
  assert.match(prompt, /final=true 只确认本条消息，不结束会话/);
  assert.match(prompt, /只要聊天可用就继续等待/);
  assert.match(prompt, /不计等待成本/);
  assert.match(prompt, /宿主硬性上限实际触发/);
  assert.match(prompt, /重试保留全部原参数/);
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
