import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import ts from 'typescript';

const compile = source => ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const moduleUrl = source => 'data:text/javascript;base64,' + Buffer.from(compile(source)).toString('base64');
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const store = pathToFileURL(createRequire(import.meta.url).resolve('svelte/store')).href;
const connection = moduleUrl(read('../src/lib/chat/connection.ts'));
const { connectPhase, trackConnectRows, justConnected, clearJustConnected, connectRowKey } = await import(moduleUrl(
  read('../src/lib/chat/connect-progress.ts').replace("'svelte/store'", JSON.stringify(store)).replace("'./connection'", JSON.stringify(connection)),
));
const { get } = await import(store);

const request = { id: 'r', role: 'user', kind: 'connection_request', text: 'connect', created_at: 1 };
const greeting = { id: 'g', role: 'assistant', reply_to: 'r', text: '你好，有什么能帮到你？', final: true, created_at: 2 };

test('an attached AI that has not answered the greeting is still connecting, not connected', () => {
  assert.equal(connectPhase({ status: 'offline', messages: [request] }), 'waiting_ai');
  assert.equal(connectPhase({ status: 'waiting', messages: [request] }), 'greeting');
  assert.equal(connectPhase({ status: 'offline', messages: [{ ...request, received_at: 5 }] }), 'greeting');
  assert.equal(connectPhase({ status: 'waiting', messages: [request, greeting] }), 'none');
  assert.equal(connectPhase({ status: 'waiting', messages: [request, { ...greeting, text: 'hi' }] }), 'failed');
  assert.equal(connectPhase({ status: 'waiting', messages: [] }), 'none');
  assert.equal(connectPhase({ status: 'offline', closed: true, messages: [request] }), 'none');
  assert.equal(connectPhase({ mode: 'group', status: 'offline', messages: [request] }), 'none');
});

test('list rows use the server connecting flag', () => {
  assert.equal(connectPhase({ status: 'offline', connecting: true }), 'waiting_ai');
  assert.equal(connectPhase({ status: 'connected', connecting: true }), 'greeting');
  assert.equal(connectPhase({ status: 'connected', connecting: false }), 'none');
});

test('a row that finishes pairing is highlighted until opened', () => {
  const key = connectRowKey('w', 'f', 'c');
  trackConnectRows([{ key, session: { status: 'offline', connecting: true } }], 1000);
  assert.equal(get(justConnected)[key], undefined);
  trackConnectRows([{ key, session: { status: 'waiting', connecting: false } }], 2000);
  assert.equal(get(justConnected)[key], 2000);
  trackConnectRows([{ key, session: { status: 'waiting', connecting: false } }], 3000);
  assert.equal(get(justConnected)[key], 2000, 'a steady state does not re-trigger');
  clearJustConnected(key);
  assert.equal(get(justConnected)[key], undefined);
  const other = connectRowKey('w', 'f', 'closed');
  trackConnectRows([{ key: other, session: { status: 'offline', connecting: true } }], 4000);
  trackConnectRows([{ key: other, session: { status: 'offline', connecting: false } }], 5000);
  assert.equal(get(justConnected)[other], undefined, 'an AI that left is not reported as connected');
});

test('chat UI: no modal connect dialog, inline panel, accurate empty state and transcript copy', () => {
  const panel = read('../src/lib/components/chat/ChatPanel.svelte');
  assert.doesNotMatch(panel, /<dialog class="connect-dialog"/);
  assert.match(panel, /\{#snippet connectPanel\(\)\}/);
  assert.match(panel, /\{:else if connectingNow\|\|phase==='failed'\}\{@render connectStatus\(\)\}/);
  assert.match(panel, /class="archive-copy"[^>]*onclick=\{copyTranscript\}/);
  const start = read('../src/lib/components/chat/SeedStart.svelte');
  assert.match(start, /<SeedLoading variant=\{choosingDirect\?'direct':'seed'\}\/>/);
  const nav = read('../src/lib/components/chat/ProjectNavigator.svelte');
  assert.match(nav, /class:connecting=/);
  assert.match(nav, /class:just-connected=/);
});
