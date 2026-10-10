import type { ChatMessage, ChatSession } from '../api/chat';
import { pendingChatState } from './status';

/** Transport polling alternates waiting/connected; both mean the same online presence. */
export function sessionPresence(session: ChatSession): 'online' | 'offline' | 'working' | 'error' | 'closed' | 'queued' | 'awaiting_user' {
  if (session.closed) return 'closed';
  const pending = session.messages ? pendingChatState(session) : session.work_state;
  if (pending === 'interrupted' || (pending === 'processing' && session.status === 'offline')) return 'error';
  if (pending === 'awaiting_user' || session.awaiting_user) return 'awaiting_user';
  if (pending === 'queued' && session.status !== 'offline') return 'queued';
  if (pending === 'processing') return 'working';
  return session.status === 'waiting' || session.status === 'connected' ? 'online' : 'offline';
}

/** Only live attached conversations belong in the compact switcher. */
export function isConnectedConversation(session: ChatSession): boolean {
  return !session.closed && !session.archived && (session.status === 'connected' || session.status === 'waiting');
}

export function messageOutline(messages: ChatMessage[]) {
  const replies = new Map<string, string>();
  for (const message of messages) {
    if (message.role === 'assistant' && message.reply_to && !message.tool_event && message.text.trim()) {
      replies.set(message.reply_to, message.text.replace(/\s+/g, ' ').slice(0, 180));
    }
  }
  return messages.filter(message => message.role === 'user').map(message => ({
    id: message.id,
    title: (message.text.trim() || message.attachments?.map(file => file.name).join(', ') || '').replace(/\s+/g, ' ').slice(0, 120),
    preview: replies.get(message.id) ?? '',
  }));
}

/** Activity excludes lease renewals, pinning and edits to conversation metadata. */
export function conversationActivity(session: ChatSession): number {
  return session.last_message_at ?? session.messages?.reduce((time, message) => Math.max(time, message.created_at), session.created_at) ?? session.created_at;
}
export function recentConversationOrder(a: ChatSession, b: ChatSession): number {
  return conversationActivity(b) - conversationActivity(a) || a.id.localeCompare(b.id);
}
export function conversationOrder(a: ChatSession, b: ChatSession): number {
  return Number(!!a.archived) - Number(!!b.archived)
    || Number(isConnectedConversation(b)) - Number(isConnectedConversation(a))
    || Number(!!b.pinned) - Number(!!a.pinned) || recentConversationOrder(a,b);
}
