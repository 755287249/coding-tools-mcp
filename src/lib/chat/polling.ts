/** Serial polling: no background traffic, and an immediate refresh on return. */
export function startVisiblePolling(task: () => Promise<void>, interval: () => number): () => void {
  let stopped = false, running = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  async function run() {
    if (stopped || running || document.hidden) return;
    running = true;
    try { await task(); }
    finally {
      running = false;
      if (!stopped && !document.hidden) timer = setTimeout(() => { void run(); }, interval());
    }
  }
  function visibility() {
    clearTimeout(timer);
    if (!document.hidden) void run();
  }
  document.addEventListener('visibilitychange', visibility);
  void run();
  return () => { stopped = true; clearTimeout(timer); document.removeEventListener('visibilitychange', visibility); };
}

/** Reuse unchanged JSON subtrees so heartbeat updates do not re-render old messages. */
export function reconcileSnapshot<T>(previous: T, next: T): T {
  if (Object.is(previous, next)) return previous;
  if (!previous || !next || typeof previous !== 'object' || typeof next !== 'object' || Array.isArray(previous) !== Array.isArray(next)) return next;
  const before = previous as Record<string, unknown>, after = next as Record<string, unknown>;
  const keys = Object.keys(after);
  let same = keys.length === Object.keys(before).length;
  const result: Record<string, unknown> = Array.isArray(next) ? [] as unknown as Record<string, unknown> : {};
  for (const key of keys) {
    const value = reconcileSnapshot(before[key], after[key]);
    Object.defineProperty(result, key, {value, enumerable:true, configurable:true, writable:true});
    if (!Object.hasOwn(before, key) || !Object.is(value, before[key])) same = false;
  }
  return same ? previous : result as T;
}
