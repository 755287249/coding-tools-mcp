import { existsSync } from 'node:fs';
import path from 'node:path';
import type { JsonObject } from '../types.js';

function candidateStrength(root: string, relative: string): number {
  const full = path.resolve(root, relative);
  if (existsSync(full)) return 2;
  return existsSync(path.dirname(full)) ? 1 : 0;
}

function relativePath(base: string, value: string, root?: string): string {
  if (base === '.') return value;
  if (!value || value === '.') return base;
  if (
    path.posix.isAbsolute(value)
    || path.win32.isAbsolute(value)
    || value.startsWith('..')
  ) return value;
  const prefixed = `${base.replaceAll('\\', '/').replace(/\/$/, '')}/${value.replaceAll('\\', '/').replace(/^\.\//, '')}`;
  if (!root) return prefixed;
  const cwdStrength = candidateStrength(root, prefixed);
  const rootStrength = candidateStrength(root, value);
  return rootStrength > cwdStrength ? value : prefixed;
}

function prefixArrayPaths(args: JsonObject, field: string, keys: readonly string[], base: string, root?: string): void {
  const items = Array.isArray(args[field]) ? args[field] : undefined;
  if (!items) return;
  args[field] = items.map(item => {
    if (!item || typeof item !== 'object' || Array.isArray(item)) return item;
    const output = { ...(item as JsonObject) };
    for (const key of keys) {
      if (typeof output[key] === 'string') output[key] = relativePath(base, String(output[key]), root);
    }
    return output;
  });
}

function prefixStringArray(args: JsonObject, field: string, base: string, root?: string): void {
  const items = Array.isArray(args[field]) ? args[field] : undefined;
  if (!items) return;
  args[field] = items.map(item => typeof item === 'string' ? relativePath(base, item, root) : item);
}

function prefixHashKeys(args: JsonObject, field: string, base: string, root?: string): void {
  const value = args[field];
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  args[field] = Object.fromEntries(
    Object.entries(value as JsonObject).map(([key, hash]) => [relativePath(base, key, root), hash])
  );
}

function prefixPatchPaths(base: string, patch: string, root?: string): string {
  return patch.split(/\r?\n/).map(line => {
    for (const marker of ['--- a/', '+++ b/']) {
      if (line.startsWith(marker)) return `${marker}${relativePath(base, line.slice(marker.length), root)}`;
    }
    return line;
  }).join('\n');
}

export function applyDefaultCwdArgs(name: string, args: JsonObject, defaultCwd: string, workspaceRoot?: string): JsonObject {
  const base = defaultCwd.replaceAll('\\', '/') || '.';
  if (base === '.') return args;
  const effective: JsonObject = structuredClone(args);

  if (name === 'exec_command' && effective.workdir === undefined && effective.cwd === undefined) {
    effective.workdir = base;
    return effective;
  }

  if (['list_files', 'project_map', 'git_status', 'git_log'].includes(name)) {
    effective.path = relativePath(base, String(effective.path ?? '.'), workspaceRoot);
  } else if (['read_file', 'search_text', 'git_blame', 'view_image'].includes(name)) {
    if (typeof effective.path === 'string') effective.path = relativePath(base, effective.path, workspaceRoot);
  } else if (name === 'read_many') {
    prefixArrayPaths(effective, 'items', ['path'], base, workspaceRoot);
  } else if (name === 'git_diff' || name === 'git_show') {
    // Git read targets use the same coordinate contract as Git mutators:
    // repo_path is workspace-relative and pathspecs are repo-relative.
    if (effective.repo_path === undefined) effective.repo_path = base;
  } else if (name === 'format_files') {
    prefixStringArray(effective, 'paths', base, workspaceRoot);
    if (!Array.isArray(effective.paths) && ['changed', 'staged', 'project'].includes(String(effective.scope ?? ''))) {
      effective.paths = [base];
    }
    prefixHashKeys(effective, 'expected_sha256', base, workspaceRoot);
  } else if (name === 'apply_patch' || name === 'patch_check') {
    if (typeof effective.patch === 'string') effective.patch = prefixPatchPaths(base, effective.patch, workspaceRoot);
    prefixHashKeys(effective, 'expected_sha256', base, workspaceRoot);
  } else if (name === 'edit') {
    prefixArrayPaths(effective, 'files', ['path'], base, workspaceRoot);
  } else if (name === 'file_ops') {
    prefixArrayPaths(effective, 'operations', ['path', 'destination'], base, workspaceRoot);
  } else if (['git_branch', 'git_stage', 'git_commit', 'git_push', 'git_restore', 'git_worktree'].includes(name)) {
    // Git mutator repo_path is workspace-relative; pathspecs are repo-relative.
    // Only an omitted repo_path inherits the conversation default cwd.
    if (effective.repo_path === undefined) effective.repo_path = base;
  }

  return effective;
}
