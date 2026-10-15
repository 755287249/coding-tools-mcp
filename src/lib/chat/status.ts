import type { ChatSession } from '../api/chat';

export const USER_MESSAGE_STATUS_KEYS = { queued: 'chat.81', processing: 'chat.82', awaiting_user: 'chat.73', replied: 'chat.83', closed: 'chat.4', recalled: 'chat.recalled', interrupted: 'chat.interruptedTag' } as const;

/** A recalled or paused/replaced message no longer needs an answer. */
export const messageStopped = (m: {recalled_at?: number; interrupted_at?: number} | undefined) => typeof m?.recalled_at === 'number' || typeof m?.interrupted_at === 'number';

export function messageAnswered(session:ChatSession|null,messageId:string):boolean {
 const messages=session?.messages??[];const message=messages.find(m=>m.id===messageId);
 if(messageStopped(message))return true;
 const replies=messages.filter(r=>r.role==='assistant'&&r.reply_to===messageId&&r.final===true);
 return message?.recipient_ids?.length?message.recipient_ids.every(id=>replies.some(r=>r.agent_id===id)):replies.length>0;
}
export function chatUserState(session: ChatSession | null, messageId: string): {read: boolean; status: keyof typeof USER_MESSAGE_STATUS_KEYS} | null {
  const messages = session?.messages ?? [];
  const message = messages.find(item => item.id === messageId && item.role === 'user');
  if (!message) return null;
  const replies = messages.filter(reply => reply.role === 'assistant' && reply.reply_to === message.id);
  const read = !!message.received_at || replies.length > 0;
  if (typeof message.recalled_at === 'number') return { read, status: 'recalled' };
  const final = replies.filter(reply => reply.final === true).at(-1);
  if (typeof message.interrupted_at === 'number' && !final) return { read, status: 'interrupted' };
  if (final && messageAnswered(session,messageId)) {
    const answered = messages.slice(messages.indexOf(final) + 1).some(next => next.role === 'user');
    return { read, status: final.awaiting_user && !answered ? session?.closed ? 'closed' : 'awaiting_user' : 'replied' };
  }
  return { read, status: session?.closed ? 'closed' : read ? 'processing' : 'queued' };
}

/** Receipt/progress is evidence of pickup; an open connection alone is not. */
export function pendingChatState(session: ChatSession | null): 'queued' | 'processing' | 'interrupted' | 'awaiting_user' | null {
  if (!session || session.closed) return null;
  const messages = session.messages ?? [];
  const pending = messages.find(message => message.role === 'user' && !messageAnswered(session,message.id));
  if (!pending) {
    const latestUser = messages.filter(message => message.role === 'user' && typeof message.recalled_at !== 'number').at(-1);
    return latestUser && chatUserState(session, latestUser.id)?.status === 'awaiting_user' ? 'awaiting_user' : null;
  }
  const accepted = !!pending.received_at || messages.some(reply => reply.role === 'assistant' && reply.reply_to === pending.id);
  if (!accepted) return 'queued';
  return session.status === 'offline' ? 'interrupted' : 'processing';
}

export function chatReplyState(session: ChatSession | null, messageId: string): 'supplementing' | 'awaiting_user' | 'answered' | 'complete' | null {
  const messages = session?.messages ?? [];
  const index = messages.findIndex(message => message.id === messageId);
  const message = messages[index];
  if (!message || message.role !== 'assistant' || message.tool_event) return null;
  if (message.awaiting_user) {
    if (messages.slice(index + 1).some(next => next.role === 'user')) return 'answered';
    return session?.closed ? null : 'awaiting_user';
  }
  if (message.final === false) {
    return session?.closed || messages.some(reply => reply.role === 'assistant' && reply.reply_to === message.reply_to && reply.agent_id===message.agent_id && reply.final === true) ? null : 'supplementing';
  }
  return 'complete';
}
