import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

const compile = source => ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const moduleUrl = source => 'data:text/javascript;base64,' + Buffer.from(compile(source)).toString('base64');
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const { chatUserState, pendingChatState, messageAnswered } = await import(moduleUrl(read('../src/lib/chat/status.ts')));

const user = (extra = {}) => ({ id: 'm1', role: 'user', text: 'task', created_at: 1, ...extra });

test('recalled and paused messages are no longer pending work', () => {
  for (const stop of [{ recalled_at: 5 }, { interrupted_at: 5 }]) {
    const session = { status: 'waiting', messages: [user({ received_at: 2, ...stop })] };
    assert.equal(messageAnswered(session, 'm1'), true);
    assert.equal(pendingChatState(session), null);
  }
  assert.equal(chatUserState({ messages: [user({ recalled_at: 5 })] }, 'm1').status, 'recalled');
  assert.equal(chatUserState({ messages: [user({ received_at: 2, interrupted_at: 5 })] }, 'm1').status, 'interrupted');
  const replied = { messages: [user({ received_at: 2, interrupted_at: 5 }), { id: 'r', role: 'assistant', reply_to: 'm1', final: true, text: 'stopped at 2', created_at: 6 }] };
  assert.equal(chatUserState(replied, 'm1').status, 'replied');
});

test('chat panel offers recall, pause, jump queue and a not-delivered badge', () => {
  const panel = read('../src/lib/components/chat/ChatPanel.svelte');
  assert.match(panel, /onclick=\{\(\)=>control\('recall',m\)\}/);
  assert.match(panel, /class="send-button pause-button"[^>]*onclick=\{\(\)=>control\('interrupt',currentTask\)\}/);
  assert.match(panel, /onclick=\{\(\)=>control\('jump_queue',item\)\}/);
  assert.match(panel, /class="undelivered"/);
  assert.match(panel, /const canPause = \$derived\(mode === 'work' && pendingState === 'processing' && !draft\.trim\(\) && !attachments\.length/);
});
