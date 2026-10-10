import type { ChatSession, ChatAction, ChatResult } from '../api/chat';
import { onChatCacheReset } from './cache-lifecycle.js';

export function sessionCacheKey(workspace: string, folder: string, chat: string): string {
  return JSON.stringify([workspace, folder, chat]);
}

/** Bounded page-lifetime history snapshots. Never write conversation content to browser storage. */
export function createSessionCache(maxEntries = 12, maxCharacters = 4 * 1024 * 1024) {
  const entries = new Map<string, { session: ChatSession; size: number }>();
  let characters = 0;
  function remove(key: string) {
    characters -= entries.get(key)?.size ?? 0;
    entries.delete(key);
  }
  return {
    get(key: string): ChatSession | null {
      const entry = entries.get(key);
      if (!entry) return null;
      entries.delete(key);
      entries.set(key, entry);
      return entry.session;
    },
    put(key: string, session: ChatSession) {
      remove(key);
      const size = JSON.stringify(session).length;
      if (size > maxCharacters || maxEntries < 1) return;
      entries.set(key, { session, size });
      characters += size;
      while (entries.size > maxEntries || characters > maxCharacters) {
        remove(entries.keys().next().value!);
      }
    },
    remove,
    clear() { entries.clear(); characters = 0; },
  };
}

/** One request per scope while pending; mutations always reach the server exactly once. */
export function createSessionReader() {
  const snapshots = createSessionCache();
  const pending = new Map<string, Promise<ChatResult>>();
  const lists = new Map<string, { result: ChatResult; at: number }>();
  let epoch = 0, identity = 0;
  function invalidate() { epoch++; pending.clear(); lists.clear(); }
  function clear() { identity++; invalidate(); snapshots.clear(); }
  async function request(workspace: string, folder: string, args: ChatAction, load: () => Promise<ChatResult>): Promise<ChatResult> {
    const scope = sessionCacheKey(workspace, folder, args.chat_id ?? '');
    const readonly = ['list', 'read', 'read_attachment', 'read_attachment_chunk', 'read_artifact', 'reveal_path'].includes(args.action);
    if (!readonly) {
      const context = identity;
      invalidate();
      try {
        const result = await load();
        if (context !== identity) throw new Error('Chat identity changed');
        invalidate();
        if (args.action === 'delete' && result.deleted) snapshots.remove(scope);
        if (result.session?.messages) snapshots.put(sessionCacheKey(workspace, folder, result.session.id), result.session);
        return result;
      } catch (error) { if (context === identity) invalidate(); throw error; }
    }
    if (args.action !== 'read' && args.action !== 'list') return load();
    const key = JSON.stringify([workspace, folder, args]);
    const recent = lists.get(key);
    if (recent && Date.now() - recent.at < 500) return recent.result;
    const existing = pending.get(key);
    if (existing) return existing;
    const version = epoch;
    const promise = Promise.resolve().then(load).then(result => {
      if (version !== epoch) throw new Error('Conversation changed; refresh again');
      if (result.session?.id === args.chat_id && result.session?.messages) snapshots.put(scope, result.session);
      if (args.action === 'list') {
        lists.delete(key); lists.set(key, { result, at: Date.now() });
        while (lists.size > 24) lists.delete(lists.keys().next().value!);
      }
      return result;
    }).finally(() => { if (pending.get(key) === promise) pending.delete(key); });
    pending.set(key, promise);
    return promise;
  }
  return { snapshots, request, clear };
}
export const chatSessionReader = createSessionReader();
onChatCacheReset(chatSessionReader.clear);
