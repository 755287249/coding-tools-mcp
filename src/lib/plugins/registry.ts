import { browser } from '$app/environment';
import { getBackend } from '$lib/backend';
import { writable, get } from 'svelte/store';
import { translate } from '$lib/i18n';
import type { PluginPackage } from './types';
// Built-in packages are bundled into the app instead of fetched at runtime: the desktop CSP
// (`connect-src`) blocks same-origin fetch in release builds, which left the plugin list empty.
import coderabbitSeeds from '../../../plugins/coderabbit/dist/coderabbit-seeds.ctplugin.json';

export interface AppPlugin { id: string; name: string; summary: string; version: string; builtIn: boolean; userscript?: string; userscriptVersion?: string }
export const APP_PLUGIN_PREFIX = 'app:';
const KEY = 'ctmcp-app-plugins-v2';
const BUILT_INS: { package: PluginPackage; userscript?: string; userscriptVersion?: string }[] = [
  { package: coderabbitSeeds as PluginPackage, userscript: 'coderabbit-seeds.user.js', userscriptVersion: '1.4.0' },
];
function selected(): string[] { try { const value = JSON.parse(localStorage.getItem(KEY) ?? '[]'); return Array.isArray(value) ? value.filter(item => typeof item === 'string') : []; } catch { return []; } }

export const pluginPackages = writable<PluginPackage[]>(BUILT_INS.map(item => item.package));
export const APP_PLUGINS = writable<AppPlugin[]>(BUILT_INS.map(item => row(item.package, true, item.userscript, item.userscriptVersion)));
/** IDs added on this device. Built-ins are only added when the user clicks "add"; imported packages are added on import. */
export const installedAppPlugins = writable<string[]>(browser ? selected() : []);
/** Last error from loading user plugins (shown in the plugin page instead of failing silently). */
export const pluginError = writable('');
if (browser) installedAppPlugins.subscribe(ids => { try { localStorage.setItem(KEY, JSON.stringify(ids)); } catch {} });

function row(plugin: PluginPackage, builtIn: boolean, userscript?: string, userscriptVersion?: string): AppPlugin {
  return { id: plugin.manifest.id, name: plugin.manifest.name, summary: plugin.manifest.description ?? '', version: plugin.manifest.version, builtIn, userscript, userscriptVersion };
}

const PLUGIN_ID = /^[a-z][a-z0-9-]{1,63}$/;
/** Validates a parsed `.ctplugin.json` and returns a readable error instead of a backend failure. */
export function validatePluginPackage(value: unknown): PluginPackage {
  const plugin = value as Partial<PluginPackage> | null;
  if (!plugin || typeof plugin !== 'object' || Array.isArray(plugin)) throw new Error(translate('plugins.errNotObject'));
  if (plugin.format !== 1 || !plugin.manifest || typeof plugin.code !== 'string' || !plugin.code.trim()) throw new Error(translate('plugins.errNotPackage'));
  const manifest = plugin.manifest;
  if (typeof manifest.id !== 'string' || !PLUGIN_ID.test(manifest.id)) throw new Error(translate('plugins.errId'));
  if (typeof manifest.name !== 'string' || !manifest.name.trim()) throw new Error(translate('plugins.errName'));
  if (typeof manifest.version !== 'string' || !manifest.version.trim()) throw new Error(translate('plugins.errName'));
  return plugin as PluginPackage;
}

let users: PluginPackage[] = [];
function publish(): void {
  const builtIds = new Set(BUILT_INS.map(item => item.package.manifest.id));
  const extra = users.filter(user => !builtIds.has(user.manifest.id));
  pluginPackages.set([...BUILT_INS.map(item => item.package), ...extra]);
  APP_PLUGINS.set([...BUILT_INS.map(item => row(item.package, true, item.userscript, item.userscriptVersion)), ...extra.map(user => row(user, false))]);
  const known = new Set([...builtIds, ...extra.map(user => user.manifest.id)]);
  // Imported packages count as added; unknown IDs (e.g. from older builds) are dropped.
  installedAppPlugins.update(ids => { const next = [...new Set([...ids.filter(id => known.has(id)), ...extra.map(user => user.manifest.id)])]; return next.length === ids.length && next.every((id, index) => id === ids[index]) ? ids : next; });
}

export async function refreshPlugins(): Promise<void> {
  if (!browser) return;
  try {
    users = (await getBackend().plugins.list()).filter(item => { try { validatePluginPackage(item); return true; } catch { return false; } });
    pluginError.set('');
  } catch (error) {
    pluginError.set(translate('plugins.errList', { error: error instanceof Error ? error.message : String(error) }));
  }
  publish();
}

export function setAppPlugin(id: string, installed: boolean): void {
  installedAppPlugins.update(ids => installed ? [...new Set([...ids, id])] : ids.filter(item => item !== id));
}

export async function importPlugin(value: unknown): Promise<PluginPackage> {
  const plugin = validatePluginPackage(value);
  await getBackend().plugins.install(plugin);
  users = [...users.filter(item => item.manifest.id !== plugin.manifest.id), plugin];
  publish();
  setAppPlugin(plugin.manifest.id, true);
  void refreshPlugins();
  return plugin;
}

export async function removeAppPlugin(id: string): Promise<void> {
  const item = get(APP_PLUGINS).find(plugin => plugin.id === id); if (!item) return;
  setAppPlugin(id, false);
  if (!item.builtIn) { await getBackend().plugins.remove(id); users = users.filter(plugin => plugin.manifest.id !== id); publish(); }
}
