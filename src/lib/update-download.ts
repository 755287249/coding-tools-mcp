import { get, writable } from 'svelte/store';

export interface DownloadProgress {
  phase: 'connecting' | 'downloading' | 'verifying';
  downloaded: number;
  total: number | null;
}
export interface UpdateSelection { repo: string; version: string; sha256: string }
export interface DownloadState {
  phase: DownloadProgress['phase'] | 'idle' | 'ready' | 'error';
  downloaded: number;
  total: number | null;
  error: string;
}
type Downloader = (selection: UpdateSelection, progress: (event: DownloadProgress) => void) => Promise<{ bytes: number }>;
const empty = (): DownloadState => ({ phase: 'idle', downloaded: 0, total: null, error: '' });
export const downloadBusy = (state: DownloadState) => ['connecting', 'downloading', 'verifying'].includes(state.phase);
export function downloadPercent(state: Pick<DownloadState, 'downloaded' | 'total'>): number | undefined {
  return state.total && state.total > 0 ? Math.min(100, Math.floor(state.downloaded / state.total * 100)) : undefined;
}
export function formatDownloadBytes(bytes: number): string {
  return bytes >= 1024 * 1024 ? `${(bytes / (1024 * 1024)).toFixed(1)} MiB` : `${Math.round(bytes / 1024)} KiB`;
}

// Owned by the application store, so leaving the updates page cannot lose an active transfer.
export function createUpdateDownload(downloader: Downloader) {
  const state = writable<DownloadState>(empty());
  return {
    subscribe: state.subscribe,
    reset() {
      if (downloadBusy(get(state))) return false;
      state.set(empty());
      return true;
    },
    async start(selection: UpdateSelection) {
      if (downloadBusy(get(state))) return;
      state.set({ ...empty(), phase: 'connecting' });
      let active = true;
      try {
        const result = await downloader(selection, (event) => {
          if (active) state.set({ ...event, error: '' });
        });
        state.set({ phase: 'ready', downloaded: result.bytes, total: result.bytes, error: '' });
      } catch (error) {
        state.update((current) => ({ ...current, phase: 'error', error: String(error) }));
      } finally { active = false; }
    },
  };
}
