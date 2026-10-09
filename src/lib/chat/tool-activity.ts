import type { ChatMessage } from '../api/chat';

export type ChatFeedItem =
  | { kind: 'message'; id: string; message: ChatMessage }
  | { kind: 'tools'; id: string; messages: ChatMessage[]; settled: boolean };

/** Keep each task's tool reports together even when prose progress is interleaved. */
export function groupChatMessages(messages: ChatMessage[], sessionClosed = false): ChatFeedItem[] {
  const feed: ChatFeedItem[] = [];
  const groups = new Map<string, Extract<ChatFeedItem, { kind: 'tools' }>>();
  const finishedRequests = new Set(messages
    .filter(message => message.role === 'assistant' && message.final === true && message.reply_to)
    .map(message => message.agent_id?message.agent_id+':'+message.reply_to:message.reply_to));
  for (const message of messages) {
    if (message.role !== 'assistant' || !message.tool_event) {
      feed.push({ kind: 'message', id: `message:${message.id}`, message });
      continue;
    }
    const key = message.agent_id?message.agent_id+':'+(message.reply_to??message.id):message.reply_to ?? message.id;
    let group = groups.get(key);
    if (!group) {
      group = { kind: 'tools', id: `tools:${key}`, messages: [], settled: sessionClosed || finishedRequests.has(key) };
      groups.set(key, group);
      feed.push(group);
    }
    group.messages.push(message);
  }
  return feed;
}

/** Count outstanding starts; do not guess which concurrent command produced an output. */
export function summarizeToolActivity(messages: ChatMessage[], settled = false) {
  const pending = new Map<string, number>();
  let calls = 0, completed = 0, failed = 0;
  for (const message of messages) {
    const event = message.tool_event;
    if (!event) continue;
    const count = pending.get(event.name) ?? 0;
    if (event.status === 'running') {
      calls++;
      pending.set(event.name, count + 1);
    } else {
      if (count) pending.set(event.name, count - 1);
      else calls++; // Older callers sometimes report only the result.
      if (event.status === 'failed') failed++;
      else completed++;
    }
  }
  const outstanding = [...pending.values()].reduce((sum, count) => sum + count, 0);
  // A finished reply ends the activity report, not necessarily the underlying process.
  // Keep missing results explicit instead of inventing success or spinning forever.
  return { calls, completed, failed, running: settled ? 0 : outstanding, unresolved: settled ? outstanding : 0 };
}
