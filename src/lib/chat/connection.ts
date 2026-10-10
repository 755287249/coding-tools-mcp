import type { ChatSession } from '$lib/api/chat';

export function visibleChatMessages(session: ChatSession | null) {
  return (session?.messages ?? []).filter(message => message.kind !== 'connection_request');
}

/** Only a persisted final greeting to this exact control message completes pairing. */
export function connectionResult(session: ChatSession | null, requestId: string): 'waiting' | 'success' | 'failed' {
  if (!session?.messages?.some(message => message.id === requestId && message.kind === 'connection_request')) return 'waiting';
  const reply = session.messages.find(message => message.role === 'assistant' && message.reply_to === requestId && message.final === true);
  if (!reply) return session.closed ? 'failed' : 'waiting';
  const normalized = reply.text.replace(/[\s\p{P}\p{S}]/gu, '');
  return normalized.startsWith('你好有什么能帮到你') && !reply.awaiting_user ? 'success' : 'failed';
}

/** A connection request or AI join starts a conversation even before the first user task. */
export function isPristineConversation(session: ChatSession | null): boolean {
  return !!session && !session.closed && !(session.messages?.length) && !(session.queued_messages?.length)
    && !(session.members?.length) && !session.connection_id && !session.agent_name
    && session.status !== 'connected' && session.status !== 'waiting';
}

/** Greeting/control traffic does not lock the choice; the first actual user message does. */
export function canChooseConversationMode(session: ChatSession | null): boolean {
  return !!session && !session.closed && ![...session.messages??[],...session.queued_messages??[]].some(message=>message.role==='user'&&message.kind!=='connection_request');
}
