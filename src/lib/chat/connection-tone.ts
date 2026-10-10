import type { ChatSession, Discussion } from '../api/chat';

export type ConnectionTone = 'offline' | 'online' | 'stale';

/** A live polling wait and an attached agent between polls are both online. */
export function chatConnectionTone(session: ChatSession | null): ConnectionTone {
  if (!session || session.closed) return 'offline';
  if (session.status === 'connected' || session.status === 'waiting') return 'online';
  const attached = session.mode === 'group'
    ? session.members?.some(member => !member.paused)
    : !!session.connection_id;
  return attached ? 'stale' : 'offline';
}

/** Paused groups are deliberately disconnected; offline members need attention. */
export function discussionConnectionTone(discussion: Discussion | undefined): ConnectionTone {
  if (!discussion || discussion.paused || discussion.archived) return 'offline';
  const members = discussion.member_details ?? [];
  if (members.some(member => member.status === 'connected' || member.status === 'waiting')) return 'online';
  return members.some(member => member.status === 'offline') ? 'stale' : 'offline';
}
