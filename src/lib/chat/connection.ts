import type { ChatSession } from '$lib/api/chat';

export function visibleChatMessages(session: ChatSession | null) {
  return (session?.messages ?? []).filter(message => message.kind !== 'connection_request');
}

/** Only a persisted final greeting to this exact control message completes pairing. */
export function connectionResult(session: ChatSession | null, requestId: string): 'waiting' | 'success' | 'failed' {
  if (!session?.messages?.some(message => message.id === requestId && message.kind === 'connection_request')) return 'waiting';
  const reply = session.messages.find(message => message.role === 'assistant' && message.reply_to === requestId && message.final === true);
  if (!reply) return session.closed ? 'failed' : 'waiting';
  return reply.text.trim() === '你好，有什么能帮到你？' && !reply.awaiting_user ? 'success' : 'failed';
}
