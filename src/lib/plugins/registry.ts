import { browser } from '$app/environment';
import { appUrl } from '$lib/app-path';
import { getBackend } from '$lib/backend';
import { writable, get } from 'svelte/store';
import type { PluginPackage } from './types';

export interface AppPlugin { id: string; name: string; summary: string; version: string; builtIn: boolean; userscript?: string }
export const APP_PLUGIN_PREFIX = 'app:';
const KEY = 'ctmcp-app-plugins-v1';
const BUILT_INS = [{ id: 'coderabbit-seeds', file: 'coderabbit-seeds.ctplugin.json', userscript: 'coderabbit-seeds.user.js' }];
function selected(): string[] { try { const value = JSON.parse(localStorage.getItem(KEY) ?? '[]'); return Array.isArray(value) ? value : []; } catch { return []; } }

export const pluginPackages = writable<PluginPackage[]>([]);
export const APP_PLUGINS = writable<AppPlugin[]>([]);
export const installedAppPlugins = writable<string[]>(browser ? selected() : []);
if (browser) installedAppPlugins.subscribe(ids => { try { localStorage.setItem(KEY, JSON.stringify(ids)); } catch {} });

function row(plugin: PluginPackage, builtIn: boolean, userscript?: string): AppPlugin {
  return { id: plugin.manifest.id, name: plugin.manifest.name, summary: plugin.manifest.description ?? '', version: plugin.manifest.version, builtIn, userscript };
}

export async function refreshPlugins(): Promise<void> {
  if (!browser) return;
  const built = await Promise.all(BUILT_INS.map(async item => { const response = await fetch(appUrl(`/plugins/${item.file}`)); if (!response.ok) throw new Error(`Built-in plugin ${item.id} is missing`); return { package: await response.json() as PluginPackage, item }; }));
  const users = await getBackend().plugins.list();
  const packages = [...built.map(item => item.package), ...users.filter(user => !built.some(item => item.package.manifest.id === user.manifest.id))];
  pluginPackages.set(packages);
  APP_PLUGINS.set([...built.map(item => row(item.package, true, item.item.userscript)), ...users.filter(user => !built.some(item => item.package.manifest.id === user.manifest.id)).map(user => row(user, false))]);
  installedAppPlugins.update(ids => [...new Set([...ids.filter(id => packages.some(plugin => plugin.manifest.id === id)), ...users.map(plugin => plugin.manifest.id)])]);
}

export function setAppPlugin(id: string, installed: boolean): void {
  installedAppPlugins.update(ids => installed ? [...new Set([...ids, id])] : ids.filter(item => item !== id));
}

export async function importPlugin(plugin: PluginPackage): Promise<void> { await getBackend().plugins.install(plugin); await refreshPlugins(); setAppPlugin(plugin.manifest.id, true); }
export async function removeAppPlugin(id: string): Promise<void> {
  const item = get(APP_PLUGINS).find(plugin => plugin.id === id); if (!item) return;
  setAppPlugin(id, false); if (!item.builtIn) { await getBackend().plugins.remove(id); await refreshPlugins(); }
}
