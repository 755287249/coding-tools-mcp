import type { ChatSession } from '../api/chat';

export type SearchConversation = {
  workspaceId: string;
  workspaceName: string;
  folderId: string;
  folderName: string;
  chat: ChatSession;
};

/** Search summary metadata only; never fetch transcripts or connection secrets. */
export function searchConversations(items: SearchConversation[], query: string, mode: 'all' | 'work' | 'group' = 'all') {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return items.filter(item => {
    if (mode !== 'all' && (item.chat.mode ?? 'work') !== mode) return false;
    if (!words.length && item.chat.archived) return false;
    const text = `${item.chat.title} ${item.workspaceName} ${item.folderName}`.toLocaleLowerCase();
    return words.every(word => text.includes(word));
  }).sort((a, b) => Number(!!a.chat.archived) - Number(!!b.chat.archived) || b.chat.updated_at - a.chat.updated_at)
    .slice(0, words.length ? 40 : 9);
}
