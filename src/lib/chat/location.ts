/** Explicit workspace/folder/chat identities keep navigation scoped across browser history. */
export function chatLocation(workspaceId: string, folderId: string, chatId?: string): string {
  const query = new URLSearchParams({ tab: 'chat', folder: folderId });
  if (chatId) query.set('chat', chatId);
  else query.set('new', '1');
  return `/workspace/${encodeURIComponent(workspaceId)}?${query}`;
}

export function navigationWidth(value: unknown): number {
  const width = Number(value);
  return Number.isFinite(width) && width > 0 ? Math.min(420, Math.max(220, width)) : 280;
}
