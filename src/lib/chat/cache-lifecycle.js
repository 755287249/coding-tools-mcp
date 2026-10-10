/** Page-memory only; authentication/backend changes discard snapshots and pending reads. */
/** @type {Set<() => void>} */
const resets = new Set();
/** @param {() => void} reset */
export function onChatCacheReset(reset) { resets.add(reset); }
export function clearChatCaches() { for (const reset of resets) reset(); }
