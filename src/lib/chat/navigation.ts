import type { ChatMessage, ChatSession } from '../api/chat';
import { pendingChatState } from './status';

/** Transport polling alternates waiting/connected; both mean the same online presence. */
export function sessionPresence(session: ChatSession): 'online' | 'offline' | 'working' | 'error' | 'closed' {
  if (session.closed) return 'closed';
  const pending = session.messages ? pendingChatState(session) : session.work_state;
  if (pending === 'interrupted' || (pending === 'processing' && session.status === 'offline')) return 'error';
  if (pending === 'processing') return 'working';
  return session.status === 'waiting' || session.status === 'connected' ? 'online' : 'offline';
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
