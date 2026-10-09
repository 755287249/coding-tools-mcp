import assert from 'node:assert/strict';
import test from 'node:test';
import { buildChatPrompt } from '../src/lib/connect/chat-prompt.ts';
import { buildConnectionPrompt, buildClientConfigJson, MANUAL_OAUTH_REDIRECT_URI } from '../src/lib/connect/prompt.ts';

test('session target round-trips quotes and newlines as one JSON line', () => {
  const chat = 'session-"\n1', folder = 'folder-\\"\n2';
  const prompt = buildChatPrompt(chat, folder);
  const target = prompt.split('\n').find(line => line.startsWith('目标参数'));
  assert.deepEqual(JSON.parse(target.slice(target.indexOf('{'))), { chat_id: chat, workspace_folder_id: folder });
  assert.match(prompt, /所有回复、提问、进度和成果都通过 chat_reply/);
  assert.match(prompt, /awaiting_user:true/);
  assert.match(prompt, /首次成功立即保存实际 attachment_id/);
  assert.match(prompt, /每次 bash\/工作工具调用前后各发 tool_event/);
  assert.match(prompt, /persisted=true/);
  assert.match(prompt, /status=idle/);
  assert.ok(prompt.length < 1120, 'session instructions should stay concise');
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
  assert.match(prompt, /未经用户明确同意，不调用转交其他模型的生图\/语音工具/);
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
