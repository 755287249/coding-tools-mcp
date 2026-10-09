import type { ChatMessage } from '../api/chat';

export type ChatFeedItem =
  | { kind: 'message'; id: string; message: ChatMessage }
  | { kind: 'tools'; id: string; messages: ChatMessage[] };

/** Keep each task's tool reports together even when prose progress is interleaved. */
export function groupChatMessages(messages: ChatMessage[]): ChatFeedItem[] {
  const feed: ChatFeedItem[] = [];
  const groups = new Map<string, Extract<ChatFeedItem, { kind: 'tools' }>>();
  for (const message of messages) {
    if (message.role !== 'assistant' || !message.tool_event) {
      feed.push({ kind: 'message', id: `message:${message.id}`, message });
      continue;
    }
    const key = message.reply_to ?? message.id;
    let group = groups.get(key);
    if (!group) {
      group = { kind: 'tools', id: `tools:${key}`, messages: [] };
      groups.set(key, group);
      feed.push(group);
    }
    group.messages.push(message);
  }
  return feed;
}

/** Count outstanding starts; do not guess which concurrent command produced an output. */
export function summarizeToolActivity(messages: ChatMessage[]) {
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
  return { calls, completed, failed, running: [...pending.values()].reduce((sum, count) => sum + count, 0) };
}
