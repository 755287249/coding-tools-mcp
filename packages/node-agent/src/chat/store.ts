import { randomUUID } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { redactSensitiveText } from '../redaction.js';

// The on-disk v1 contract is shared with tools/chat.rs. Never persist waiting=true.
export interface ChatMessage { id: string; role: 'user' | 'assistant'; text: string; created_at: number; reply_to?: string; final?: boolean }
export interface ChatSession { version: 1; id: string; title: string; created_at: number; updated_at: number; closed: boolean; messages: ChatMessage[]; attachment_id: string; lease_until: number }
const DIR = 'docs/chat-sessions';
const MAX_BYTES = 2 * 1024 * 1024;
const LEASE_MS = 10 * 60_000;
const waiters = new Set<string>();
const validId = (id: unknown): string => { if (typeof id !== 'string' || !/^[a-zA-Z0-9_-]{1,80}$/.test(id)) throw new Error('Invalid chat/message ID'); return id; };
function text(value: unknown, max = 32000): string {
  if (typeof value !== 'string' || !value.trim() || Buffer.byteLength(value) > max) throw new Error(`Text must contain 1–${max} bytes`);
  return redactSensitiveText(value.trim()).value;
}
function safe(root: string, relative: string): string {
  const base = realpathSync(root);
  let current = base;
  for (const part of relative.split('/')) {
    current = path.join(current, part);
    if (existsSync(current) || (() => { try { return lstatSync(current).isSymbolicLink(); } catch { return false; } })()) {
      if (lstatSync(current).isSymbolicLink()) throw new Error('Chat storage must not contain symlinks');
    }
  }
  return current;
}
function file(root: string, id: string): string { return safe(root, `${DIR}/${validId(id)}.json`); }
function load(root: string, id: string): ChatSession {
  const target = file(root, id);
  if (statSync(target).size > MAX_BYTES) throw new Error('Chat archive exceeds size limit');
  const s = JSON.parse(readFileSync(target, 'utf8')) as ChatSession;
  if (s.version !== 1 || s.id !== id || !Array.isArray(s.messages)) throw new Error('Invalid chat archive');
  return s;
}
export function chatMarkdown(s: ChatSession): string {
  return `# ${s.title}\n\nSession: ${s.id}\n\n` + s.messages.map(m => `## ${m.role === 'user' ? '你' : m.final === false ? 'AI · 进度' : 'AI'} · ${m.created_at}\n\n${m.text}\n`).join('\n');
}
function save(root: string, s: ChatSession): void {
  const body = JSON.stringify(s, null, 2);
  if (Buffer.byteLength(body) > MAX_BYTES || s.messages.length > 500) throw new Error('Session is full; start a new conversation');
  const target = file(root, s.id);
  const temporary = safe(root, `${DIR}/${s.id}.${randomUUID()}.tmp`);
  writeFileSync(temporary, body, { mode: 0o600, flag: 'wx' });
  try { renameSync(temporary, target); } finally { rmSync(temporary, { force: true }); }
  // JSON is authoritative. An export failure is surfaced; retries rebuild the projection.
  const md = safe(root, `${DIR}/${s.id}.md`);
  const mdTemp = safe(root, `${DIR}/${s.id}.${randomUUID()}.tmp`);
  try { writeFileSync(mdTemp, chatMarkdown(s), { mode: 0o600, flag: 'wx' }); renameSync(mdTemp, md); }
  finally { rmSync(mdTemp, { force: true }); }
}
function locked<T>(root: string, callback: () => T): T {
  const directory = safe(root, DIR);
  mkdirSync(directory, { recursive: true });
  const lock = safe(root, `${DIR}/.lock`);
  // Fail closed on contention, including a stale lock left by a crashed writer.
  mkdirSync(lock);
  try { return callback(); } finally { rmSync(lock, { recursive: true }); }
}
function key(root: string, id: string): string { return `${realpathSync(root)}:${id}`; }
function pending(s: ChatSession): ChatMessage | undefined {
  return s.messages.find(m => m.role === 'user' && !s.messages.some(r => r.role === 'assistant' && r.reply_to === m.id && r.final));
}
function view(root: string, s: ChatSession) {
  return { id: s.id, title: s.title, created_at: s.created_at, updated_at: s.updated_at, closed: s.closed, messages: s.messages,
    status: s.closed ? 'closed' : waiters.has(key(root, s.id)) ? 'waiting' : s.lease_until > Date.now() ? 'connected' : 'offline',
    archive_path: `${DIR}/${s.id}.md` };
}
export function chatUi(root: string, args: Record<string, unknown>): Record<string, unknown> {
  return locked(root, () => {
    const action = args.action;
    if (action === 'list') {
      const sessions = readdirSync(safe(root, DIR)).filter(n => /^[a-zA-Z0-9_-]{1,80}\.json$/.test(n)).map(n => {
        const s = view(root, load(root, n.slice(0, -5))); return { ...s, messages: undefined };
      }).sort((a, b) => b.updated_at - a.updated_at);
      return { sessions };
    }
    if (action === 'create') {
      const now = Date.now(); const s: ChatSession = { version: 1, id: randomUUID(), title: text(args.title ?? '新对话', 240), created_at: now, updated_at: now, closed: false, messages: [], attachment_id: '', lease_until: 0 };
      save(root, s); return { session: view(root, s) };
    }
    const s = load(root, validId(args.chat_id));
    if (action === 'send') {
      if (s.closed) throw new Error('Conversation is closed');
      const id = validId(args.message_id); const content = text(args.text);
      const existing = s.messages.find(m => m.id === id);
      if (existing && (existing.role !== 'user' || existing.text !== content)) throw new Error('Message ID conflicts with an existing message');
      if (!existing) { if (!s.messages.length) s.title = [...content.replace(/\s+/g, ' ')].slice(0, 36).join(''); s.messages.push({ id, role: 'user', text: content, created_at: Date.now() }); s.updated_at = Date.now(); }
      save(root, s);
    } else if (action === 'close') { s.closed = true; s.attachment_id = ''; s.lease_until = 0; s.updated_at = Date.now(); save(root, s); }
    else if (action !== 'read') throw new Error('Unknown chat action');
    return { session: view(root, s) };
  });
}
function owned(s: ChatSession, attachment: unknown): void {
  if (!attachment || attachment !== s.attachment_id || s.lease_until <= Date.now()) throw new Error('Chat attachment expired; call chat_open again');
}
export function chatTool(root: string, name: string, args: Record<string, unknown>): Record<string, unknown> {
  return locked(root, () => {
    const s = load(root, validId(args.chat_id));
    if (s.closed) return { ok: true, status: 'closed' };
    if (name === 'chat_open') {
      if (s.lease_until > Date.now() && args.attachment_id !== s.attachment_id) throw new Error('Conversation already attached; close it in the UI or wait for the lease to expire');
      if (s.lease_until <= Date.now()) s.attachment_id = randomUUID();
      s.lease_until = Date.now() + LEASE_MS; save(root, s);
      return { ok: true, attachment_id: s.attachment_id, session: view(root, s), instruction: 'Call chat_wait. Publish progress and complete replies with chat_reply; final=true acknowledges reply_to. Then wait again until closed. Redelivered messages may have unfinished work: inspect before repeating side effects.' };
    }
    owned(s, args.attachment_id);
    if (name === 'chat_reply') {
      const id = validId(args.message_id); const replyTo = validId(args.reply_to); const content = text(args.text);
      const final = args.final !== false;
      const existing = s.messages.find(m => m.id === id);
      if (existing) {
        if (existing.role !== 'assistant' || existing.text !== content || existing.reply_to !== replyTo || existing.final !== final) throw new Error('Message ID conflicts with an existing reply');
      } else {
        if (pending(s)?.id !== replyTo) throw new Error('Reply must address the oldest unanswered user message');
        s.messages.push({ id, role: 'assistant', text: content, reply_to: replyTo, final, created_at: Date.now() }); s.updated_at = Date.now();
      }
      s.lease_until = Date.now() + LEASE_MS; save(root, s); return { ok: true, persisted: true, message_id: id };
    }
    if (name === 'chat_close') { s.closed = true; s.attachment_id = ''; s.lease_until = 0; save(root, s); return { ok: true, status: 'closed' }; }
    if (name === 'chat_wait') {
      s.lease_until = Date.now() + LEASE_MS;
      save(root, s);
      return { ok: true, status: pending(s) ? 'message' : 'idle', message: pending(s) ?? null };
    }
    throw new Error('Unknown chat tool');
  });
}
export async function chatWait(root: string, args: Record<string, unknown>, signal?: AbortSignal): Promise<Record<string, unknown>> {
  const timeout = args.timeout_ms ?? 120000;
  if (typeof timeout !== 'number' || !Number.isInteger(timeout) || timeout < 0 || timeout > 180000) throw new Error('timeout_ms must be 0–180000');
  const initial = chatTool(root, 'chat_wait', args);
  if (initial.status !== 'idle') return initial;
  const id = validId(args.chat_id); const k = key(root, id);
  if (waiters.has(k)) throw new Error('A wait request is already active');
  waiters.add(k);
  try {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      signal?.throwIfAborted();
      const s = load(root, id);
      if (s.closed) return { ok: true, status: 'closed' };
      owned(s, args.attachment_id);
      const message = pending(s);
      if (message) return { ok: true, status: 'message', message };
      await sleep(Math.min(250, end - Date.now()), undefined, { signal });
    }
    return { ok: true, status: 'idle', instruction: 'No message yet. Call chat_wait again unless the user ended the loop.' };
  } finally { waiters.delete(k); }
}
