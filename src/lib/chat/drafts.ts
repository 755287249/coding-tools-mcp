import type { ChatFile } from '../api/chat';
export interface DraftRetry { text: string; id: string; attachmentKey: string }
export interface ChatDraft { text: string; attachments: ChatFile[]; retry: DraftRetry | null }
type StoragePort = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
const empty = (): ChatDraft => ({ text: '', attachments: [], retry: null });
export function chatDraftKey(workspace: string, folder: string, chat: string): string {
  return `ctmcp-chat-draft:${JSON.stringify([workspace, folder, chat])}`;
}
function parseDraft(raw: string | null): ChatDraft {
  if (!raw) return empty();
  try {
    const value = JSON.parse(raw);
    if (!value || typeof value.text !== 'string' || value.text.length > 32000 || !Array.isArray(value.attachments)) return empty();
    if (!value.attachments.every((file: ChatFile) => file && ['id','name','path','mime','sha256'].every(field => typeof file[field as keyof ChatFile] === 'string') && Number.isFinite(file.size) && file.size > 0)) return empty();
    const retry = value.retry;
    return { text: value.text, attachments: value.attachments, retry: retry && typeof retry.text === 'string' && typeof retry.id === 'string' && typeof retry.attachmentKey === 'string' ? retry : null };
  } catch { return empty(); }
}
/** Memory survives route changes if browser storage is disabled/full. No draft is sent here. */
export function createDraftStore(storage: () => StoragePort) {
  const memory = new Map<string, { raw: string; persisted: boolean }>();
  function load(key: string): { draft: ChatDraft; persisted: boolean } {
    const cached = memory.get(key);
    if (cached) return { draft: parseDraft(cached.raw), persisted: cached.persisted };
    try { return { draft: parseDraft(storage().getItem(key)), persisted: true }; }
    catch { return { draft: empty(), persisted: false }; }
  }
  function save(key: string, draft: ChatDraft): boolean {
    const raw = JSON.stringify(draft);
    let persisted = true;
    try {
      if (!draft.text && !draft.attachments.length) storage().removeItem(key);
      else storage().setItem(key, raw);
    } catch { persisted = false; }
    memory.set(key, { raw, persisted });
    return persisted;
  }
  function clearSent(key: string, sent: DraftRetry) {
    const current = load(key).draft;
    if (current.retry?.id !== sent.id || current.text.trim() !== sent.text || current.attachments.map(file => file.id).join(',') !== sent.attachmentKey) return { cleared: false, persisted: true };
    return { cleared: true, persisted: save(key, empty()) };
  }
  return { load, save, clearSent };
}
export const chatDrafts = createDraftStore(() => localStorage);

/** A UUID-derived hue feels random while remaining stable between views and reloads. */
export function conversationAccent(chatId: string): string {
  if (!chatId) return 'transparent';
  let hash = 2166136261;
  for (let i = 0; i < chatId.length; i++) hash = Math.imul(hash ^ chatId.charCodeAt(i), 16777619);
  return `hsl(${(hash >>> 0) % 360} 34% 55%)`;
}
