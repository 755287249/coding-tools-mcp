import {operationsMarkdown, type ChatOperation} from './operations.js';
import * as group from './group.js';
import type {ChatMember} from './group.js';
import {reduceChatPlan, chatPlanSummary, chatPlanMarkdown, type ChatTaskPlan} from './plan.js';
import { localChatSkill } from '../rustCatalog.generated.js';
import { randomUUID, createHash } from 'node:crypto';
import { writeSync, fsyncSync, closeSync, openSync, readSync, fstatSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { redactSensitiveText } from '../redaction.js';
import { resolveChatPath } from './reveal.js';

// The on-disk v1 contract is shared with tools/chat.rs. Never persist waiting=true.
export interface ChatFile { label?: string; local_reference?: boolean; id: string; name: string; path: string; mime: string; size: number; sha256: string }
export interface ToolEvent { name: string; status: 'running' | 'completed' | 'failed'; input?: string; output?: string; output_truncated?: boolean }
export interface ChatMessage { received_by?:string[]; agent_id?:string;agent_name?:string;recipient_ids?:string[];agent_plans?:{agent_id:string;plan:ChatTaskPlan}[]; task_plan?:ChatTaskPlan; kind?: 'connection_request' | 'assignment'; awaiting_user?: boolean; received_at?: number; attachments?: ChatFile[]; tool_event?: ToolEvent; id: string; role: 'user' | 'assistant'; text: string; created_at: number; reply_to?: string; final?: boolean }
export interface ChatSession { work_member?:ChatMember; mode?:'work'|'group';members?:ChatMember[];agent_name?:string; pinned?: boolean; archived?: boolean; queue?: ChatMessage[]; queue_mode?: 'merge' | 'split'; queue_receipts?: Record<string,string>; title_custom?: boolean; files?: ChatFile[]; version: 1|2; id: string; title: string; created_at: number; updated_at: number; closed: boolean; messages: ChatMessage[]; attachment_id: string; lease_until: number }
const DIR = 'docs/chat-sessions';
const ASSET_DIR = 'mcp-assistant/chat-assets';
const ARTIFACT_DIR = 'mcp-assistant/artifacts/';
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
  if ((s.version !== 1 && s.version !== 2) || s.id !== id || !Array.isArray(s.messages)) throw new Error('Invalid chat archive');
  labelFiles(s);
  return s;
}
function userMessageState(s: ChatSession, message: ChatMessage): string {
  const replies = s.messages.filter(reply => reply.role === 'assistant' && reply.reply_to === message.id);
  const read = !!message.received_at || replies.length > 0;
  const final = replies.filter(reply => reply.final === true).at(-1);
  let status = s.closed ? '会话已结束' : read ? '正在处理' : '排队中';
  if (final && group.taskComplete(s,message)) {
    const answered = s.messages.slice(s.messages.indexOf(final) + 1).some(next => next.role === 'user');
    status = final.awaiting_user && !answered ? s.closed ? '会话已结束' : '待确认' : '已回复';
  }
  return `消息状态：${read ? '已读' : '未读'} · ${status}`;
}
export function chatMarkdown(s: ChatSession): string {
  return `# ${s.title}\n\nSession: ${s.id}\n\n` + group.groupMarkdown(s) + s.messages.map(m => `## ${m.kind === 'connection_request' ? '接入请求' : m.role === 'user' ? '你' : m.final === false ? 'AI · 进度' : 'AI'} · ${m.created_at}\n\n${m.tool_event ? `Tool (AI reported): ${m.tool_event.name} · ${m.tool_event.status}\n\n` : ''}${m.text}\n${group.groupMarkdown(s,m)}${chatPlanMarkdown(m.task_plan)}${m.role === 'user' ? `\n${userMessageState(s, m)}\n` : ''}${m.role === 'assistant' ? `\nReply state: ${m.awaiting_user ? 'awaiting_user' : m.final === false ? 'supplementing' : 'complete'}\n` : ''}${m.tool_event?.input ? `\nInput:\n${m.tool_event.input}\n` : ''}${m.tool_event?.output ? `\nOutput:\n${m.tool_event.output}\n` : ''}${m.tool_event?.output_truncated ? '\nOutput truncated\n' : ''}${(m.attachments ?? []).map(f => `\nAttachment: ${f.name} (${f.size} bytes)\nPath: ${f.path}\n${f.label ? `Reference: @${f.label}\n` : ''}`).join('')}`).join('\n') + queuedMarkdown(s);
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
function pending(s: ChatSession, attachment?:unknown,waiting=false): ChatMessage | undefined {
  if(group.grouped(s))return group.groupPending(s,attachment?group.memberFor(s,attachment).id:undefined,waiting);
  return s.messages.find(m => m.role === 'user' && !s.messages.some(r => r.role === 'assistant' && r.reply_to === m.id && r.final));
}
function view(root: string, s: ChatSession) {
  const message = s.closed ? undefined : pending(s);
  const work_state = !message ? null : message.received_at || s.messages.some(r => r.role === 'assistant' && r.reply_to === message.id) ? 'processing' : 'queued';
  const members=group.members(s).map(m=>({id:m.id,name:m.name,role:m.role,paused:m.paused===true,status:s.closed?'offline':waiters.has(key(root,s.id)+':'+m.attachment_id)?'waiting':m.lease_until>Date.now()?'connected':'offline'}));
  const presence=members.some(m=>m.status==='waiting')?'waiting':members.some(m=>m.status==='connected')?'connected':'offline';
  return { mode:s.mode??'work',members,agent_name:s.agent_name, pinned:s.pinned===true, archived:s.archived===true, work_state, id: s.id, title: s.title, created_at: s.created_at, updated_at: s.updated_at, closed: s.closed, messages: s.messages, assistant_message_count: s.messages.filter(m => m.role === 'assistant').length,
    status: s.closed ? 'closed' : group.grouped(s)?presence:waiters.has(key(root, s.id)+':'+s.attachment_id) ? 'waiting' : s.lease_until > Date.now() ? 'connected' : 'offline',
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
function referencePath(value: unknown): string {
  if (typeof value !== 'string' || !value.startsWith(ARTIFACT_DIR) || value.length > 4096 || /[\\:\x00-\x1f\x7f]/.test(value) || value.split('/').some(p => !p || p === '.' || p === '..')) throw new Error('Local files must be inside mcp-assistant/artifacts');
  return value;
}
function fingerprint(target: string): {sha256: string; size: number; mime: string} {
  if (!statSync(target).isFile()) throw new Error('Attachment must be a regular file');
  const fd = openSync(target, 'r');
  try {
    if (!fstatSync(fd).isFile()) throw new Error('Attachment must be a regular file');
    const buffer = Buffer.alloc(64 * 1024), hash = createHash('sha256');
    let size = 0, mime = 'application/octet-stream', count: number;
    while ((count = readSync(fd, buffer, 0, buffer.length, null)) > 0) {
      if (!size) mime = fileMime(buffer.subarray(0, count));
      hash.update(buffer.subarray(0, count)); size += count;
    }
    if (!size) throw new Error('Attachment must not be empty');
    return {sha256: hash.digest('hex'), size, mime};
  } finally { closeSync(fd); }
}
function filePath(s: ChatSession, f: ChatFile): string {
  if (f.local_reference) return referencePath(f.path);
  const ext = ({'image/png':'png','image/jpeg':'jpg','image/gif':'gif','image/webp':'webp'} as Record<string,string>)[f.mime] ?? 'bin';
  const legacy = `${DIR}/${validId(s.id)}-${validId(f.id)}.${ext}`;
  const current = `${ASSET_DIR}/${validId(s.id)}/${validId(f.id)}.${ext}`;
  if (f.path && f.path !== legacy && f.path !== current) throw new Error('Invalid attachment path');
  return f.path === legacy ? legacy : current;
}
function upload(root: string, s: ChatSession, args: Record<string, unknown>): ChatFile {
  if (s.closed) throw new Error('Conversation is closed');
  const id = validId(args.upload_id);
  const name = text(args.name, 240);
  if (/[\r\n\x00-\x1f\x7f\/\\]/.test(name)) throw new Error('Invalid attachment name');
  if (args.source_path !== undefined) {
    if (args.data_base64 !== undefined) throw new Error('Choose source_path or data_base64, not both');
    const relative = referencePath(args.source_path);
    const info = fingerprint(safe(root, relative));
    const files = s.files ??= [];
    const existing = files.find(f => f.id === id);
    if (existing) {
      if (!existing.local_reference || existing.path !== relative || existing.sha256 !== info.sha256 || existing.name !== name) throw new Error('Attachment ID conflict');
      save(root, s); return existing;
    }
    const f: ChatFile = {id, name, path:relative, local_reference:true, ...info};
    f.label = nextLabel(s,f.mime); files.push(f); save(root, s); return f;
  }
  const encoded = args.data_base64;
  if (typeof encoded !== 'string' || encoded.length > 2796204 || encoded.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) throw new Error('Invalid attachment encoding or size');
  const bytes = Buffer.from(encoded, 'base64');
  if (bytes.toString('base64') !== encoded) throw new Error('Invalid attachment encoding or size');
  if (!bytes.length || bytes.length > MAX_FILE_BYTES) throw new Error('Attachment must contain 1–2097152 bytes');
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  const files = s.files ??= [];
  const existing = files.find(f => f.id === id);
  if (existing) {
    if (existing.local_reference || existing.sha256 !== sha256 || existing.name !== name) throw new Error('Attachment ID conflict');
    save(root, s); return existing;
  }
  const f: ChatFile = {id,name,mime:fileMime(bytes),size:bytes.length,sha256,path:''};
  f.path = filePath(s, f);
  mkdirSync(safe(root, `${ASSET_DIR}/${validId(s.id)}`), {recursive:true});
  const target = safe(root, f.path);
  // An interrupted upload may leave its immutable bytes; same-ID retry may reuse them.
  if (existsSync(target)) {
    if (statSync(target).size !== bytes.length || !readFileSync(target).equals(bytes)) throw new Error('Attachment ID conflict');
  } else writeFileSync(target, bytes, {mode:0o600,flag:'wx'});
  f.label = nextLabel(s,f.mime); files.push(f); save(root, s); return f;
}
function messageFiles(s: ChatSession, ids: unknown): ChatFile[] {
  if (ids === undefined) return [];
  if (!Array.isArray(ids) || new Set(ids).size !== ids.length) throw new Error('Select unique attachments');
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
function readArtifact(root: string, value: unknown): Record<string, unknown> {
  const relative = referencePath(value), target = safe(root, relative);
  if (!statSync(target).isFile()) throw new Error('Preview requires a regular image file');
  const fd = openSync(target, 'r');
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || !stat.size || stat.size > MAX_FILE_BYTES) throw new Error('Image preview limit is 2 MiB');
    const buffer = Buffer.alloc(MAX_FILE_BYTES + 1); let size = 0, count: number;
    while (size < buffer.length && (count = readSync(fd, buffer, size, buffer.length - size, null)) > 0) size += count;
    if (!size || size > MAX_FILE_BYTES) throw new Error('Image preview limit is 2 MiB');
    const bytes = buffer.subarray(0, size), mime = fileMime(bytes);
    if (!mime.startsWith('image/')) throw new Error('Only PNG, JPEG, GIF and WebP images can be previewed');
    return {name: relative.split('/').at(-1), mime, data_base64: bytes.toString('base64')};
  } finally { closeSync(fd); }
}
export function chatUi(root: string, args: Record<string, unknown>): Record<string, unknown> {
  return locked(root, () => {
    const action = args.action;
    if (action === 'list') {
      const sessions = readdirSync(safe(root, DIR)).filter(n => /^[a-zA-Z0-9_-]{1,80}\.json$/.test(n)).map(n => {
        const s = view(root, load(root, n.slice(0, -5))); return { ...s, messages: undefined };
      }).sort((a, b) => Number(a.archived)-Number(b.archived)||Number(b.pinned)-Number(a.pinned)||b.updated_at-a.updated_at);
      return { sessions };
    }
    if (action === 'create') {
      const now = Date.now(); const s: ChatSession = { version: 1, id: randomUUID(), title: text(args.title ?? '新对话', 240), created_at: now, updated_at: now, closed: false, messages: [], attachment_id: '', lease_until: 0 };
      if(args.mode!==undefined)group.setMode(s,args.mode);save(root, s); return { session: localView(root, s) };
    }
    const s = load(root, validId(args.chat_id));
    if(['set_mode','rename_member','detach_member','resume_member','set_coordinator'].includes(String(action))){group.groupUi(s,args);s.updated_at=Date.now();save(root,s);return {session:localView(root,s)};}
    if (action === 'reveal_path') return resolveChatPath(root, args.source_path);
    if (action === 'read_artifact') return readArtifact(root, args.source_path);
    if (action === 'upload_chunk') return uploadChunk(root,s,args);
    if (action === 'read_attachment_chunk') return readAttachmentChunk(root,s,args);
    if (action === 'upload') return { attachment: upload(root, s, args) };
    if (action === 'read_attachment') {
      const f = messageFiles(s, [args.upload_id])[0];
      const target = safe(root, filePath(s, f));
      const info = fingerprint(target);
      if (info.sha256 !== f.sha256 || info.size !== f.size) throw new Error('Attachment content changed');
      if (info.size > MAX_FILE_BYTES) return {attachment:f, local_only:true};
      const bytes = readFileSync(target);
      if (createHash('sha256').update(bytes).digest('hex') !== f.sha256) throw new Error('Attachment content changed');
      return { attachment: f, data_base64: bytes.toString('base64') };
    }
    if (action === 'request_connection') {
      if (s.closed) throw new Error('Conversation is closed');
      const id = validId(args.message_id);
      if (Object.hasOwn(s.queue_receipts??{},id)) throw new Error('Message ID conflicts with a queued delivery');
      const existing = [...s.messages,...(s.queue??[])].find(m => m.id === id);
      if (existing && existing.kind !== 'connection_request') throw new Error('Message ID conflicts with an existing message');
      const request = existing ?? s.messages.find(m => m.kind === 'connection_request' && !s.messages.some(r => r.role === 'assistant' && r.reply_to === m.id && r.final === true));
      const requestId = request?.id ?? id;
      if (!request) {
        s.messages.push({id, role:'user', kind:'connection_request', text:'请通过 chat_reply 回复“你好，有什么能帮到你？”（final=true），确认接入后继续 chat_wait。', created_at:Date.now()});
        s.updated_at = Date.now();
      }
      save(root, s);
      return {session:localView(root, s), connection_request_id:requestId};
    }
    if (action === 'send') {
      if (s.closed) throw new Error('Conversation is closed');
      const id = validId(args.message_id); const attachments = messageFiles(s, args.attachment_ids); const content = text(args.text || (attachments.length ? '📎' : ''));
      const receipt=Object.hasOwn(s.queue_receipts??{},id)?s.queue_receipts![id]:undefined;
      if(receipt){if(receipt!==queuedFingerprint(content,attachments))throw new Error('Message ID conflicts with a queued delivery');save(root,s);return {session:localView(root,s)};}
      const existing = [...s.messages,...(s.queue??[])].find(m => m.id === id);
      if (existing && (existing.role !== 'user' || existing.kind === 'connection_request' || existing.text !== content || JSON.stringify(existing.attachments ?? []) !== JSON.stringify(attachments))) throw new Error('Message ID conflicts with an existing message');
      if (!existing) { if (!s.messages.some(m => m.role === 'user' && m.kind !== 'connection_request') && !s.title_custom) s.title = [...content.replace(/\s+/g, ' ')].slice(0, 36).join(''); const message:ChatMessage={ id, role: 'user', text: content, attachments, created_at: Date.now() };group.targetUser(s,message);if(!awaitingConfirmation(s)&&(pending(s)||s.queue?.length))(s.queue??=[]).push(message);else s.messages.push(message); s.updated_at = Date.now(); }
      save(root, s);
    } else if(action==='set_queue_mode'){if(args.mode!=='merge'&&args.mode!=='split')throw new Error('Invalid queue mode');s.queue_mode=args.mode;save(root,s);}
    else if(action==='pin'){if(typeof args.pinned!=='boolean')throw new Error('pinned must be a boolean');s.pinned=args.pinned;save(root,s);}
    else if(action==='archive'){if(typeof args.archived!=='boolean')throw new Error('archived must be a boolean');s.archived=args.archived;save(root,s);}
    else if(action==='delete'){
      const status=view(root,s).status;
      if(status==='connected'||status==='waiting')throw new Error('Disconnect the AI before deleting this conversation');
      deleteSessionFiles(root,s.id);
      return {deleted:true,chat_id:s.id};
    }
    else if (action === 'rename') { s.title = text(args.title, 240).replace(/\s+/gu, ' '); s.title_custom = true; s.updated_at = Date.now(); save(root, s); }
    else if (action === 'detach') { for(const m of group.members(s)){m.lease_until=0;m.paused=true;}s.attachment_id = ''; s.lease_until = 0; s.updated_at = Date.now(); save(root, s); }
    else if (action === 'close') { s.closed = true; s.attachment_id = ''; s.lease_until = 0; s.updated_at = Date.now(); save(root, s); }
    else if (action !== 'read') throw new Error('Unknown chat action');
    return { session: localView(root, s) };
  });
}
/** Removes the JSON record, Markdown projection, `<id>.*` sidecars, pasted `<id>-*.<image>` files and chat assets. */
function deleteSessionFiles(root: string, id: string): void {
  const dotted = `${id}.`; const dashed = `${id}-`;
  const targets: string[] = [];
  // Validate every path before deleting anything; a bad asset must not erase the transcript.
  for (const name of readdirSync(safe(root, DIR))) {
    const image = /\.(png|jpe?g|gif|webp|bmp)$/i.test(name);
    if (!name.startsWith(dotted) && !(name.startsWith(dashed) && image)) continue;
    const target = safe(root, `${DIR}/${name}`);
    if (lstatSync(target).isFile()) targets.push(target);
  }
  const assets = safe(root, `${ASSET_DIR}/${id}`);
  if (existsSync(assets) && !lstatSync(assets).isDirectory()) throw new Error('Chat asset path is not a directory');
  if (existsSync(assets)) rmSync(assets, { recursive: true, force: true });
  const record = file(root, id);
  for (const target of targets.filter(target => target !== record)) rmSync(target, { force: true });
  rmSync(record, { force: true });
}
function owned(s: ChatSession, attachment: unknown): void {
  if(group.grouped(s)){group.memberFor(s,attachment);return;}
  // The lease only governs takeover by another AI; the holder keeps working through long tasks.
  if (!attachment || attachment !== s.attachment_id) throw new Error('Chat attachment expired; call chat_open again');
}
export function chatTool(root: string, name: string, args: Record<string, unknown>): Record<string, unknown> {
  return locked(root, () => {
    const s = load(root, validId(args.chat_id));
    if (s.closed) return { ok: true, status: 'closed' };
    if (name === 'chat_open') {
      if(group.grouped(s)){const member=group.openGroup(s,args);group.bindTargets(s);save(root,s);return {ok:true,attachment_id:member.attachment_id,agent_id:member.id,role:member.role,session:view(root,s),instruction: 'Read skill.text; reply only to your delivered message IDs. Call chat_wait.',skill:localChatSkill};}
      if(args.agent_name!==undefined)s.agent_name=group.memberName(args.agent_name);
      // Keepalive: the saved attachment_id resumes even after the lease lapsed, unless another AI attached meanwhile.
      const resuming = typeof args.attachment_id === 'string' && !!args.attachment_id && args.attachment_id === s.attachment_id;
      if(args.attachment_id && !resuming)throw new Error('Chat attachment expired or replaced; start a new connection from the UI');
      if (!resuming && s.lease_until > Date.now()) throw new Error('Conversation already attached; close it in the UI or wait for the lease to expire');
      if (!resuming) s.attachment_id = randomUUID();
      group.renew(s,args.attachment_id); save(root, s);
      return { ok: true, attachment_id: s.attachment_id, session: view(root, s), instruction: 'Read skill.text and follow it for this session; save attachment_id and call chat_wait now.', skill: localChatSkill };
    }
    owned(s, args.attachment_id);
    if (name === 'chat_upload') {
      const encoded = args.data_base64;
      if (typeof encoded === 'string' && (encoded.length > 699052 || Buffer.from(encoded, 'base64').length > 512 * 1024)) throw new Error('MCP attachment must not exceed 512 KiB; compress it before upload');
      group.renew(s,args.attachment_id); return { ok: true, attachment: upload(root, s, args) };
    }
    if (name === 'chat_reply') {
      const id = validId(args.message_id); const replyTo = validId(args.reply_to); const content = text(args.text);
      const attachments = messageFiles(s, args.attachment_ids);
      const final = args.final !== false; const tool_event = toolEvent(args.tool_event, final);
      if (args.awaiting_user !== undefined && typeof args.awaiting_user !== 'boolean') throw new Error('awaiting_user must be a boolean');
      if (Object.hasOwn(s.queue_receipts??{},id)) throw new Error('Message ID conflicts with a queued delivery');
      const awaiting_user = args.awaiting_user === true;
      if (awaiting_user && !final) throw new Error('awaiting_user requires final=true');
      const identity=group.replyIdentity(s,args,final);
      const existing = [...s.messages,...(s.queue??[])].find(m => m.id === id);
      if (existing) {
        if(existing.agent_id!==identity.agent_id||JSON.stringify(existing.recipient_ids)!==JSON.stringify(identity.recipient_ids))throw new Error('Message ID conflicts with another agent reply');
        if (JSON.stringify(existing.attachments ?? []) !== JSON.stringify(attachments) || existing.role !== 'assistant' || existing.text !== content || existing.reply_to !== replyTo || existing.final !== final || (existing.awaiting_user === true) !== awaiting_user || JSON.stringify(existing.tool_event ?? undefined) !== JSON.stringify(tool_event)) throw new Error('Message ID conflicts with an existing reply');
      } else {
        group.validateFinal(s,args);
        if (pending(s,args.attachment_id)?.id !== replyTo) throw new Error('Reply must address the oldest unanswered user message');
        s.messages.push({ ...identity, id, role: 'assistant', text: content, reply_to: replyTo, final, awaiting_user, tool_event, attachments, created_at: Date.now() }); s.updated_at = Date.now();
      }
      group.renew(s,args.attachment_id); save(root, s); return { ok: true, persisted: true, message_id: id };
    }
    if (name === 'chat_close') { if(group.grouped(s)&&group.memberFor(s,args.attachment_id).role!=='coordinator')throw new Error('Only the coordinator can close the group');s.closed = true; s.attachment_id = ''; s.lease_until = 0; save(root, s); return { ok: true, status: 'closed' }; }
    if (name === 'chat_wait') {
      publishQueued(s);
      group.renew(s,args.attachment_id);
      const message = pending(s,args.attachment_id,true);
      if(message&&group.grouped(s)){const agent=group.memberFor(s,args.attachment_id).id;if(!message.received_by?.includes(agent)){(message.received_by??=[]).push(agent);s.updated_at=Date.now();}}
      if (message && !message.received_at) {
        message.received_at = Date.now();
        s.updated_at = message.received_at;
      }
      save(root, s);
      return { ok: true, status: message ? 'message' : 'idle', message: message ?? null,...(group.grouped(s)?{session:view(root,s)}:{}) };
    }
    throw new Error('Unknown chat tool');
  });
}
export async function chatWait(root: string, args: Record<string, unknown>, signal?: AbortSignal): Promise<Record<string, unknown>> {
  const timeout = args.timeout_ms ?? 120000;
  if (typeof timeout !== 'number' || !Number.isInteger(timeout) || timeout < 0 || timeout > 180000) throw new Error('timeout_ms must be 0–180000');
  const initial = chatTool(root, 'chat_wait', args);
  if (initial.status !== 'idle') return initial;
  const id = validId(args.chat_id); const k = key(root, id)+':'+String(args.attachment_id);
  if (waiters.has(k)) throw new Error('A wait request is already active');
  waiters.add(k);
  try {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      signal?.throwIfAborted();
      const s = load(root, id);
      if (s.closed) return { ok: true, status: 'closed' };
      owned(s, args.attachment_id);
      const message = pending(s,args.attachment_id,true);
      if (message || (s.queue?.length && !pending(s) && !awaitingConfirmation(s))) return chatTool(root, 'chat_wait', args);
      await sleep(Math.min(250, end - Date.now()), undefined, { signal });
    }
    return { ok: true, status: 'idle', instruction: 'No message. Call chat_wait again now with the same session and attachment; idle is not an exit. Keep waiting within host limits.' };
  } finally { waiters.delete(k); }
}

const CHUNK_BYTES = 512 * 1024;
function labelFiles(s: ChatSession): void {
  let images = 0, files = 0;
  for (const file of s.files ?? []) file.label = file.mime.startsWith('image/') ? `图片${++images}` : `文件${++files}`;
  const labels = new Map((s.files ?? []).map(file => [file.id, file.label]));
  for (const message of s.messages) for (const file of message.attachments ?? []) if (labels.has(file.id)) file.label = labels.get(file.id);
}
function nextLabel(s: ChatSession, mime: string): string {
  const image = mime.startsWith('image/');
  return `${image ? '图片' : '文件'}${(s.files ?? []).filter(f => f.mime.startsWith('image/') === image).length + 1}`;
}
function uploadChunk(root: string, s: ChatSession, args: Record<string, unknown>): Record<string, unknown> {
  if (s.closed) throw new Error('Conversation is closed');
  const id = validId(args.upload_id), name = text(args.name, 240);
  if (/[\r\n\x00-\x1f\x7f\/\\]/.test(name)) throw new Error('Invalid attachment name');
  const offset = args.offset, total = args.total_size, encoded = args.data_base64;
  if (typeof offset !== 'number' || !Number.isSafeInteger(offset) || offset < 0 || typeof total !== 'number' || !Number.isSafeInteger(total) || total <= 0 || offset >= total) throw new Error('Invalid upload range');
  if (typeof encoded !== 'string' || encoded.length > 699052) throw new Error('Invalid chunk encoding or size');
  const bytes = Buffer.from(encoded, 'base64');
  if (!bytes.length || bytes.length > CHUNK_BYTES || bytes.toString('base64') !== encoded || offset + bytes.length > total) throw new Error('Invalid chunk encoding or size');
  const existing = s.files?.find(f => f.id === id);
  const dir = `${ASSET_DIR}/${validId(s.id)}`;
  mkdirSync(safe(root, dir), {recursive:true});
  const metaPath = safe(root, `${dir}/${id}.upload.json`), partPath = safe(root, `${dir}/${id}.part`);
  let meta: {name:string; size:number; mime:string};
  if (existing) {
    if (existing.local_reference || existing.name !== name || existing.size !== total) throw new Error('Attachment ID conflict');
    meta = existing;
  } else if (existsSync(metaPath)) {
    meta = JSON.parse(readFileSync(metaPath,'utf8'));
    if (meta.name !== name || meta.size !== total) throw new Error('Attachment ID conflict');
  } else {
    if (offset !== 0) throw new Error('Upload must start at offset 0');
    meta = {name, size:total, mime:fileMime(bytes)};
    writeFileSync(metaPath,JSON.stringify(meta),{flag:'wx',mode:0o600});
  }
  const f: ChatFile = {id, name, size:total, mime:meta.mime, sha256:'', path:''};
  f.path = filePath(s, f);
  const target = safe(root, f.path);
  const completed = existing || existsSync(target);
  const source = completed ? target : partPath;
  if (!existsSync(source)) writeFileSync(source,Buffer.alloc(0),{flag:'wx',mode:0o600});
  const fd = openSync(source, completed ? 'r' : 'r+');
  try {
    const size = fstatSync(fd).size;
    if (size > total || offset > size) throw new Error('Upload offset does not match stored bytes');
    const overlap = Math.min(bytes.length, size - offset);
    const prior = Buffer.alloc(overlap);
    if (readSync(fd,prior,0,overlap,offset) !== overlap || !prior.equals(bytes.subarray(0,overlap))) throw new Error('Attachment ID conflict');
    if (overlap < bytes.length) {
      if (completed) throw new Error('Attachment content changed');
      let written = overlap;
      while (written < bytes.length) { const n = writeSync(fd,bytes,written,bytes.length-written,offset+written); if (!n) throw new Error('Cannot write attachment'); written += n; }
      fsyncSync(fd);
    }
  } finally {closeSync(fd);}
  if (existing) { save(root,s); return {attachment:existing, next_offset:offset+bytes.length}; }
  if (statSync(source).size < total) return {next_offset:offset+bytes.length};
  Object.assign(f,fingerprint(source)); f.label = nextLabel(s,f.mime);
  if (!completed) renameSync(partPath,target);
  (s.files ??= []).push(f); save(root,s); rmSync(metaPath,{force:true});
  return {attachment:f,next_offset:offset+bytes.length};
}
function readAttachmentChunk(root: string, s: ChatSession, args: Record<string, unknown>): Record<string, unknown> {
  const f = s.files?.find(file => file.id === validId(args.upload_id));
  if (!f) throw new Error('Attachment not found');
  const offset = args.offset;
  if (typeof offset !== 'number' || !Number.isSafeInteger(offset) || offset < 0 || offset >= f.size) throw new Error('Invalid attachment range');
  const fd = openSync(safe(root,filePath(s,f)),'r');
  try {
    if (!fstatSync(fd).isFile() || fstatSync(fd).size !== f.size) throw new Error('Attachment content changed');
    const bytes = Buffer.alloc(Math.min(CHUNK_BYTES,f.size-offset));
    if (readSync(fd,bytes,0,bytes.length,offset) !== bytes.length) throw new Error('Attachment content changed');
    return {attachment:f,data_base64:bytes.toString('base64'),next_offset:offset+bytes.length};
  } finally {closeSync(fd);}
}

function awaitingConfirmation(s: ChatSession): boolean {
  for(let i=s.messages.length-1;i>=0;i--){const m=s.messages[i];if(m.role==='user')return false;if(m.role==='assistant'&&m.final===true)return m.awaiting_user===true;}
  return false;
}
function queuedFingerprint(content: string, attachments: ChatFile[]): string {
  return createHash('sha256').update(JSON.stringify([content,attachments.map(f=>f.id)])).digest('hex');
}
function localView(root: string, s: ChatSession) {return {...view(root,s),connection_id:s.attachment_id?createHash('sha256').update(s.attachment_id).digest('hex').slice(0,12):undefined,...readChatOperations(root,s.id),queued_messages:s.queue ?? [],queue_mode:s.queue_mode ?? (group.grouped(s)?'split':'merge')};}
function publishQueued(s: ChatSession): void {
  if(pending(s)||awaitingConfirmation(s)||!s.queue?.length)return;
  const items=s.queue.splice(0,(s.queue_mode ?? (group.grouped(s)?'split':'merge'))==='split'?1:s.queue.length);
  const attachments=[...new Map(items.flatMap(m=>m.attachments??[]).map(f=>[f.id,f])).values()];
  const message:ChatMessage={...items[0],text:items.length===1?items[0].text:items.map((m,i)=>`队列${i+1}：${m.text}`).join('\n\n'),attachments,created_at:Date.now()};
  for(const item of items)Object.defineProperty(s.queue_receipts??={},item.id,{value:queuedFingerprint(item.text,item.attachments??[]),enumerable:true,writable:true,configurable:true});
  // Preserve queued recipients even when a member has since been renamed.
  if(group.grouped(s)){const targets=items.flatMap(item=>{if(!item.recipient_ids?.length)group.targetUser(s,item);return item.recipient_ids??[]});message.recipient_ids=[...new Set(targets)];}
  s.messages.push(message);s.updated_at=Date.now();
}
function queuedMarkdown(s: ChatSession): string {
  return s.queue?.length ? '\n## 待发送队列\n\n'+s.queue.map((m,i)=>`### 队列${i+1}\n\n${m.text}\n\n${(m.attachments??[]).map(f=>`@${f.label??f.name}: ${f.path}`).join('\n')}\n`).join('\n') : '';
}

/** Plans belong to an authenticated attachment and its current user message. */
export function chatPlan(root:string,name:string,args:Record<string,unknown>):Record<string,unknown> {
 return locked(root,()=>{
  const session=load(root,validId(args.chat_id));owned(session,args.attachment_id);
  if(session.closed)throw new Error('Conversation is closed');
  const replyTo=validId(args.reply_to);const message=pending(session,args.attachment_id);
  if(!message||message.id!==replyTo||message.kind==='connection_request')throw new Error('Plan must address the current unanswered user message');
  const actor=group.grouped(session)?group.memberFor(session,args.attachment_id).id:undefined;
  const previous=actor?message.agent_plans?.find(p=>p.agent_id===actor)?.plan:message.task_plan;
  const plan=reduceChatPlan(previous,name,args);
  if(actor){message.agent_plans=(message.agent_plans??[]).filter(p=>p.agent_id!==actor);if(plan)message.agent_plans.push({agent_id:actor,plan});}
  else if(plan)message.task_plan=plan;else delete message.task_plan;
  session.updated_at=Date.now();group.renew(session,args.attachment_id);save(root,session);
  return {ok:true,persisted:true,chat_id:session.id,reply_to:replyTo,plan:chatPlanSummary(plan),...(name==='report_progress'?{progress:plan?.progress}: {})};
 });
}

// Operation archives have a separate budget from messages and never contain attachment IDs.
export function chatOperationOwner(root:string,chatId:string,attachmentId:string,replyTo?:string):{agent_id?:string;agent_name:string}|undefined {
 try {
  const s=load(root,chatId);if(s.closed)return;owned(s,attachmentId);
  const actor=group.grouped(s)?group.memberFor(s,attachmentId):undefined;
  if(replyTo){const m=s.messages.find(m=>m.id===replyTo);if(!m||m.kind==='connection_request')return;
   if(actor?!m.received_by?.includes(actor.id):!m.received_at)return;
   if(s.messages.some(r=>r.reply_to===replyTo&&r.final===true&&(!actor||r.agent_id===actor.id)))return;
  }
  return {agent_name:actor?.name??s.agent_name??'AI',...(actor?{agent_id:actor.id}:{})};
 }catch{return;}
}
export function readChatOperations(root:string,chatId:string):{operations:ChatOperation[];operations_error?:string} {
 try{const target=safe(root,`${DIR}/${validId(chatId)}.operations.json`);if(!existsSync(target))return {operations:[]};
  if(statSync(target).size>600000)throw new Error('Operation archive exceeds limit');
  const events=JSON.parse(readFileSync(target,'utf8'));if(!Array.isArray(events))throw new Error('Invalid operation archive');return {operations:events};
 }catch{return {operations:[],operations_error:'Operation archive unavailable'};}
}
export function writeChatOperation(root:string,chatId:string,event:ChatOperation):void {
 locked(root,()=>{
  const current=readChatOperations(root,chatId);if(current.operations_error)throw new Error(current.operations_error);
  const events=current.operations;const i=events.findIndex(e=>e.id===event.id);if(i<0)events.push(event);else events[i]=event;
  while(events.length>240||Buffer.byteLength(JSON.stringify(events))>512000)events.shift();
  for(const [ext,body] of [['json',JSON.stringify(events)],['md',operationsMarkdown(events)]]){
   const temporary=safe(root,`${DIR}/${validId(chatId)}.${randomUUID()}.tmp`);
   try{writeFileSync(temporary,body,{mode:0o600,flag:'wx'});renameSync(temporary,safe(root,`${DIR}/${chatId}.operations.${ext}`));}finally{rmSync(temporary,{force:true});}
  }
 });
}
