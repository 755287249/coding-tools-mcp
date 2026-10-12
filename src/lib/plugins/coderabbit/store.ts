import { writable } from 'svelte/store';
import type { CodeRabbitSession } from './engine';

export interface SeedingJob {
  id: number; total: number; done: number; status: 'running' | 'done' | 'error' | 'stopped';
  message: string; repo?: string; minimized: boolean; workspaceId: string; folderId: string;
}
/** One background seeding job at a time; shown by the top progress bar. */
export const seedingJob = writable<SeedingJob | null>(null);
/** CodeRabbit session kept in memory only (never written to disk or local storage). */
export const coderabbitSession = writable<CodeRabbitSession | null>(null);
/** A pending GitHub device-flow code the user must enter on github.com. */
export const githubDeviceCode = writable<{ user_code: string; verification_uri: string } | null>(null);

let controller: AbortController | null = null;
export function beginJob(): AbortSignal { controller?.abort(); controller = new AbortController(); return controller.signal; }
export function stopJob(): void { controller?.abort(); }
