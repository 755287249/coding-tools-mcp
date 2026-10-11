export interface SeedDraft { account: string; repo: string; branch: string; count: number; endpoint: string }
type StoragePort = Pick<Storage, 'getItem' | 'setItem'>;
export const seedDraftKey = (workspace: string, folder: string) => `ctmcp-seed-draft:${JSON.stringify([workspace, folder])}`;
function clean(value: Partial<SeedDraft>): SeedDraft {
  return {account: typeof value.account === 'string' ? value.account : '', repo: typeof value.repo === 'string' ? value.repo : '',
    branch: typeof value.branch === 'string' ? value.branch : 'main', count: Number.isInteger(value.count) && value.count! >= 1 && value.count! <= 50 ? value.count! : 3,
    endpoint: typeof value.endpoint === 'string' ? value.endpoint : ''};
}
/** Ordinary configuration is durable; enrollment tickets only survive route changes in memory. */
export function createSeedDraftStore(storage: () => StoragePort) {
  const drafts = new Map<string, SeedDraft>();
  const batches = new Map<string, {text: string; expires: number}>();
  function load(key: string): SeedDraft {
    if (drafts.has(key)) return {...drafts.get(key)!};
    try { return clean(JSON.parse(storage().getItem(key) || '{}') || {}); } catch { return clean({}); }
  }
  function save(key: string, value: SeedDraft): boolean {
    const draft = clean(value);
    drafts.set(key, draft);
    try { storage().setItem(key, JSON.stringify(draft)); return true; } catch { return false; }
  }
  function batch(key: string, now = Date.now()): string {
    const item = batches.get(key);
    if (!item || item.expires <= now) { batches.delete(key); return ''; }
    return item.text;
  }
  function setBatch(key: string, text: string): void {
    if (!text) { batches.delete(key); return; }
    const data = JSON.parse(text);
    const expires = Math.min(...data.seeds.map((seed: {expires_at: number}) => seed.expires_at));
    if (!Number.isFinite(expires) || expires <= Date.now()) { batches.delete(key); return; }
    batches.set(key, {text, expires});
  }
  return {load, save, batch, setBatch};
}
export const seedDrafts = createSeedDraftStore(() => localStorage);
