import { writable } from 'svelte/store';
import { browser } from '$app/environment';

/** Built-in app plugins that run inside Coding Tools MCP (not MCP servers). */
export interface AppPlugin { id: string; name: string; summary: string; version: string; userscript?: string }
export const APP_PLUGINS: AppPlugin[] = [
  { id: 'coderabbit-seeds', name: 'CodeRabbit', summary: 'rabbit.desc', version: '1.4.0', userscript: 'coderabbit-seeds.user.js' },
];
export const APP_PLUGIN_PREFIX = 'app:';

const KEY = 'ctmcp-app-plugins-v1';
function load(): string[] {
  if (!browser) return [];
  try { const value = JSON.parse(localStorage.getItem(KEY) ?? '[]'); return Array.isArray(value) ? value.filter(id => APP_PLUGINS.some(p => p.id === id)) : []; }
  catch { return []; }
}
/** IDs of the app plugins the user added. Saved per device. */
export const installedAppPlugins = writable<string[]>(load());
if (browser) installedAppPlugins.subscribe(ids => { try { localStorage.setItem(KEY, JSON.stringify(ids)); } catch { /* storage may be blocked */ } });

export function setAppPlugin(id: string, installed: boolean): void {
  installedAppPlugins.update(ids => installed ? [...new Set([...ids, id])] : ids.filter(item => item !== id));
}
