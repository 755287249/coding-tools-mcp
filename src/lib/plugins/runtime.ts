import { writable } from 'svelte/store';
export interface PluginSlotState { id: string; workspaceId: string; folderId: string; element: HTMLElement; rect: DOMRect }
export const pluginSlot = writable<PluginSlotState | null>(null);
export interface PluginProgress { pluginId: string; name: string; total: number; done: number; status: 'running'|'done'|'error'|'stopped'; message: string; detail?: string; minimized: boolean }
export const pluginProgress = writable<PluginProgress | null>(null);
export const pluginStop = writable(0);
export function stopPluginJob(): void { pluginStop.update(value => value + 1); }
