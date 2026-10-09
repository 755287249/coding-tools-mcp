import type { ChatSession } from '../api/chat';

export function sessionCacheKey(workspace: string, folder: string, chat: string): string {
  return JSON.stringify([workspace, folder, chat]);
}

/** Component-lifetime history snapshots. Never write conversation content to browser storage. */
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
  };
}
