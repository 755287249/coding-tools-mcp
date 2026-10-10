import { onChatCacheReset } from './cache-lifecycle.js';

/** Cache only fully verified immutable attachment bytes, never credentials or arbitrary paths. */
export function createAttachmentCache(maxBytes = 24 * 1024 * 1024, maxEntries = 64) {
  /** @type {Map<string, Uint8Array>} */
  const values = new Map();
  /** @type {Map<string, Promise<Uint8Array>>} */
  const pending = new Map();
  let bytes = 0, epoch = 0;
  function clear() { epoch++; values.clear(); pending.clear(); bytes = 0; }
  /** @param {string} key @param {() => Promise<Uint8Array>} load */
  async function read(key, load) {
    const found = values.get(key);
    if (found) { values.delete(key); values.set(key, found); return found.slice(); }
    let task = pending.get(key);
    if (!task) {
      const version = epoch;
      const operation = Promise.resolve().then(load).then(value => {
        if (version !== epoch) throw new Error('Chat identity changed');
        if (value.byteLength <= maxBytes && maxEntries > 0) {
          values.set(key, value); bytes += value.byteLength;
          while (bytes > maxBytes || values.size > maxEntries) {
            const oldest = /** @type {string} */ (values.keys().next().value);
            bytes -= (values.get(oldest)?.byteLength ?? 0); values.delete(oldest);
          }
        }
        return value;
      }).finally(() => { if (pending.get(key) === operation) pending.delete(key); });
      pending.set(key, operation); task = operation;
    }
    return (await task).slice();
  }
  return { read, clear };
}
export const attachmentCache = createAttachmentCache();
onChatCacheReset(attachmentCache.clear);
