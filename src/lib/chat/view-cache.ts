import { onChatCacheReset } from './cache-lifecycle.js';
export interface ChatReadingPosition { historyAnchor: string; scrollTop: number; following: boolean; activeMessage: string }
/** Small, page-memory reading positions survive panel unmounts without retaining DOM nodes. */
export function createReadingPositionCache(limit = 24) {
  const positions = new Map<string, ChatReadingPosition>();
  let epoch = 0;
  return {
    version: () => epoch,
    get(key: string): ChatReadingPosition | null {
      const value = positions.get(key);
      if (!value) return null;
      positions.delete(key); positions.set(key, value);
      return { ...value };
    },
    put(key: string, value: ChatReadingPosition, version = epoch) {
      if (version !== epoch || limit < 1) return;
      positions.delete(key); positions.set(key, { ...value });
      while (positions.size > limit) positions.delete(positions.keys().next().value!);
    },
    clear() { epoch++; positions.clear(); },
  };
}
export const chatReadingPositions = createReadingPositionCache();
onChatCacheReset(() => chatReadingPositions.clear());
