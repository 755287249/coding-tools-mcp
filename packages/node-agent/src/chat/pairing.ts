import { randomBytes } from 'node:crypto';
import { realpathSync } from 'node:fs';

const TTL = 15 * 60_000;
const LIMIT = 256;
interface Entry { root: string; chat: string; attempt_id: string; ticket: string; expires_at: number; started_at: number | null }

/** Short-lived UI hints only. These tickets never authorize MCP, chat access or file reads. */
export class PairingRegistry {
  private entries = new Map<string, Entry>();
  constructor(private now: () => number = Date.now) {}
  private prune() {
    for (const [ticket, entry] of this.entries) if (entry.expires_at <= this.now()) this.entries.delete(ticket);
  }
  issue(root: string, chat: string, attempt: string) {
    this.prune();
    for (const [ticket, entry] of this.entries) if (entry.root === root && entry.chat === chat) {
      if (entry.attempt_id === attempt) return {ticket, attempt_id: attempt, expires_at: entry.expires_at};
      this.entries.delete(ticket);
    }
    if (this.entries.size >= LIMIT) this.entries.delete(this.entries.keys().next().value!);
    const ticket = randomBytes(16).toString('hex');
    const entry: Entry = {root, chat, attempt_id: attempt, ticket, expires_at: this.now() + TTL, started_at: null};
    this.entries.set(ticket, entry);
    return {ticket, attempt_id: attempt, expires_at: entry.expires_at};
  }
  mark(ticket: string, roots: string[]): boolean {
    if (!/^[a-f0-9]{32}$/.test(ticket)) return false;
    this.prune();
    const entry = this.entries.get(ticket);
    if (!entry || !roots.includes(entry.root)) return false;
    entry.started_at ??= this.now();
    return true;
  }
  status(root: string, chat: string) {
    this.prune();
    const entry = [...this.entries.values()].find(e => e.root === root && e.chat === chat);
    return entry ? {attempt_id: entry.attempt_id, expires_at: entry.expires_at, started_at: entry.started_at} : null;
  }
}

const registry = new PairingRegistry();
export const preparePairing = (root: string, chat: string, attempt: string) => registry.issue(realpathSync(root), chat, attempt);
export const pairingStatus = (root: string, chat: string) => registry.status(realpathSync(root), chat);
export const markPairing = (ticket: string, roots: string[]) => registry.mark(ticket, roots.map(root => realpathSync(root)));
