import { spawn } from 'node:child_process';
import { readdir, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import type { AgentConfig } from './types.js';
import { normalizeWorkspacePath } from './wsl.js';

const MAX_DIRECTORY_ENTRIES = 2_000;

export interface ManagementDirectoryEntry {
  name: string;
  path: string;
}

export interface ManagementDirectoryPayload {
  ok: true;
  path: string;
  parent: string | null;
  roots: string[];
  directories: ManagementDirectoryEntry[];
  totalDirectories: number;
  truncated: boolean;
}

function absoluteDirectory(value: string): string {
  if (value.length > 4_096) throw new Error('Directory path exceeds 4096 characters.');
  const normalized = normalizeWorkspacePath(value);
  if (!path.isAbsolute(normalized)) throw new Error('Directory path must be absolute.');
  return path.normalize(normalized);
}

function addRoot(roots: Set<string>, value: string | undefined): void {
  if (!value?.trim()) return;
  const normalized = normalizeWorkspacePath(value.trim());
  if (!path.isAbsolute(normalized)) return;
  const root = path.parse(normalized).root;
  if (root) roots.add(path.normalize(root));
}

function directoryRoots(config: AgentConfig, selected: string): string[] {
  const roots = new Set<string>();
  addRoot(roots, selected);
  addRoot(roots, homedir());
  addRoot(roots, config.dataDir);
  for (const folder of config.folders) addRoot(roots, folder.path);
  return [...roots].sort((left, right) => left.localeCompare(right, undefined, { numeric: true, sensitivity: 'base' }));
}

export async function managementDirectoryPayload(config: AgentConfig, requestedPath?: string | null): Promise<ManagementDirectoryPayload> {
  const fallback = config.folders[0]?.path || homedir() || process.cwd();
  const selected = absoluteDirectory(requestedPath?.trim() || fallback);
  const metadata = await stat(selected);
  if (!metadata.isDirectory()) throw new Error('Selected path is not a directory.');

  const entries = await readdir(selected, { withFileTypes: true });
  const allDirectories = entries
    .filter(entry => entry.isDirectory())
    .map(entry => ({ name: entry.name, path: path.join(selected, entry.name) }))
    .sort((left, right) => left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: 'base' }));
  const parent = path.dirname(selected);

  return {
    ok: true,
    path: selected,
    parent: parent === selected ? null : parent,
    roots: directoryRoots(config, selected),
    directories: allDirectories.slice(0, MAX_DIRECTORY_ENTRIES),
    totalDirectories: allDirectories.length,
    truncated: allDirectories.length > MAX_DIRECTORY_ENTRIES
  };
}

export async function openManagementDirectory(requestedPath: string): Promise<{ ok: true; path: string }> {
  const selected = absoluteDirectory(requestedPath);
  const metadata = await stat(selected);
  if (!metadata.isDirectory()) throw new Error('Selected path is not a directory.');
  const command = process.platform === 'win32' ? 'explorer' : process.platform === 'darwin' ? 'open' : 'xdg-open';
  await new Promise<void>((resolve, reject) => {
    const child = spawn(command, [selected], { detached: true, stdio: 'ignore', shell: false });
    child.once('error', reject);
    child.once('spawn', () => { child.unref(); resolve(); });
  });
  return { ok: true, path: selected };
}
