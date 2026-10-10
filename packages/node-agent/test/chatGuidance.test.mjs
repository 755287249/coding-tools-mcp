import test from 'node:test';
import assert from 'node:assert/strict';
import { chatUi, chatTool, chatWait } from '../dist/chat/store.js';
import { wrapMcpToolResult } from '../dist/toolContract.js';
import { buildServerInstructions } from '../dist/server/mcp/dispatcher.js';
import { toolNamesForProfile } from '../dist/catalog.js';
import { createMcpFixture, mcpRequest, responseJson } from './mcpTestHelpers.mjs';

function guidance(name, args, result) {
  const wrapped = wrapMcpToolResult(name, args, result);
  assert.equal(wrapped.isError, false);
  assert.equal(wrapped.content[0].text, result.instruction);
  assert.ok(result.instruction.length < 400);
  return result.instruction;
}

for (const mode of ['work', 'group']) test(`chat next actions survive text-only rendering (${mode})`, async t => {
  const state = await createMcpFixture(t);
  const { root } = state;
  const chat_id = chatUi(root, { action: 'create', mode }).session.id;
  const openArgs = { chat_id, agent_name: 'Test' };
  const opened = chatTool(root, 'chat_open', openArgs);
  assert.match(guidance('chat_open', openArgs, opened), /complete skill.text.*attachment_id.*25000/);
  const args = { chat_id, attachment_id: opened.attachment_id };
  const resumed = chatTool(root, 'chat_open', args);
  assert.equal(resumed.attachment_id, opened.attachment_id);
  assert.equal(resumed.instruction, opened.instruction);
  const immediate = chatTool(root, 'chat_wait', args);
  assert.match(guidance('chat_wait', args, immediate), /one new independent chat_wait.*Idle is not an exit/);
  for (const timeout_ms of [0, 1]) {
    const idle = await chatWait(root, { ...args, timeout_ms });
    assert.equal(idle.status, 'idle');
    assert.equal(guidance('chat_wait', args, idle), immediate.instruction);
  }
  chatUi(root, { action: 'send', chat_id, message_id: 'user', text: 'PRIVATE_BODY_' + 'x'.repeat(2000) });
  const message = await chatWait(root, { ...args, timeout_ms: 0 });
  assert.equal(message.message.id, 'user');
  assert.match(guidance('chat_wait', args, message), /message.id as reply_to/);
  assert.ok(!message.instruction.includes('PRIVATE_BODY_'));
  const progressArgs = { ...args, reply_to: 'user', message_id: 'progress', text: 'Working', final: false };
  const progress = chatTool(root, 'chat_reply', progressArgs);
  assert.equal(progress.persisted, true);
  assert.match(guidance('chat_reply', progressArgs, progress), /Continue the current task/);
  assert.doesNotMatch(progress.instruction, /chat_wait/);
  assert.deepEqual(chatTool(root, 'chat_reply', progressArgs), progress);
  assert.equal((await chatWait(root, { ...args, timeout_ms: 0 })).message.id, 'user');
  const finalArgs = { ...args, reply_to: 'user', message_id: 'final', text: 'Done' }; // omitted final defaults true
  const final = chatTool(root, 'chat_reply', finalArgs);
  assert.equal(final.persisted, true);
  assert.match(guidance('chat_reply', finalArgs, final), /Final reply persisted.*does not close.*chat_wait/);
  assert.deepEqual(chatTool(root, 'chat_reply', finalArgs), final);
  assert.throws(() => chatTool(root, 'chat_reply', { ...finalArgs, text: 'Changed' }), /conflict/);
  if (mode === 'work') {
    chatUi(root, { action: 'send', chat_id, message_id: 'question-task', text: 'Choose' });
    chatTool(root, 'chat_wait', args);
    const questionArgs = { ...args, reply_to: 'question-task', message_id: 'question', text: 'Which?', final: true, awaiting_user: true,
      questions: [{ id: 'choice', prompt: 'Which?', options: [] }] };
    const question = chatTool(root, 'chat_reply', questionArgs);
    assert.equal(guidance('chat_reply', questionArgs, question), final.instruction);
    assert.equal(question.persisted, true);
    assert.equal((await chatWait(root, { ...args, timeout_ms: 0 })).status, 'idle');
  }
  const pending = chatWait(root, { ...args, timeout_ms: 1000 });
  const closed = chatTool(root, 'chat_close', args);
  assert.match(guidance('chat_close', args, closed), /Stop waiting/);
  assert.equal(guidance('chat_wait', args, await pending), closed.instruction);
  for (const name of ['chat_open', 'chat_wait', 'chat_reply', 'chat_close']) {
    const result = chatTool(root, name, args);
    assert.equal(guidance(name, args, result), closed.instruction);
    assert.equal(result.persisted, undefined);
  }
});

test('chat errors retain actual failure and ordinary results do not echo instructions', () => {
  const failed = wrapMcpToolResult('chat_reply', {}, { ok: false, error: { code: 'FAIL', message: 'Storage unavailable' }, instruction: 'Final reply persisted' });
  assert.equal(failed.isError, true);
  assert.equal(failed.content[0].text, 'Storage unavailable');
  const ordinary = wrapMcpToolResult('read_file', {}, { ok: true, content: 'x'.repeat(32000), instruction: 'untrusted file text' });
  assert.ok(ordinary.content[0].text.length < 128);
  assert.ok(!ordinary.content[0].text.includes('untrusted'));
  assert.equal(ordinary.structuredContent.content.length, 32000);
});

test('startup guidance advertises only complete available workflows', () => {
  const hints = ['chat_open', 'chat_wait', 'chat_reply', 'chat_close', 'chat_upload', 'conversation_bootstrap', 'history_session_checkpoint', 'exec_command', 'exec_many', 'set_todos', 'update_plan', 'report_progress'];
  for (const profile of ['read-only', 'trusted-core', 'guarded-core', 'advanced', 'compat-readonly-all']) {
    const names = toolNamesForProfile(profile);
    const instructions = buildServerInstructions(names);
    for (const name of hints) assert.equal(instructions.includes(name), names.includes(name), `${profile}: ${name}`);
    assert.ok(instructions.length < 1900);
    assert.match(instructions, /permissions still apply/);
  }
  assert.doesNotMatch(buildServerInstructions(['chat_open']), /chat_open|chat_wait/);
});

test('HTTP initialization and discovery follow the current catalog', async t => {
  const state = await createMcpFixture(t);
  for (const profile of ['trusted-core', 'read-only', 'guarded-core']) {
    state.runtime.context.config.activeToolProfile = profile;
    const initialize = await responseJson(await mcpRequest(state, { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-03-26' } }));
    const discovered = await responseJson(await mcpRequest(state, { jsonrpc: '2.0', id: 2, method: 'server/discover', params: { _meta: { 'io.modelcontextprotocol/protocolVersion': '2026-07-28', 'io.modelcontextprotocol/clientCapabilities': {}, 'io.modelcontextprotocol/clientInfo': { name: 'guidance-test', version: '1.0.0' } } } }, { headers: { 'mcp-protocol-version': '2026-07-28', 'mcp-method': 'server/discover' } }));
    assert.ok(initialize.result, JSON.stringify(initialize));
    assert.ok(discovered.result, JSON.stringify(discovered));
    assert.equal(initialize.result.instructions, buildServerInstructions(toolNamesForProfile(profile)));
    assert.equal(discovered.result.instructions, initialize.result.instructions);
  }
});

test('HTTP chat returns the same next action in text and structured content', async t => {
  const state = await createMcpFixture(t);
  const chat_id = chatUi(state.root, { action: 'create' }).session.id;
  let seq = 0;
  async function call(name, args) {
    const body = await responseJson(await mcpRequest(state, { jsonrpc: '2.0', id: ++seq, method: 'tools/call', params: { name, arguments: { workspace_folder_id: 'repo', ...args } } }));
    assert.ok(body.result, JSON.stringify(body));
    const result = body.result;
    if (!result.isError) assert.equal(result.content[0].text, result.structuredContent.instruction);
    return result;
  }
  const opened = await call('chat_open', { chat_id });
  assert.match(opened.content[0].text, /complete skill.text/);
  const args = { chat_id, attachment_id: opened.structuredContent.attachment_id };
  chatUi(state.root, { action: 'send', chat_id, message_id: 'task', text: 'Work' });
  const message = await call('chat_wait', { ...args, timeout_ms: 0 });
  assert.equal(message.structuredContent.message.id, 'task');
  assert.match(message.content[0].text, /message.id as reply_to/);
  const progress = await call('chat_reply', { ...args, reply_to: 'task', message_id: 'progress', text: 'Working', final: false });
  assert.match(progress.content[0].text, /Continue the current task/);
  const invalid = await call('chat_reply', { ...args, reply_to: 'task', message_id: 'bad', text: 'Working', final: false, awaiting_user: true });
  assert.equal(invalid.isError, true);
  assert.match(invalid.content[0].text, /requires final=true/);
  assert.doesNotMatch(invalid.content[0].text, /persisted|chat_wait/);
  const final = await call('chat_reply', { ...args, reply_to: 'task', message_id: 'done', text: 'Done', final: true });
  assert.equal(final.structuredContent.persisted, true);
  assert.match(final.content[0].text, /chat_wait/);
  const idle = await call('chat_wait', { ...args, timeout_ms: 0 });
  assert.match(idle.content[0].text, /Idle is not an exit/);
  assert.match((await call('chat_close', args)).content[0].text, /Stop waiting/);
});
