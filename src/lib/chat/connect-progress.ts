import { writable } from 'svelte/store';
import type { ChatSession } from '../api/chat';
import { connectionResult } from './connection';

/**
 * Accurate pairing phase. An AI that holds a lease but has not answered the greeting
 * is not "connected": user messages still queue behind the pairing request.
 */
export type ConnectPhase = 'none' | 'waiting_ai' | 'greeting' | 'failed';

export function connectPhase(session: ChatSession | null | undefined): ConnectPhase {
  if (!session || session.closed || session.mode === 'group') return 'none';
  const online = session.status === 'connected' || session.status === 'waiting';
  if (session.messages) {
    const request = [...session.messages].reverse().find(m => m.kind === 'connection_request');
    if (!request) return 'none';
    const result = connectionResult(session, request.id);
    if (result === 'failed') return 'failed';
    if (result === 'success') return 'none';
    return online || request.received_at ? 'greeting' : 'waiting_ai';
  }
  // List rows carry no messages; the server reports the pending pairing directly.
  if (!session.connecting) return 'none';
  return online ? 'greeting' : 'waiting_ai';
}

/** Rows that finished pairing while the user was elsewhere stay highlighted until opened. */
const HIGHLIGHT_MS = 10 * 60_000;
export const justConnected = writable<Record<string, number>>({});
const lastConnecting = new Map<string, boolean>();

export function connectRowKey(workspace: string, folder: string, chat: string): string {
  return JSON.stringify([workspace, folder, chat]);
}

/** Feed every list snapshot; returns nothing, updates `justConnected` on transitions. */
export function trackConnectRows(rows: { key: string; session: ChatSession }[], now = Date.now()): void {
  const finished: string[] = [];
  for (const { key, session } of rows) {
    const was = lastConnecting.get(key) === true;
    const is = connectPhase(session) !== 'none';
    lastConnecting.set(key, is);
    const online = session.status === 'connected' || session.status === 'waiting';
    if (was && !is && !session.closed && online) finished.push(key);
  }
  justConnected.update(current => {
    const next: Record<string, number> = {};
    for (const [key, at] of Object.entries(current)) if (now - at < HIGHLIGHT_MS) next[key] = at;
    for (const key of finished) next[key] = now;
    return next;
  });
}

export function clearJustConnected(key: string): void {
  justConnected.update(current => {
    if (!(key in current)) return current;
    const next = { ...current };
    delete next[key];
    return next;
  });
}
