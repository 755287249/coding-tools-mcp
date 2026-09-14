import { createHash } from 'node:crypto';
import { statSync } from 'node:fs';
import { access, lstat, readFile, readdir, realpath, stat } from 'node:fs/promises';
import path from 'node:path';
import type { ToolContext, WorkspaceFolder } from './types.js';
import { extendIgnoreRules, isIgnoredByRules, rootIgnoreRules, type IgnoreRule } from './gitignore.js';
import { MAX_TEXT_BYTES, readDecodedTextFile } from './textCodec.js';
import {
  looksLikeWindowsDrivePath, normalizeWorkspacePath, parseWslUncPath,
  sameWorkspacePath, workspacePathIdentity, WslRoutingError, wslUncPath
} from './wsl.js';
import { currentExecutionBinding } from './executionScope.js';
import { ConversationRoutingError } from './conversation.js';

export { MAX_TEXT_BYTES } from './textCodec.js';

export function selectedFolder(ctx: ToolContext, key: string): WorkspaceFolder {
  const id = currentExecutionBinding(ctx, key)?.folderId ?? ctx.selections.get(key);
  if (!id) {
    throw new ConversationRoutingError(
      'WORKSPACE_FOLDER_NOT_SELECTED',
      'This conversation has not selected a workspace folder. Call list_workspace_folders and switch_workspace_folder first.',
      true
    );
  }
  const folder = ctx.config.folders.find(item => item.id === id);
  if (!folder) {
    throw new ConversationRoutingError(
      'WORKSPACE_FOLDER_NOT_FOUND',
      `The selected workspace folder is no longer configured: ${id}`,
      true,
      { folder_id: id }
    );
  }
  return folder;
}

export function selectedFolderSafe(ctx: ToolContext, key: string): WorkspaceFolder | undefined {
  try { return selectedFolder(ctx, key); } catch { return undefined; }
}

function outside(relative: string, separator: string, absolute: boolean): boolean {
  return relative === '..' || relative.startsWith(`..${separator}`) || absolute;
}

export class WorkspacePathError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly category: string,
    readonly retryable = false,
    readonly details: Record<string, unknown> = {}
  ) {
    super(message);
    this.name = 'WorkspacePathError';
  }
}

export interface ResolvedWorkspacePath {
  root: string;
  candidate: string;
  full: string;
  display: string;
  existed: boolean;
  linkedComponent?: string;
}

function invalidPath(message: string): WorkspacePathError {
  return new WorkspacePathError('INVALID_ARGUMENT', message, 'validation');
}

function absolutePathDenied(): WorkspacePathError {
  return new WorkspacePathError('ABSOLUTE_PATH_DENIED', 'Absolute paths are denied.', 'security');
}

function pathOutsideWorkspace(): WorkspacePathError {
  return new WorkspacePathError('PATH_OUTSIDE_WORKSPACE', 'Path escapes the configured workspace.', 'security');
}

function symlinkEscape(): WorkspacePathError {
  return new WorkspacePathError('SYMLINK_ESCAPE', 'Path escapes the configured workspace.', 'security');
}

function notFound(raw: string): WorkspacePathError {
  return new WorkspacePathError('NOT_FOUND', `Path not found: ${raw}`, 'not_found');
}

function notDirectory(message: string): WorkspacePathError {
  return new WorkspacePathError('NOT_A_DIRECTORY', message, 'validation');
}

function isFsCode(error: unknown, ...codes: string[]): boolean {
  return Boolean(error && typeof error === 'object' && 'code' in error && codes.includes(String((error as NodeJS.ErrnoException).code)));
}

export function validateWorkspaceUserPath(value: string, options: { allowDot?: boolean } = {}): string {
  if (!value) throw invalidPath('Path must be a non-empty string');
  if (value.includes('\0')) throw invalidPath('Path contains a NUL byte');
  if (
    value.startsWith('/')
    || value.startsWith('\\')
    || /^[A-Za-z]:/.test(value)
    || path.posix.isAbsolute(value)
    || path.win32.isAbsolute(value)
  ) throw absolutePathDenied();
  const normalized = value.replaceAll('\\', '/');
  if (normalized.split('/').includes('..')) throw pathOutsideWorkspace();
  if (options.allowDot === false && (normalized === '.' || normalized.endsWith('/.'))) {
    throw invalidPath('Invalid write target');
  }
  return value;
}

export function rejectProtectedWritePath(value: string): void {
  const normalized = value.replaceAll('\\', '/');
  const first = normalized.split('/').find(Boolean) ?? '';
  if (first === '.git' || first === '.github') {
    throw new WorkspacePathError(
      'PROTECTED_PATH',
      `Protected repository path cannot be modified: ${value}`,
      'security'
    );
  }
}

export async function canonicalWorkspaceRoot(rootValue: string): Promise<string> {
  const root = normalizeWorkspacePath(rootValue);
  try {
    const resolved = normalizeWorkspacePath(await realpath(root));
    if (!(await stat(resolved)).isDirectory()) throw invalidPath('Workspace root must be a directory');
    return resolved;
  } catch (error) {
    if (error instanceof WorkspacePathError) throw error;
    throw invalidPath('Workspace root must exist');
  }
}

export function validateUniqueWorkspaceFolders(folders: readonly WorkspaceFolder[]): void {
  const ids = new Set<string>();
  const roots = new Map<string, WorkspaceFolder>();
  for (const folder of folders) {
    if (!folder.id || ids.has(folder.id)) throw new Error(`workspace folder id must be unique: ${folder.id}`);
    ids.add(folder.id);
    const identity = workspacePathIdentity(folder.path);
    const existing = roots.get(identity) ?? [...roots.values()].find(candidate => sameWorkspacePath(candidate.path, folder.path));
    if (existing) {
      throw new WorkspacePathError(
        'WORKSPACE_FOLDER_DUPLICATE_ROOT',
        `The same physical workspace root cannot use multiple folder IDs: ${existing.id} and ${folder.id}`,
        'workspace_routing',
        false,
        { folder_id: folder.id, existing_folder_id: existing.id, path: folder.path }
      );
    }
    roots.set(identity, folder);
  }
}

export async function canonicalizeWorkspaceFolders(folders: readonly WorkspaceFolder[]): Promise<WorkspaceFolder[]> {
  const canonical = await Promise.all(folders.map(async folder => ({
    ...folder,
    path: await canonicalWorkspaceRoot(folder.path)
  })));
  validateUniqueWorkspaceFolders(canonical);
  return canonical;
}

function displayInside(root: string, full: string): string {
  return relativeInside(root, full).replaceAll('\\', '/') || '.';
}

async function firstLinkedComponent(root: string, candidate: string): Promise<string | undefined> {
  const relative = relativeInside(root, candidate).replaceAll('\\', '/');
  if (relative === '.') return undefined;
  let current = root;
  for (const part of relative.split('/').filter(Boolean)) {
    current = resolveInside(root, path.join(displayInside(root, current), part));
    try {
      if ((await lstat(current)).isSymbolicLink()) return current;
    } catch (error) {
      if (isFsCode(error, 'ENOENT', 'ENOTDIR')) return undefined;
      throw error;
    }
  }
  return undefined;
}

function ensureCanonicalInside(root: string, full: string, linkedComponent?: string): void {
  try {
    relativeInside(root, full);
  } catch {
    throw linkedComponent ? symlinkEscape() : pathOutsideWorkspace();
  }
}

function appendRelative(base: string, relative: string): string {
  const location = parseWslUncPath(base);
  return location
    ? wslUncPath(location.distro, path.posix.resolve(location.linuxPath, relative.replaceAll('\\', '/')))
    : path.resolve(base, relative);
}

export async function resolveExistingPath(rootValue: string, rawValue: string): Promise<ResolvedWorkspacePath> {
  const raw = validateWorkspaceUserPath(rawValue);
  const root = await canonicalWorkspaceRoot(rootValue);
  const candidate = resolveInside(root, raw);
  const linkedComponent = await firstLinkedComponent(root, candidate);
  let full: string;
  try {
    full = normalizeWorkspacePath(await realpath(candidate));
  } catch {
    throw notFound(raw);
  }
  ensureCanonicalInside(root, full, linkedComponent);
  return {
    root,
    candidate,
    full,
    display: displayInside(root, full),
    existed: true,
    ...(linkedComponent ? { linkedComponent } : {})
  };
}

export async function resolveExistingDirectory(
  rootValue: string,
  rawValue: string,
  message = 'Path must be a directory'
): Promise<ResolvedWorkspacePath> {
  const resolved = await resolveExistingPath(rootValue, rawValue);
  if (!(await stat(resolved.full)).isDirectory()) throw notDirectory(message);
  return resolved;
}

export async function rejectDirectWriteSymlink(resolved: ResolvedWorkspacePath): Promise<void> {
  try {
    if ((await lstat(resolved.candidate)).isSymbolicLink()) throw symlinkEscape();
  } catch (error) {
    if (error instanceof WorkspacePathError) throw error;
    if (!isFsCode(error, 'ENOENT')) throw error;
  }
}

export async function resolveExistingWritePath(rootValue: string, rawValue: string): Promise<ResolvedWorkspacePath> {
  const resolved = await resolveExistingPath(rootValue, rawValue);
  await rejectDirectWriteSymlink(resolved);
  return resolved;
}

export async function resolveWritePath(rootValue: string, rawValue: string): Promise<ResolvedWorkspacePath> {
  const raw = validateWorkspaceUserPath(rawValue, { allowDot: false });
  const root = await canonicalWorkspaceRoot(rootValue);
  const candidate = resolveInside(root, raw);
  const linkedComponent = await firstLinkedComponent(root, candidate);
  try {
    await lstat(candidate);
    let full: string;
    try { full = normalizeWorkspacePath(await realpath(candidate)); }
    catch { throw notFound(raw); }
    ensureCanonicalInside(root, full, linkedComponent);
    return {
      root,
      candidate,
      full,
      display: displayInside(root, full),
      existed: true,
      ...(linkedComponent ? { linkedComponent } : {})
    };
  } catch (error) {
    if (error instanceof WorkspacePathError) throw error;
    if (!isFsCode(error, 'ENOENT', 'ENOTDIR')) throw error;
  }

  let ancestor = path.dirname(candidate);
  while (true) {
    try {
      const info = await stat(ancestor);
      if (!info.isDirectory()) throw notDirectory('Parent path is not a directory');
      break;
    } catch (error) {
      if (error instanceof WorkspacePathError) throw error;
      if (!isFsCode(error, 'ENOENT')) {
        if (isFsCode(error, 'ENOTDIR')) throw notDirectory('Parent path is not a directory');
        throw error;
      }
      if (displayInside(root, ancestor) === '.') throw notDirectory('Parent directory not found');
      ancestor = path.dirname(ancestor);
    }
  }

  let resolvedAncestor: string;
  try { resolvedAncestor = normalizeWorkspacePath(await realpath(ancestor)); }
  catch { throw notDirectory('Parent directory not found'); }
  ensureCanonicalInside(root, resolvedAncestor, linkedComponent);
  const suffix = path.relative(ancestor, candidate);
  const full = appendRelative(resolvedAncestor, suffix);
  ensureCanonicalInside(root, full, linkedComponent);
  return {
    root,
    candidate,
    full,
    display: raw.replaceAll('\\', '/'),
    existed: false,
    ...(linkedComponent ? { linkedComponent } : {})
  };
}

export function relativeInside(rootValue: string, fullValue: string): string {
  const rootLocation = parseWslUncPath(rootValue);
  const fullLocation = parseWslUncPath(fullValue);
  if (rootLocation || fullLocation) {
    if (!rootLocation || !fullLocation || rootLocation.distro.toLowerCase() !== fullLocation.distro.toLowerCase()) {
      throw new WslRoutingError('WSL_CROSS_DISTRIBUTION_PATH', 'Workspace paths must use the same WSL distribution');
    }
    const relative = path.posix.relative(rootLocation.linuxPath, fullLocation.linuxPath);
    if (outside(relative, '/', path.posix.isAbsolute(relative))) throw new Error('PATH_OUTSIDE_WORKSPACE');
    return relative || '.';
  }
  const root = path.resolve(rootValue);
  const full = path.resolve(fullValue);
  const relative = path.relative(root, full);
  if (outside(relative, path.sep, path.isAbsolute(relative))) throw new Error('PATH_OUTSIDE_WORKSPACE');
  return relative || '.';
}

export function resolveInside(rootValue: string, value: string): string {
  const rootLocation = parseWslUncPath(rootValue);
  if (rootLocation) {
    const explicitLocation = parseWslUncPath(value);
    if (explicitLocation && explicitLocation.distro.toLowerCase() !== rootLocation.distro.toLowerCase()) {
      throw new WslRoutingError(
        'WSL_CROSS_DISTRIBUTION_PATH',
        `Path references WSL distribution '${explicitLocation.distro}' while the workspace runs in '${rootLocation.distro}'`,
        'validation',
        false,
        { workspace_distro: rootLocation.distro, path_distro: explicitLocation.distro, path: value }
      );
    }
    if (!explicitLocation && looksLikeWindowsDrivePath(value)) throw new Error('PATH_OUTSIDE_WORKSPACE');
    const target = explicitLocation?.linuxPath
      ?? path.posix.resolve(rootLocation.linuxPath, (value || '.').replaceAll('\\', '/'));
    const relative = path.posix.relative(rootLocation.linuxPath, target);
    if (outside(relative, '/', path.posix.isAbsolute(relative))) throw new Error('PATH_OUTSIDE_WORKSPACE');
    return wslUncPath(rootLocation.distro, target);
  }
  const root = path.resolve(rootValue);
  const full = path.resolve(root, value || '.');
  const relative = path.relative(root, full);
  if (outside(relative, path.sep, path.isAbsolute(relative))) throw new Error('PATH_OUTSIDE_WORKSPACE');
  return full;
}

export function resolveFromWorkspace(root: string, cwd: string, value: string): string {
  const rootLocation = parseWslUncPath(root);
  const cwdLocation = parseWslUncPath(cwd);
  if (rootLocation) {
    if (!cwdLocation || cwdLocation.distro.toLowerCase() !== rootLocation.distro.toLowerCase()) {
      throw new WslRoutingError('WSL_CROSS_DISTRIBUTION_PATH', 'Command cwd must use the workspace WSL distribution');
    }
    const explicit = parseWslUncPath(value);
    const target = explicit?.linuxPath
      ?? path.posix.resolve(cwdLocation.linuxPath, value.replaceAll('\\', '/'));
    return resolveInside(root, explicit ? value : wslUncPath(rootLocation.distro, target));
  }
  return resolveInside(root, path.resolve(cwd, value));
}

function existingDirectoryInside(root: string, value: string): string | undefined {
  try {
    const cwd = resolveInside(root, value || '.');
    if (parseWslUncPath(root)) return cwd;
    return statSync(cwd).isDirectory() ? cwd : undefined;
  } catch {
    return undefined;
  }
}

export function validatedFolderCwd(ctx: ToolContext, key: string, folderId: string, preferred?: string): string {
  const folder = ctx.config.folders.find(item => item.id === folderId);
  if (!folder) return '.';
  const root = normalizeWorkspacePath(folder.path);
  const saved = preferred ?? ctx.conversations?.peekCwdFor(key, folderId) ?? '.';
  if (existingDirectoryInside(root, saved)) return saved || '.';
  ctx.conversations?.setFolderCwd(key, folderId, '.');
  return '.';
}

export function rootAndCwd(ctx: ToolContext, key: string): { folder: WorkspaceFolder; root: string; cwd: string } {
  const binding = currentExecutionBinding(ctx, key);
  const folder = selectedFolder(ctx, key);
  const root = normalizeWorkspacePath(folder.path);
  const saved = validatedFolderCwd(ctx, key, folder.id, binding?.defaultCwd ?? ctx.defaultCwds.get(key) ?? '.');
  const cwd = existingDirectoryInside(root, saved) ?? root;
  return { folder, root, cwd };
}

export async function exists(value: string): Promise<boolean> {
  try { await access(value); return true; } catch { return false; }
}

export async function sha256File(file: string): Promise<string> {
  return createHash('sha256').update(await readFile(file)).digest('hex');
}

export async function readText(file: string, maxBytes = MAX_TEXT_BYTES): Promise<string> {
  return (await readDecodedTextFile(file, maxBytes)).text;
}

export const DEFAULT_EXCLUDED_NAMES = new Set([
  '.git', '.reference', 'node_modules', 'target', 'dist', 'build',
  '.venv', 'venv', '.tox', '.mypy_cache', '.pytest_cache', '.ruff_cache', '__pycache__'
]);

export interface WalkOptions {
  maxDepth?: number;
  maxResults?: number;
  includeHidden?: boolean;
  includeIgnored?: boolean;
  includeGenerated?: boolean;
  includeDirectories?: boolean;
  excludedPrefixes?: string[];
}

export interface WalkEntry {
  path: string;
  type: 'file' | 'directory' | 'symlink';
  size?: number;
  modified?: string;
}

function hiddenPath(relative: string): boolean {
  return relative.split('/').some(part => part.startsWith('.') && part !== '.');
}

function fixedExcluded(name: string, options: WalkOptions): boolean {
  if (name.toLowerCase() === '.git') return true;
  return options.includeGenerated !== true && DEFAULT_EXCLUDED_NAMES.has(name);
}

function prefixExcluded(relative: string, options: WalkOptions): boolean {
  const normalized = relative.replaceAll('\\', '/');
  return (options.excludedPrefixes ?? []).some(prefix => normalized === prefix || normalized.startsWith(`${prefix}/`));
}

async function rulesForStart(root: string, start: string, options: WalkOptions): Promise<{ rules: IgnoreRule[]; blocked: boolean }> {
  let rules = options.includeIgnored === true ? [] : await rootIgnoreRules(root);
  const relativeStart = relativeInside(root, start).replaceAll('\\', '/');
  if (!relativeStart || relativeStart === '.') return { rules, blocked: false };
  let current = root;
  for (const part of relativeStart.split('/').filter(Boolean)) {
    current = path.join(current, part);
    const relative = relativeInside(root, current).replaceAll('\\', '/');
    if (fixedExcluded(part, options) || (!options.includeHidden && hiddenPath(relative))) return { rules, blocked: true };
    let info;
    try { info = await lstat(current); } catch { return { rules, blocked: true }; }
    if (info.isSymbolicLink()) return { rules, blocked: true };
    if (options.includeIgnored !== true && isIgnoredByRules(relative, info.isDirectory(), rules)) return { rules, blocked: true };
    if (options.includeIgnored !== true && info.isDirectory()) rules = await extendIgnoreRules(root, current, rules);
  }
  return { rules, blocked: false };
}

export async function walk(rootValue: string, startValue: string, options: WalkOptions = {}): Promise<WalkEntry[]> {
  const root = normalizeWorkspacePath(rootValue);
  const start = resolveInside(root, startValue);
  relativeInside(root, start);
  const maxDepth = Math.max(0, options.maxDepth ?? 20);
  const maxResults = Math.max(1, options.maxResults ?? 5_000);
  const maxVisited = Math.max(1_024, Math.min(500_000, maxResults * 8));
  const output: WalkEntry[] = [];
  let visited = 0;
  let stopped = false;
  let startInfo;
  try { startInfo = await lstat(start); } catch { return output; }
  const initial = await rulesForStart(root, start, options);
  if (initial.blocked) return output;

  const addEntry = (relative: string, type: WalkEntry['type'], info: { size: number; mtime: Date }): void => {
    output.push({ path: relative, type, size: info.size, modified: info.mtime.toISOString() });
    if (output.length >= maxResults) stopped = true;
  };

  if (!startInfo.isDirectory()) {
    const relative = relativeInside(root, start).replaceAll('\\', '/');
    if (!relative || fixedExcluded(path.basename(start), options) || (!options.includeHidden && hiddenPath(relative))) return output;
    if (options.includeIgnored !== true && isIgnoredByRules(relative, false, initial.rules)) return output;
    addEntry(relative, startInfo.isSymbolicLink() ? 'symlink' : 'file', startInfo);
    return output;
  }

  async function visit(directory: string, depth: number, rules: readonly IgnoreRule[]): Promise<void> {
    if (depth > maxDepth || stopped) return;
    let entries;
    try { entries = await readdir(directory, { withFileTypes: true }); } catch { return; }
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      if (stopped || visited >= maxVisited) { stopped = true; break; }
      visited += 1;
      if (fixedExcluded(entry.name, options)) continue;
      const full = path.join(directory, entry.name);
      const relative = relativeInside(root, full).replaceAll('\\', '/');
      if (prefixExcluded(relative, options)) continue;
      if (!options.includeHidden && hiddenPath(relative)) continue;
      let info;
      try { info = await lstat(full); } catch { continue; }
      const type: WalkEntry['type'] = info.isSymbolicLink() ? 'symlink' : info.isDirectory() ? 'directory' : 'file';
      if (options.includeIgnored !== true && isIgnoredByRules(relative, type === 'directory', rules)) continue;
      if (type === 'directory') {
        if (options.includeDirectories) addEntry(relative, 'directory', info);
        if (!stopped && depth + 1 < maxDepth) {
          const nestedRules = options.includeIgnored === true ? rules : await extendIgnoreRules(root, full, rules);
          await visit(full, depth + 1, nestedRules);
        }
      } else {
        addEntry(relative, type, info);
      }
    }
  }

  await visit(start, 0, initial.rules);
  return output;
}

export function globRegex(pattern: string): RegExp {
  const escaped = pattern
    .replace(/[.+^${}()|[\]\\]/g, '\\$&')
    .replaceAll('**', '§§')
    .replaceAll('*', '[^/]*')
    .replaceAll('§§', '.*')
    .replaceAll('?', '.');
  return new RegExp(`^${escaped}$`);
}

export function truncateUtf8(value: string, maxBytes: number): string {
  const data = Buffer.from(value);
  if (data.length <= maxBytes) return value;
  return data.subarray(data.length - maxBytes).toString('utf8');
}
