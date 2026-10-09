import { randomUUID, createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { redactSensitiveText } from '../redaction.js';

// The on-disk v1 contract is shared with tools/chat.rs. Never persist waiting=true.
export interface ChatFile { id: string; name: string; path: string; mime: string; size: number; sha256: string }
export interface ToolEvent { name: string; status: 'running' | 'completed' | 'failed'; input?: string; output?: string; output_truncated?: boolean }
export interface ChatMessage { awaiting_user?: boolean; received_at?: number; attachments?: ChatFile[]; tool_event?: ToolEvent; id: string; role: 'user' | 'assistant'; text: string; created_at: number; reply_to?: string; final?: boolean }
export interface ChatSession { title_custom?: boolean; files?: ChatFile[]; version: 1; id: string; title: string; created_at: number; updated_at: number; closed: boolean; messages: ChatMessage[]; attachment_id: string; lease_until: number }
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
function userMessageState(s: ChatSession, message: ChatMessage): string {
  const replies = s.messages.filter(reply => reply.role === 'assistant' && reply.reply_to === message.id);
  const read = !!message.received_at || replies.length > 0;
  const final = replies.find(reply => reply.final === true);
  let status = s.closed ? '会话已结束' : read ? '正在处理' : '排队中';
  if (final) {
    const answered = s.messages.slice(s.messages.indexOf(final) + 1).some(next => next.role === 'user');
    status = final.awaiting_user && !answered ? s.closed ? '会话已结束' : '待确认' : '已回复';
  }
  return `消息状态：${read ? '已读' : '未读'} · ${status}`;
}
export function chatMarkdown(s: ChatSession): string {
  return `# ${s.title}\n\nSession: ${s.id}\n\n` + s.messages.map(m => `## ${m.role === 'user' ? '你' : m.final === false ? 'AI · 进度' : 'AI'} · ${m.created_at}\n\n${m.tool_event ? `Tool (AI reported): ${m.tool_event.name} · ${m.tool_event.status}\n\n` : ''}${m.text}\n${m.role === 'user' ? `\n${userMessageState(s, m)}\n` : ''}${m.role === 'assistant' ? `\nReply state: ${m.awaiting_user ? 'awaiting_user' : m.final === false ? 'supplementing' : 'complete'}\n` : ''}${m.tool_event?.input ? `\nInput:\n${m.tool_event.input}\n` : ''}${m.tool_event?.output ? `\nOutput:\n${m.tool_event.output}\n` : ''}${m.tool_event?.output_truncated ? '\nOutput truncated\n' : ''}${(m.attachments ?? []).map(f => `\nAttachment: ${f.name} (${f.size} bytes)\nPath: ${f.path}\n`).join('')}`).join('\n');
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
  // Other processes may hold the lock briefly; never steal or remove their lock.
  const deadline = Date.now() + 2000;
  while (true) {
    try { mkdirSync(lock); break; }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
      if (Date.now() >= deadline) throw new Error('Chat storage is busy; retry shortly. If it persists, stop all clients before inspecting docs/chat-sessions/.lock');
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20);
    }
  }
  try { return callback(); } finally { rmSync(lock, { recursive: true }); }
}
function key(root: string, id: string): string { return `${realpathSync(root)}:${id}`; }
function pending(s: ChatSession): ChatMessage | undefined {
  return s.messages.find(m => m.role === 'user' && !s.messages.some(r => r.role === 'assistant' && r.reply_to === m.id && r.final));
}
function view(root: string, s: ChatSession) {
  const message = s.closed ? undefined : pending(s);
  const work_state = !message ? null : message.received_at || s.messages.some(r => r.role === 'assistant' && r.reply_to === message.id) ? 'processing' : 'queued';
  return { work_state, id: s.id, title: s.title, created_at: s.created_at, updated_at: s.updated_at, closed: s.closed, messages: s.messages, assistant_message_count: s.messages.filter(m => m.role === 'assistant').length,
    status: s.closed ? 'closed' : waiters.has(key(root, s.id)) ? 'waiting' : s.lease_until > Date.now() ? 'connected' : 'offline',
    archive_path: `${DIR}/${s.id}.md` };
}
const MAX_FILE_BYTES = 2 * 1024 * 1024;
function fileMime(bytes: Buffer): string {
  if (bytes.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10]))) return 'image/png';
  if (bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255) return 'image/jpeg';
  if (['GIF87a','GIF89a'].includes(bytes.subarray(0, 6).toString())) return 'image/gif';
  if (bytes.subarray(0, 4).toString() === 'RIFF' && bytes.subarray(8, 12).toString() === 'WEBP') return 'image/webp';
  return 'application/octet-stream';
}
function filePath(s: ChatSession, f: ChatFile): string {
  const ext = ({'image/png':'png','image/jpeg':'jpg','image/gif':'gif','image/webp':'webp'} as Record<string,string>)[f.mime] ?? 'bin';
  return `${DIR}/${validId(s.id)}-${validId(f.id)}.${ext}`;
}
function upload(root: string, s: ChatSession, args: Record<string, unknown>): ChatFile {
  if (s.closed) throw new Error('Conversation is closed');
  const id = validId(args.upload_id);
  const name = text(args.name, 240);
  if (/[\r\n\x00-\x1f\x7f\/\\]/.test(name)) throw new Error('Invalid attachment name');
  const encoded = args.data_base64;
  if (typeof encoded !== 'string' || encoded.length > 2796204 || encoded.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) throw new Error('Invalid attachment encoding or size');
  const bytes = Buffer.from(encoded, 'base64');
  if (bytes.toString('base64') !== encoded) throw new Error('Invalid attachment encoding or size');
  if (!bytes.length || bytes.length > MAX_FILE_BYTES) throw new Error('Attachment must contain 1–2097152 bytes');
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  const files = s.files ??= [];
  const existing = files.find(f => f.id === id);
  if (existing) {
    if (existing.sha256 !== sha256 || existing.name !== name) throw new Error('Attachment ID conflict');
    save(root, s); return existing;
  }
  if (files.length >= 32 || files.reduce((sum,f) => sum + f.size, 0) + bytes.length > 32 * 1024 * 1024) throw new Error('Session attachment limit reached');
  const f: ChatFile = {id,name,mime:fileMime(bytes),size:bytes.length,sha256,path:''};
  f.path = filePath(s, f);
  const target = safe(root, f.path);
  // An interrupted upload may leave its immutable bytes; same-ID retry may reuse them.
  if (existsSync(target)) {
    if (statSync(target).size !== bytes.length || !readFileSync(target).equals(bytes)) throw new Error('Attachment ID conflict');
  } else writeFileSync(target, bytes, {mode:0o600,flag:'wx'});
  files.push(f); save(root, s); return f;
}
function messageFiles(s: ChatSession, ids: unknown): ChatFile[] {
  if (ids === undefined) return [];
  if (!Array.isArray(ids) || ids.length > 5 || new Set(ids).size !== ids.length) throw new Error('Select up to 5 unique attachments');
  return ids.map(id => {
    const f = s.files?.find(f => f.id === validId(id));
    if (!f) throw new Error('Attachment does not belong to this conversation');
    return f;
  });
}
function toolEvent(value: unknown, final: boolean): ToolEvent | undefined {
  if (value === undefined) return undefined;
  const event = value as Record<string, unknown>;
  if (!event || typeof event !== 'object' || final || !['running','completed','failed'].includes(String(event.status))) throw new Error('Tool events require final=false and a valid status');
  const result: ToolEvent = { name: text(event.name, 120), status: event.status as ToolEvent['status'] };
  if (event.input !== undefined) result.input = text(event.input, 8000);
  if (event.output !== undefined) result.output = text(event.output, 16000);
  if (event.output_truncated !== undefined) {
    if (typeof event.output_truncated !== 'boolean') throw new Error('output_truncated must be a boolean');
    if (event.output_truncated) result.output_truncated = true;
  }
  return result;
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
    if (action === 'upload') return { attachment: upload(root, s, args) };
    if (action === 'read_attachment') {
      const f = messageFiles(s, [args.upload_id])[0];
      const target = safe(root, filePath(s, f));
      if (statSync(target).size > MAX_FILE_BYTES) throw new Error('Attachment exceeds size limit');
      const bytes = readFileSync(target);
      if (createHash('sha256').update(bytes).digest('hex') !== f.sha256) throw new Error('Attachment content changed');
      return { attachment: f, data_base64: bytes.toString('base64') };
    }
    if (action === 'send') {
      if (s.closed) throw new Error('Conversation is closed');
      const id = validId(args.message_id); const attachments = messageFiles(s, args.attachment_ids); const content = text(args.text || (attachments.length ? '📎' : ''));
      const existing = s.messages.find(m => m.id === id);
      if (existing && (existing.role !== 'user' || existing.text !== content || JSON.stringify(existing.attachments ?? []) !== JSON.stringify(attachments))) throw new Error('Message ID conflicts with an existing message');
      if (!existing) { if (!s.messages.length && !s.title_custom) s.title = [...content.replace(/\s+/g, ' ')].slice(0, 36).join(''); s.messages.push({ id, role: 'user', text: content, attachments, created_at: Date.now() }); s.updated_at = Date.now(); }
      save(root, s);
    } else if (action === 'rename') { s.title = text(args.title, 240).replace(/\s+/gu, ' '); s.title_custom = true; s.updated_at = Date.now(); save(root, s); }
    else if (action === 'detach') { s.attachment_id = ''; s.lease_until = 0; s.updated_at = Date.now(); save(root, s); }
    else if (action === 'close') { s.closed = true; s.attachment_id = ''; s.lease_until = 0; s.updated_at = Date.now(); save(root, s); }
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
      return { ok: true, attachment_id: s.attachment_id, session: view(root, s), instruction: 'Use chat_reply for all user-visible replies, questions, progress and deliverables. Call chat_wait now with the returned attachment_id. Use final=false for progress and tool_event with actual name/status/input/output for tool calls; report results only after execution, redact secrets and mark shortened output. Use final=true to acknowledge reply_to; add awaiting_user=true for a question, then wait for the next message. Keep unique reply IDs and identical retry payloads; require persisted=true. Inspect records before repeating side effects. Immediately call chat_wait after idle or a persisted final reply; never stop voluntarily for idle duration, task completion or cost. Stop only on user cancellation, closed session, actual host limits or a blocking chat configuration error. Renew via chat_open with the same attachment_id before the 10-minute lease expires.' };
    }
    owned(s, args.attachment_id);
    if (name === 'chat_upload') {
      const encoded = args.data_base64;
      if (typeof encoded === 'string' && (encoded.length > 699052 || Buffer.from(encoded, 'base64').length > 512 * 1024)) throw new Error('MCP attachment must not exceed 512 KiB; compress it before upload');
      s.lease_until = Date.now() + LEASE_MS; return { ok: true, attachment: upload(root, s, args) };
    }
    if (name === 'chat_reply') {
      const id = validId(args.message_id); const replyTo = validId(args.reply_to); const content = text(args.text);
      const attachments = messageFiles(s, args.attachment_ids);
      const final = args.final !== false; const tool_event = toolEvent(args.tool_event, final);
      if (args.awaiting_user !== undefined && typeof args.awaiting_user !== 'boolean') throw new Error('awaiting_user must be a boolean');
      const awaiting_user = args.awaiting_user === true;
      if (awaiting_user && !final) throw new Error('awaiting_user requires final=true');
      const existing = s.messages.find(m => m.id === id);
      if (existing) {
        if (JSON.stringify(existing.attachments ?? []) !== JSON.stringify(attachments) || existing.role !== 'assistant' || existing.text !== content || existing.reply_to !== replyTo || existing.final !== final || (existing.awaiting_user === true) !== awaiting_user || JSON.stringify(existing.tool_event ?? undefined) !== JSON.stringify(tool_event)) throw new Error('Message ID conflicts with an existing reply');
      } else {
        if (pending(s)?.id !== replyTo) throw new Error('Reply must address the oldest unanswered user message');
        s.messages.push({ id, role: 'assistant', text: content, reply_to: replyTo, final, awaiting_user, tool_event, attachments, created_at: Date.now() }); s.updated_at = Date.now();
      }
      s.lease_until = Date.now() + LEASE_MS; save(root, s); return { ok: true, persisted: true, message_id: id };
    }
    if (name === 'chat_close') { s.closed = true; s.attachment_id = ''; s.lease_until = 0; save(root, s); return { ok: true, status: 'closed' }; }
    if (name === 'chat_wait') {
      s.lease_until = Date.now() + LEASE_MS;
      const message = pending(s);
      if (message && !message.received_at) {
        message.received_at = Date.now();
        s.updated_at = message.received_at;
      }
      save(root, s);
      return { ok: true, status: message ? 'message' : 'idle', message: message ?? null };
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
      if (message) return chatTool(root, 'chat_wait', args);
      await sleep(Math.min(250, end - Date.now()), undefined, { signal });
    }
    return { ok: true, status: 'idle', instruction: 'No message yet. Immediately call chat_wait again with the same session and attachment. Idle timeout does not end the conversation; do not stop for idle duration or cost.' };
  } finally { waiters.delete(k); }
}
