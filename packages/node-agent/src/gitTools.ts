import type { JsonObject, ToolContext } from './types.js';
import { gitRunRouting, runGitBuffered } from './gitProcess.js';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { mkdir, rm, stat } from 'node:fs/promises';
import {
  exists,
  relativeInside,
  resolveInside,
  resolveExistingPath,
  rejectProtectedWritePath,
  rootAndCwd,
  validateWorkspaceUserPath
} from './workspace.js';

const ok = (value: JsonObject): JsonObject => ({ ok: true, ...value });
const fail = (code: string, message: string): JsonObject => ({ ok: false, error: { code, message, category: 'git', retryable: false } });
const MAX_GIT_RESTORE_SNAPSHOT_BYTES = 16 * 1024 * 1024;

type GitResult = { code: number | null; stdout: string; stderr: string };
type GitTarget = {
  repoPath: string;
  root: string;
  gitDir: string;
  commonDir: string;
  branch: string;
  head: string;
  fingerprint: string;
};

export { gitRunRouting };

async function runGitAt(cwd: string, args: string[], timeoutMs = 30_000, input?: string): Promise<GitResult> {
  return runGitBuffered(cwd, args, input, timeoutMs);
}

async function runGit(ctx: ToolContext, key: string, args: string[], timeoutMs = 30_000, input?: string): Promise<GitResult> {
  return runGitAt(rootAndCwd(ctx, key).cwd, args, timeoutMs, input);
}

async function stagedPathsAt(root: string): Promise<string[]> {
  const result = await runGitAt(root, ['diff', '--cached', '--name-only', '-z', '--'], 10_000);
  if (result.code !== 0) throw new Error(result.stderr || result.stdout || 'git staged path query failed');
  return result.stdout.split('\0').filter(Boolean);
}

async function restoreCleanIndexAfterGitTransactionFailure(
  root: string,
  stagedByTool: boolean,
  indexCleanBefore: boolean
): Promise<boolean> {
  if (!stagedByTool || !indexCleanBefore) return false;
  const reset = await runGitAt(root, ['reset', '--quiet', 'HEAD', '--'], 10_000);
  return reset.code === 0;
}

function gitTransactionHeadChanged(details: {
  expectedHead?: string;
  actualHeadBefore: string;
  actualHeadAtCommit: string;
  selectedPathCount: number;
  stagedPathCountBefore: number;
  stagedPathCount: number;
  indexCleanBefore: boolean;
  stagedByTool: boolean;
  indexRestored: boolean;
}): JsonObject {
  return {
    ok: false,
    error: {
      code: 'GIT_HEAD_CHANGED_DURING_TRANSACTION',
      message: 'Git HEAD changed after transaction preflight and before commit',
      category: 'conflict',
      retryable: true,
      details: {
        transaction_stage: 'pre_commit_head_check',
        expected_head: details.expectedHead ?? null,
        actual_head_before: details.actualHeadBefore,
        actual_head_at_commit: details.actualHeadAtCommit,
        selected_path_count: details.selectedPathCount,
        staged_path_count_before: details.stagedPathCountBefore,
        staged_path_count: details.stagedPathCount,
        index_clean_before: details.indexCleanBefore,
        staged_by_tool: details.stagedByTool,
        index_restored: details.indexRestored,
        suggestion: 'Refresh git_status/git_diff and retry git_commit with the new expected_head.'
      }
    }
  };
}

function targetMetadata(target: GitTarget): JsonObject {
  return {
    repo_path: target.repoPath,
    repo_root: target.root,
    git_dir: target.gitDir,
    git_common_dir: target.commonDir,
    branch: target.branch,
    head: target.head,
    repo_fingerprint: target.fingerprint
  };
}

async function resolveGitTarget(ctx: ToolContext, key: string, args: JsonObject): Promise<GitTarget> {
  const { root: workspaceRoot } = rootAndCwd(ctx, key);
  const requestedRepoPath = String(args.repo_path ?? '.');
  const resolved = await resolveExistingPath(workspaceRoot, requestedRepoPath);
  const info = await stat(resolved.full);
  if (!info.isDirectory()) throw new Error('repo_path must be a directory');
  const [metadata, branchResult] = await Promise.all([
    runGitAt(resolved.full, ['rev-parse', '--show-toplevel', '--absolute-git-dir', '--git-common-dir', 'HEAD'], 5_000),
    runGitAt(resolved.full, ['symbolic-ref', '--quiet', '--short', 'HEAD'], 5_000)
  ]);
  if (metadata.code !== 0) {
    const detail = (metadata.stderr || metadata.stdout || 'not a git repository').trim();
    throw new Error(`NOT_GIT_REPOSITORY: ${detail}`);
  }
  const values = metadata.stdout.split(/\r?\n/).filter(Boolean);
  if (values.length < 4) throw new Error('GIT_REPOSITORY_METADATA_INCOMPLETE');
  const [root, gitDir, commonDir, head] = values;
  const branch = branchResult.code === 0 && branchResult.stdout.trim() ? branchResult.stdout.trim() : 'HEAD';
  const repoPath = relativeInside(workspaceRoot, root).replaceAll('\\', '/') || '.';
  const fingerprint = createHash('sha256')
    .update([root, gitDir, commonDir || gitDir, branch || 'HEAD', head || 'missing'].join('\0'))
    .digest('hex');
  return { repoPath, root, gitDir, commonDir: commonDir || gitDir, branch, head, fingerprint };
}

function repoTargetMismatch(target: GitTarget, expected: unknown): JsonObject | null {
  if (!expected || String(expected).toLowerCase() === target.fingerprint.toLowerCase()) return null;
  return {
    ok: false,
    error: {
      code: 'GIT_REPO_TARGET_MISMATCH',
      message: 'Git repository/worktree target changed since preflight',
      category: 'conflict',
      retryable: true,
      details: {
        expected_repo_fingerprint: String(expected),
        actual_repo_fingerprint: target.fingerprint,
        repo: targetMetadata(target),
        suggestion: 'Call git_status with path=repo_path and retry with the returned repo_fingerprint'
      }
    }
  };
}

function boundedInteger(value: unknown, fallback: number, minimum: number, maximum: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(minimum, Math.min(maximum, Math.trunc(parsed))) : fallback;
}

async function resolveExistingGitPath(ctx: ToolContext, key: string, value: unknown): Promise<{
  root: string;
  full: string;
  display: string;
  isDirectory: boolean;
}> {
  const { root, cwd } = rootAndCwd(ctx, key);
  const requested = String(value ?? '.');
  const resolved = requested === '.'
    ? await resolveExistingPath(root, relativeInside(root, cwd))
    : await resolveExistingPath(root, requested);
  const info = await stat(resolved.full);
  return {
    root: resolved.root,
    full: resolved.full,
    display: resolved.display,
    isDirectory: info.isDirectory()
  };
}

function canonicalRepoRelativePath(target: GitTarget | undefined, value: string): string {
  const normalized = value.replaceAll('\\', '/').replace(/^\.\//, '');
  if (!target || target.repoPath === '.') return normalized;
  const prefix = `${target.repoPath.replace(/[\\/]+$/, '')}/`;
  return normalized.startsWith(prefix) ? normalized.slice(prefix.length) : normalized;
}

function gitPaths(ctx: ToolContext, key: string, args: JsonObject, protectedWrites = false, target?: GitTarget): string[] {
  rootAndCwd(ctx, key);
  const values = Array.isArray(args.paths) ? args.paths.map(String) : args.path ? [String(args.path)] : [];
  return values.map(value => {
    validateWorkspaceUserPath(value, { allowDot: false });
    const canonical = canonicalRepoRelativePath(target, value);
    validateWorkspaceUserPath(canonical, { allowDot: false });
    if (protectedWrites) {
      const workspacePath = target && target.repoPath !== '.' ? `${target.repoPath.replace(/[\\/]+$/, '')}/${canonical}` : canonical;
      rejectProtectedWritePath(workspacePath);
    }
    return canonical;
  });
}

function validateGitRef(value: unknown, fallback = 'HEAD'): string {
  const ref = String(value ?? fallback);
  if (!ref || ref.startsWith('-') || /[\0\r\n]/.test(ref)) throw new Error('INVALID_ARGUMENT');
  return ref;
}

async function isGitRepoAt(cwd: string): Promise<boolean> {
  const result = await runGitAt(cwd, ['rev-parse', '--git-dir'], 5_000);
  return result.code === 0;
}

function parseBranchLine(line: string): { branch: string; upstream: string; ahead: number; behind: number } {
  const separator = line.indexOf('...');
  const branchPart = separator >= 0 ? line.slice(0, separator) : line;
  const tracking = separator >= 0 ? line.slice(separator + 3) : '';
  const branch = branchPart.includes(' ') ? branchPart.slice(0, branchPart.indexOf(' ')) : branchPart;
  let upstream = tracking;
  let ahead = 0;
  let behind = 0;
  const metadataAt = tracking.indexOf(' ');
  if (metadataAt >= 0) {
    upstream = tracking.slice(0, metadataAt);
    const metadata = tracking.slice(metadataAt + 1).replace(/^\[/, '').replace(/\]$/, '');
    for (const item of metadata.split(',')) {
      const token = item.trim();
      if (token.startsWith('ahead ')) ahead = Number.parseInt(token.slice(6).trim(), 10) || 0;
      else if (token.startsWith('behind ')) behind = Number.parseInt(token.slice(7).trim(), 10) || 0;
    }
  }
  return { branch, upstream, ahead, behind };
}

interface GitWorktreeEntry {
  path: string;
  absolutePath: string;
  head: string;
  branch: string;
  detached: boolean;
  bare: boolean;
  locked?: string | boolean;
  prunable?: string | boolean;
  primary?: boolean;
  inside_workspace?: boolean;
  managed?: boolean;
  removable?: boolean;
  dirty?: boolean | null;
  branch_merged?: boolean | null;
}

function pathInside(parent: string, candidate: string): boolean {
  const parentPath = normalizedLocalPath(parent);
  const candidatePath = normalizedLocalPath(candidate);
  return candidatePath === parentPath || candidatePath.startsWith(`${parentPath}${path.sep}`);
}

function displayWorktreePath(workspaceRoot: string, value: string): string {
  try {
    return relativeInside(workspaceRoot, value).replaceAll('\\', '/') || '.';
  } catch {
    return value.replaceAll('\\', '/');
  }
}

function parseWorktreePorcelain(workspaceRoot: string, output: string): GitWorktreeEntry[] {
  const entries: GitWorktreeEntry[] = [];
  let current: GitWorktreeEntry | undefined;
  const flush = (): void => {
    if (!current) return;
    entries.push(current);
    current = undefined;
  };
  for (const raw of output.split(/\r?\n/)) {
    if (!raw) {
      flush();
      continue;
    }
    const separator = raw.indexOf(' ');
    const field = separator >= 0 ? raw.slice(0, separator) : raw;
    const value = separator >= 0 ? raw.slice(separator + 1) : '';
    if (field === 'worktree') {
      flush();
      current = {
        path: displayWorktreePath(workspaceRoot, value),
        absolutePath: value,
        head: '',
        branch: '',
        detached: false,
        bare: false
      };
      continue;
    }
    if (!current) continue;
    if (field === 'HEAD') current.head = value;
    else if (field === 'branch') current.branch = value.replace(/^refs\/heads\//, '');
    else if (field === 'detached') current.detached = true;
    else if (field === 'bare') current.bare = true;
    else if (field === 'locked') current.locked = value || true;
    else if (field === 'prunable') current.prunable = value || true;
  }
  flush();
  return entries;
}

async function listGitWorktrees(root: string, workspaceRoot: string): Promise<GitWorktreeEntry[]> {
  const result = await runGitAt(root, ['worktree', 'list', '--porcelain'], 10_000);
  if (result.code !== 0) throw new Error(result.stderr || result.stdout || 'git worktree list failed');
  const entries = parseWorktreePorcelain(workspaceRoot, result.stdout);
  const mergedResult = await runGitAt(root, ['branch', '--merged', 'HEAD', '--format=%(refname:short)'], 10_000);
  const mergedBranches = new Set(mergedResult.code === 0 ? mergedResult.stdout.split(/\r?\n/).map(value => value.trim()).filter(Boolean) : []);
  for (const entry of entries) {
    const insideWorkspace = pathInside(workspaceRoot, entry.absolutePath);
    const primary = normalizedLocalPath(entry.absolutePath) === normalizedLocalPath(workspaceRoot);
    entry.primary = primary;
    entry.inside_workspace = insideWorkspace;
    entry.managed = insideWorkspace && !primary && entry.path.startsWith('.worktrees/');
    entry.removable = insideWorkspace && !primary;
    entry.branch_merged = entry.branch ? mergedBranches.has(entry.branch) : null;
    if (insideWorkspace && !entry.bare && await exists(entry.absolutePath)) {
      const status = await runGitAt(entry.absolutePath, ['status', '--porcelain=v1', '--untracked-files=normal'], 10_000);
      entry.dirty = status.code === 0 ? Boolean(status.stdout.trim()) : null;
    } else {
      entry.dirty = null;
    }
  }
  return entries;
}

function worktreeBranchSlug(branch: string): string {
  return branch.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '') || 'worktree';
}

async function localBranchExists(root: string, branch: string): Promise<boolean> {
  const result = await runGitAt(root, ['show-ref', '--verify', '--quiet', `refs/heads/${branch}`], 10_000);
  return result.code === 0;
}

function normalizedLocalPath(value: string): string {
  const resolved = path.resolve(value);
  return process.platform === 'win32' ? resolved.toLowerCase() : resolved;
}

function cwdInsideRemovedPath(ctx: ToolContext, key: string, workspaceRoot: string, removedPath: string): boolean {
  const { cwd } = rootAndCwd(ctx, key);
  const current = normalizedLocalPath(cwd);
  const removed = normalizedLocalPath(removedPath);
  return current === removed || current.startsWith(`${removed}${path.sep}`);
}

function parseDiffFiles(diff: string): JsonObject[] {
  const files: JsonObject[] = [];
  for (const line of diff.split(/\r?\n/)) {
    if (line.startsWith('+++ b/')) {
      files.push({ path: line.slice(6), status: 'modified', binary: false });
    } else if (line.startsWith('--- /dev/null')) {
      continue;
    } else if (line.startsWith('--- a/')) {
      const filePath = line.slice(6);
      if (!files.some(file => file.path === filePath)) files.push({ path: filePath, status: 'modified', binary: false });
    }
  }
  return files;
}

function parseGitBlamePorcelain(output: string): JsonObject[] {
  const rows: JsonObject[] = [];
  let current: JsonObject = {};
  for (const raw of output.split(/\r?\n/)) {
    const parts = raw.trim().split(/\s+/);
    if (parts.length >= 3 && /^[0-9a-fA-F^]{40}/.test(parts[0])) {
      current = { commit: parts[0].replace(/^\^/, '') };
      if (/^\d+$/.test(parts[1])) current.original_line = Number(parts[1]);
      if (/^\d+$/.test(parts[2])) current.line = Number(parts[2]);
      continue;
    }
    if (raw.startsWith('author ')) current.author = raw.slice(7);
    else if (raw.startsWith('author-mail ')) current.author_mail = raw.slice(12).replace(/^</, '').replace(/>$/, '');
    else if (raw.startsWith('author-time ')) {
      const value = raw.slice(12);
      current.author_time = /^\d+$/.test(value) ? Number(value) : value;
    } else if (raw.startsWith('summary ')) current.summary = raw.slice(8);
    else if (raw.startsWith('\t')) rows.push({ ...current, content: raw.slice(1) });
  }
  return rows;
}

function gitReadFailure(result: GitResult): JsonObject {
  return fail('GIT_ERROR', (result.stderr || result.stdout || 'Git command failed').trim());
}

async function runGitDiff(root: string, context: number, paths: string[], cached: boolean): Promise<GitResult> {
  const argv = ['diff', `--unified=${context}`];
  if (cached) argv.push('--cached');
  if (paths.length) argv.push('--', ...paths);
  return runGitAt(root, argv, 60_000);
}

async function gitIndexClean(ctx: ToolContext, key: string): Promise<boolean> {
  const result = await runGit(ctx, key, ['diff', '--cached', '--quiet', '--exit-code']);
  if (result.code === 0) return true;
  if (result.code === 1) return false;
  throw new Error(result.stderr || 'GIT_INDEX_CHECK_FAILED');
}

async function validateBranchName(ctx: ToolContext, key: string, name: string): Promise<void> {
  const result = await runGit(ctx, key, ['check-ref-format', '--branch', name]);
  if (result.code !== 0) throw new Error('INVALID_GIT_BRANCH_NAME');
}

async function ensureExpectedHead(target: GitTarget, expected: unknown): Promise<void> {
  if (!expected) return;
  if (target.head !== String(expected).trim()) throw new Error('EXPECTED_HEAD_MISMATCH');
}

export async function gitStatusTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  let target: GitTarget;
  try {
    target = await resolveGitTarget(ctx, key, { repo_path: args.path ?? '.' });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.startsWith('NOT_GIT_REPOSITORY:')) throw error;
    return ok({
      is_repo: false,
      clean: true,
      entries: [],
      warnings: [message.slice('NOT_GIT_REPOSITORY:'.length).trim() || 'not a git repository']
    });
  }
  const argv = ['status', '--porcelain=v1', '-b'];
  if (args.include_untracked === false) argv.push('--untracked-files=no');
  const result = await runGitAt(target.root, argv, 10_000);
  if (result.code !== 0) return gitReadFailure(result);
  const lines = result.stdout.split(/\r?\n/).filter(Boolean);
  const totalLines = lines.length;
  const maxEntries = boundedInteger(args.max_entries, 1_000, 1, 10_000);
  let branch = '';
  let upstream = '';
  let ahead = 0;
  let behind = 0;
  const entries: JsonObject[] = [];
  for (const line of lines) {
    if (line.startsWith('## ')) {
      ({ branch, upstream, ahead, behind } = parseBranchLine(line.slice(3)));
      continue;
    }
    if (line.length < 4) continue;
    const pathText = line.slice(3);
    const renameAt = pathText.indexOf(' -> ');
    const entry: JsonObject = {
      path: renameAt >= 0 ? pathText.slice(renameAt + 4) : pathText,
      index_status: line[0] ?? ' ',
      worktree_status: line[1] ?? ' '
    };
    if (renameAt >= 0) entry.original_path = pathText.slice(0, renameAt);
    entries.push(entry);
    if (entries.length >= maxEntries) break;
  }
  return ok({
    is_repo: true,
    branch,
    head: target.head === 'missing' ? '' : target.head,
    upstream,
    ahead,
    behind,
    clean: entries.length === 0,
    repo: targetMetadata(target),
    repo_fingerprint: target.fingerprint,
    entries,
    truncated: entries.length >= maxEntries && totalLines > maxEntries + 1,
    warnings: []
  });
}

export async function gitDiffTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  // Validate caller-supplied pathspecs before repository probing so workspace
  // security errors keep precedence even when the selected directory is not a Git repo.
  gitPaths(ctx, key, args, false);
  let target: GitTarget;
  try {
    target = await resolveGitTarget(ctx, key, args);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!message.startsWith('NOT_GIT_REPOSITORY:')) throw error;
    return ok({ diff: '', files: [], truncated: false, warnings: ['not a git repository'] });
  }
  const root = target.root;
  const requestedContext = boundedInteger(args.context_lines, 3, 0, 1_000);
  const context = Math.min(requestedContext, 20);
  const maxBytes = boundedInteger(args.max_bytes, 262_144, 1_024, 1_048_576);
  const staged = args.staged === true;
  const unstaged = args.unstaged !== false;
  const paths = gitPaths(ctx, key, args, false, target);
  const chunks: string[] = [];
  if (unstaged) {
    const result = await runGitDiff(root, context, paths, false);
    if (result.code !== 0 && result.code !== 1) return gitReadFailure(result);
    chunks.push(result.stdout);
  }
  if (staged) {
    const result = await runGitDiff(root, context, paths, true);
    if (result.code !== 0 && result.code !== 1) return gitReadFailure(result);
    chunks.push(result.stdout);
  }
  let combined = chunks.join('\n');
  if (combined && !combined.endsWith('\n')) combined += '\n';
  const data = Buffer.from(combined);
  const truncated = data.length > maxBytes;
  const diff = truncated ? data.subarray(0, maxBytes).toString('utf8') : combined;
  const normalized = requestedContext !== context;
  return ok({
    diff,
    files: parseDiffFiles(diff),
    repo: targetMetadata(target),
    repo_fingerprint: target.fingerprint,
    paths,
    arguments_normalized: normalized,
    normalized_arguments: normalized ? { context_lines: context } : null,
    truncated,
    warnings: truncated ? ['diff truncated'] : [],
    bytes: data.length
  });
}

export async function gitLogTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const requestedPath = String(args.path ?? '.');
  const resolved = await resolveExistingGitPath(ctx, key, requestedPath);
  const ref = validateGitRef(args.ref, 'HEAD');
  const maxCount = boundedInteger(args.max_count, 20, 1, 100);
  const skip = boundedInteger(args.skip, 0, 0, 10_000);
  const gitCwd = resolved.isDirectory ? resolved.full : path.dirname(resolved.full);
  if (!(await isGitRepoAt(gitCwd))) {
    return ok({ is_repo: false, commits: [], truncated: false, warnings: [] });
  }
  const argv = [
    'log', `--max-count=${maxCount + 1}`, `--skip=${skip}`, '--date=iso-strict',
    '--pretty=format:%H%x1f%h%x1f%an%x1f%ae%x1f%ad%x1f%s%x1e', ref
  ];
  if (requestedPath !== '.') argv.push('--', resolved.isDirectory ? '.' : path.basename(resolved.full));
  const result = await runGitAt(gitCwd, argv, 10_000);
  if (result.code !== 0) return gitReadFailure(result);
  const commits = result.stdout.split('\x1e').flatMap(record => {
    const fields = record.trim().split('\x1f').map(value => value.trim());
    if (fields.length < 6 || !fields[0]) return [];
    const [hash, short_hash, author_name, author_email, author_date, subject] = fields;
    return [{ hash, short_hash, author_name, author_email, author_date, authored_at: author_date, subject }];
  });
  const truncated = commits.length > maxCount;
  const page = commits.slice(0, maxCount);
  return ok({
    is_repo: true,
    ref,
    path: resolved.display,
    commits: page,
    truncated,
    warnings: truncated ? ['commit limit reached'] : [],
    count: page.length,
    skip,
    max_count: maxCount
  });
}

export async function gitShowTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  let target: GitTarget;
  try {
    target = await resolveGitTarget(ctx, key, args);
  } catch {
    return ok({ is_repo: false, content: '', files: [], truncated: false, warnings: [] });
  }
  const root = target.root;
  const rev = validateGitRef(args.rev, 'HEAD');
  const requestedContext = boundedInteger(args.context_lines, 3, 0, 1_000);
  const context = Math.min(requestedContext, 20);
  const maxBytes = boundedInteger(args.max_bytes, 262_144, 1, 1_048_576);
  const includeDiff = args.include_diff !== false;
  const paths = gitPaths(ctx, key, args, false, target);
  const argv = ['show', '--no-ext-diff', '--format=fuller', `--unified=${context}`];
  if (!includeDiff) argv.push('--no-patch');
  argv.push(rev);
  if (paths.length) argv.push('--', ...paths);
  const result = await runGitAt(root, argv, 60_000);
  if (result.code !== 0) return gitReadFailure(result);
  const data = Buffer.from(result.stdout);
  const truncated = data.length > maxBytes;
  const content = truncated ? data.subarray(0, maxBytes).toString('utf8') : result.stdout;
  const normalized = requestedContext !== context;
  return ok({
    is_repo: true,
    rev,
    content,
    output: content,
    files: parseDiffFiles(content),
    repo: targetMetadata(target),
    repo_fingerprint: target.fingerprint,
    paths,
    arguments_normalized: normalized,
    normalized_arguments: normalized ? { context_lines: context } : null,
    truncated,
    output_bytes: Buffer.byteLength(content),
    bytes: data.length,
    warnings: truncated ? ['output truncated'] : []
  });
}

export async function gitBlameTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const requestedPath = String(args.path ?? '').trim();
  if (!requestedPath) throw new Error('path is required');
  const resolved = await resolveExistingGitPath(ctx, key, requestedPath);
  if (resolved.isDirectory) throw new Error('IS_DIRECTORY');
  const gitCwd = path.dirname(resolved.full);
  if (!(await isGitRepoAt(gitCwd))) {
    return ok({ is_repo: false, path: resolved.display, lines: [], truncated: false, warnings: [] });
  }
  const rev = args.rev === undefined ? null : validateGitRef(args.rev);
  const maxLines = boundedInteger(args.max_lines, 200, 1, 1_000);
  const maximumStartLine = Number.MAX_SAFE_INTEGER - maxLines + 1;
  const startLine = boundedInteger(args.start_line, 1, 1, maximumStartLine);
  let finalLine = args.end_line === undefined
    ? startLine + maxLines - 1
    : boundedInteger(args.end_line, startLine, 1, Number.MAX_SAFE_INTEGER);
  if (finalLine < startLine) throw new Error('INVALID_ARGUMENT');
  const requestedLines = finalLine - startLine + 1;
  let truncated = requestedLines > maxLines;
  finalLine = Math.min(finalLine, startLine + maxLines - 1);
  const argv = ['blame', '--line-porcelain', '-L', `${startLine},${finalLine}`];
  if (rev) argv.push(rev);
  argv.push('--', path.basename(resolved.full));
  const result = await runGitAt(gitCwd, argv, 60_000);
  if (result.code !== 0) return gitReadFailure(result);
  let lines = parseGitBlamePorcelain(result.stdout);
  if (lines.length > maxLines) {
    lines = lines.slice(0, maxLines);
    truncated = true;
  }
  return ok({
    is_repo: true,
    path: resolved.display,
    rev,
    start_line: startLine,
    end_line: finalLine,
    lines,
    truncated,
    warnings: truncated ? ['line limit reached'] : []
  });
}

export async function gitBranchTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const target = await resolveGitTarget(ctx, key, args);
  const mismatch = repoTargetMismatch(target, args.expected_repo_fingerprint);
  if (mismatch) return mismatch;
  await ensureExpectedHead(target, args.expected_head);
  const action = String(args.action ?? '');
  const name = String(args.name ?? '');
  if (!name) throw new Error('name is required');
  await validateBranchName(ctx, key, name);
  let argv: string[];
  if (action === 'create') {
    const start = String(args.start_point ?? 'HEAD');
    argv = args.switch === false ? ['branch', name, start] : ['switch', '-c', name, start];
  }
  else if (action === 'switch') argv = ['switch', name];
  else if (action === 'delete') {
    if (ctx.config.securityPolicy.requireWriteConfirmation && args.confirm !== true) return fail('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION', 'Deleting a branch requires confirm=true');
    argv = ['branch', args.force === true ? '-D' : '-d', name];
  } else throw new Error('invalid git branch action');
  const command = ['git', ...argv];
  if (args.dry_run === true) return ok({ dry_run: true, applied: false, action, name, command, repo: targetMetadata(target), warnings: [] });
  const result = await runGitAt(target.root, argv, 60_000);
  if (result.code !== 0) return fail('GIT_BRANCH_FAILED', result.stderr || result.stdout);
  return ok({
    dry_run: false,
    applied: true,
    action,
    name,
    command,
    stdout: result.stdout.trim(),
    repo: targetMetadata(target),
    status: await gitStatusTool(ctx, key, { path: target.repoPath }),
    affected_files: [],
    warnings: []
  });
}

export async function gitWorktreeTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const action = String(args.action ?? '').trim();
  if (!['list', 'create', 'remove'].includes(action)) return fail('INVALID_ARGUMENT', 'action must be list, create, or remove');
  const target = await resolveGitTarget(ctx, key, args);
  const mismatch = repoTargetMismatch(target, args.expected_repo_fingerprint);
  if (mismatch) return mismatch;
  await ensureExpectedHead(target, args.expected_head);
  const { folder, root: workspaceRoot } = rootAndCwd(ctx, key);

  if (action === 'list') {
    return ok({
      action,
      applied: false,
      repo: targetMetadata(target),
      worktrees: await listGitWorktrees(target.root, workspaceRoot)
    });
  }

  if (action === 'create') {
    const branch = String(args.branch ?? '').trim();
    if (!branch) return fail('INVALID_ARGUMENT', 'branch is required for create');
    await validateBranchName(ctx, key, branch);
    const requestedPath = String(args.path ?? `.worktrees/${worktreeBranchSlug(branch)}`).trim();
    validateWorkspaceUserPath(requestedPath, { allowDot: false });
    const fullPath = resolveInside(workspaceRoot, requestedPath);
    const displayPath = relativeInside(workspaceRoot, fullPath).replaceAll('\\', '/');
    if (await exists(fullPath)) return fail('GIT_WORKTREE_PATH_EXISTS', `Worktree path already exists: ${displayPath}`);
    const startPoint = validateGitRef(args.start_point, 'HEAD');
    const configuredMode = String(args.branch_mode ?? 'auto');
    if (!['auto', 'create', 'existing'].includes(configuredMode)) return fail('INVALID_ARGUMENT', 'branch_mode must be auto, create, or existing');
    const branchExists = await localBranchExists(target.root, branch);
    const branchMode = configuredMode === 'auto' ? (branchExists ? 'existing' : 'create') : configuredMode;
    if (branchMode === 'create' && branchExists) return fail('GIT_WORKTREE_BRANCH_EXISTS', `Branch already exists: ${branch}`);
    if (branchMode === 'existing' && !branchExists) return fail('GIT_WORKTREE_BRANCH_NOT_FOUND', `Branch does not exist: ${branch}`);
    const gitArgs = branchMode === 'existing'
      ? ['worktree', 'add', fullPath, branch]
      : ['worktree', 'add', '-b', branch, fullPath, startPoint];
    if (args.dry_run === true) {
      return ok({ action, dry_run: true, applied: false, path: displayPath, branch, branch_mode: branchMode, start_point: startPoint, command: ['git', ...gitArgs], repo: targetMetadata(target) });
    }
    await mkdir(path.dirname(fullPath), { recursive: true });
    const result = await runGitAt(target.root, gitArgs, 60_000);
    if (result.code !== 0) return gitReadFailure(result);
    return ok({
      action,
      applied: true,
      path: displayPath,
      branch,
      branch_mode: branchMode,
      start_point: startPoint,
      repo: targetMetadata(target),
      worktrees: await listGitWorktrees(target.root, workspaceRoot)
    });
  }

  const requestedPath = String(args.path ?? '').trim();
  if (!requestedPath) return fail('INVALID_ARGUMENT', 'path is required for remove');
  validateWorkspaceUserPath(requestedPath, { allowDot: false });
  const fullPath = resolveInside(workspaceRoot, requestedPath);
  const displayPath = relativeInside(workspaceRoot, fullPath).replaceAll('\\', '/');
  if (normalizedLocalPath(fullPath) === normalizedLocalPath(workspaceRoot)) {
    return fail('GIT_WORKTREE_PRIMARY_REMOVE_DENIED', 'The configured workspace root cannot be removed as a linked worktree');
  }
  const worktrees = await listGitWorktrees(target.root, workspaceRoot);
  const entry = worktrees.find(item => normalizedLocalPath(item.absolutePath) === normalizedLocalPath(fullPath));
  if (!entry) return fail('GIT_WORKTREE_NOT_FOUND', `Linked worktree is not registered: ${displayPath}`);
  const survivingGitRoot = worktrees.find(item =>
    !item.bare && normalizedLocalPath(item.absolutePath) !== normalizedLocalPath(fullPath)
  )?.absolutePath ?? workspaceRoot;
  if (entry.locked && args.force !== true) return fail('GIT_WORKTREE_LOCKED', `Linked worktree is locked: ${displayPath}`);
  if (args.force !== true) {
    const status = await runGitAt(fullPath, ['status', '--porcelain=v1', '--untracked-files=normal'], 10_000);
    if (status.code !== 0) return gitReadFailure(status);
    if (status.stdout.trim()) return fail('GIT_WORKTREE_DIRTY', `Linked worktree has uncommitted changes: ${displayPath}`);
  }
  if (args.dry_run === true) {
    return ok({ action, dry_run: true, applied: false, path: displayPath, branch: entry.branch || null, repo: targetMetadata(target) });
  }
  if (args.confirm !== true) {
    return fail('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION', 'Removing a linked worktree requires confirm=true');
  }
  const removeArgs = ['worktree', 'remove'];
  if (args.force === true) removeArgs.push('--force');
  removeArgs.push(fullPath);
  const removed = await runGitAt(target.root, removeArgs, 60_000);
  const warnings: string[] = [];
  if (removed.code !== 0) {
    const remaining = await listGitWorktrees(await exists(target.root) ? target.root : survivingGitRoot, workspaceRoot);
    const stillRegistered = remaining.some(item => normalizedLocalPath(item.absolutePath) === normalizedLocalPath(fullPath));
    if (stillRegistered) return gitReadFailure(removed);
    warnings.push(`worktree registration removed but Git reported cleanup failure: ${(removed.stderr || removed.stdout).trim()}`);
  }
  if (await exists(fullPath)) {
    try {
      await rm(fullPath, { recursive: true, force: true, maxRetries: 3, retryDelay: 50 });
    } catch (error) {
      warnings.push(`worktree directory cleanup incomplete: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  let branchDeleted = false;
  if (args.delete_branch === true && entry.branch) {
    const deleted = await runGitAt(survivingGitRoot, ['branch', args.force === true ? '-D' : '-d', entry.branch], 30_000);
    if (deleted.code === 0) {
      branchDeleted = true;
    } else {
      warnings.push(`worktree removed but branch deletion failed: ${(deleted.stderr || deleted.stdout).trim()}`);
    }
  }
  if (cwdInsideRemovedPath(ctx, key, workspaceRoot, fullPath)) {
    ctx.conversations.setFolderCwd(key, folder.id, '.');
  }
  return ok({
    action,
    applied: true,
    path: displayPath,
    branch: entry.branch || null,
    branch_deleted: branchDeleted,
    default_cwd: ctx.conversations.peekCwdFor(key, folder.id),
    repo: targetMetadata(target),
    worktrees: await listGitWorktrees(survivingGitRoot, workspaceRoot),
    warnings
  });
}

export async function gitStageTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const preflightPaths = gitPaths(ctx, key, args, false);
  if (String(args.repo_path ?? '.') === '.') {
    for (const file of preflightPaths) rejectProtectedWritePath(file);
  }
  const target = await resolveGitTarget(ctx, key, args);
  const mismatch = repoTargetMismatch(target, args.expected_repo_fingerprint);
  if (mismatch) return mismatch;
  await ensureExpectedHead(target, args.expected_head);
  const paths = gitPaths(ctx, key, args, true, target);
  if (args.all !== true && !paths.length) throw new Error('paths or all=true is required');
  const argv = args.all === true ? ['add', '-A'] : ['add', '--', ...paths];
  if (args.dry_run === true) return ok({ dry_run: true, applied: false, paths, all: args.all === true, command: ['git', ...argv], repo: targetMetadata(target), warnings: [] });
  const result = await runGitAt(target.root, argv);
  if (result.code !== 0) return fail('GIT_STAGE_FAILED', result.stderr || result.stdout);
  return ok({
    dry_run: false,
    applied: true,
    paths,
    all: args.all === true,
    repo: targetMetadata(target),
    status: await gitStatusTool(ctx, key, { path: target.repoPath }),
    affected_files: paths.map(file => ({ path: file, operation: 'stage' })),
    warnings: []
  });
}

export async function gitCommitTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const target = await resolveGitTarget(ctx, key, args);
  const mismatch = repoTargetMismatch(target, args.expected_repo_fingerprint);
  if (mismatch) return mismatch;
  await ensureExpectedHead(target, args.expected_head);
  const message = String(args.message ?? '');
  if (!message.trim() || message.length > 10_000) throw new Error('message must be between 1 and 10000 characters');
  const paths = gitPaths(ctx, key, args, true, target);
  const all = args.all === true;
  const stagedByTool = paths.length > 0 || all;
  const requireCleanIndex = args.require_clean_index_before === undefined ? stagedByTool : args.require_clean_index_before === true;
  const expectedHead = typeof args.expected_head === 'string' ? args.expected_head : undefined;
  const actualHeadBeforeResult = await runGitAt(target.root, ['rev-parse', 'HEAD'], 10_000);
  const actualHeadBefore = actualHeadBeforeResult.code === 0 ? actualHeadBeforeResult.stdout.trim() : target.head;
  const indexClean = (await runGitAt(target.root, ['diff', '--cached', '--quiet', '--exit-code'], 10_000)).code === 0;
  const stagedPathsBefore = await stagedPathsAt(target.root);
  if (requireCleanIndex && !indexClean) {
    return {
      ok: false,
      error: {
        code: 'GIT_INDEX_NOT_CLEAN',
        message: 'git_commit requires a clean index before staging paths',
        category: 'conflict',
        retryable: false,
        details: {
          transaction_stage: 'preflight_index',
          expected_head: expectedHead ?? null,
          actual_head_before: actualHeadBefore,
          selected_path_count: paths.length,
          staged_path_count_before: stagedPathsBefore.length,
          index_clean_before: indexClean,
          staged_by_tool: stagedByTool,
          suggestion: 'commit or unstage existing staged changes, or set require_clean_index_before=false'
        }
      }
    };
  }
  if (args.dry_run === true) return ok({
    dry_run: true,
    applied: false,
    message,
    paths,
    all,
    index_clean: indexClean,
    index_clean_before: indexClean,
    staged_path_count_before: stagedPathsBefore.length,
    selected_path_count: paths.length,
    staged_by_tool: stagedByTool,
    expected_head: expectedHead ?? null,
    actual_head_before: actualHeadBefore,
    transaction_stage: 'preflight',
    would_stage: stagedByTool,
    would_commit: true,
    repo: targetMetadata(target),
    warnings: []
  });

  const oldHead = actualHeadBefore;
  if (stagedByTool) {
    const staged = await runGitAt(target.root, all ? ['add', '-A'] : ['add', '--', ...paths]);
    if (staged.code !== 0) {
      const indexRestored = await restoreCleanIndexAfterGitTransactionFailure(target.root, stagedByTool, indexClean);
      return {
        ok: false,
        error: {
          code: 'GIT_STAGE_FAILED',
          message: (staged.stderr || staged.stdout).trim(),
          category: 'runtime',
          retryable: false,
          details: {
            transaction_stage: 'stage',
            expected_head: expectedHead ?? null,
            actual_head_before: oldHead,
            selected_path_count: paths.length,
            staged_path_count_before: stagedPathsBefore.length,
            index_clean_before: indexClean,
            staged_by_tool: stagedByTool,
            index_restored: indexRestored
          }
        }
      };
    }
  }
  const stagedPaths = await stagedPathsAt(target.root);
  const actualHeadAtCommitResult = await runGitAt(target.root, ['rev-parse', 'HEAD'], 10_000);
  const actualHeadAtCommit = actualHeadAtCommitResult.code === 0 ? actualHeadAtCommitResult.stdout.trim() : '';
  if (oldHead.toLowerCase() !== actualHeadAtCommit.toLowerCase()) {
    const indexRestored = await restoreCleanIndexAfterGitTransactionFailure(target.root, stagedByTool, indexClean);
    return gitTransactionHeadChanged({
      expectedHead,
      actualHeadBefore: oldHead,
      actualHeadAtCommit,
      selectedPathCount: paths.length,
      stagedPathCountBefore: stagedPathsBefore.length,
      stagedPathCount: stagedPaths.length,
      indexCleanBefore: indexClean,
      stagedByTool,
      indexRestored
    });
  }
  const argv = ['commit', '-m', message];
  if (args.allow_empty === true) argv.push('--allow-empty');
  const result = await runGitAt(target.root, argv, 60_000);
  if (result.code !== 0) {
    const indexRestored = await restoreCleanIndexAfterGitTransactionFailure(target.root, stagedByTool, indexClean);
    return {
      ok: false,
      error: {
        code: 'GIT_COMMIT_FAILED',
        message: (result.stderr || result.stdout).trim(),
        category: 'runtime',
        retryable: false,
        details: {
          transaction_stage: 'commit',
          stdout: result.stdout,
          expected_head: expectedHead ?? null,
          actual_head_before: oldHead,
          actual_head_at_commit: actualHeadAtCommit,
          selected_path_count: paths.length,
          staged_path_count_before: stagedPathsBefore.length,
          staged_path_count: stagedPaths.length,
          index_clean_before: indexClean,
          staged_by_tool: stagedByTool,
          index_restored: indexRestored
        }
      }
    };
  }
  const head = await runGitAt(target.root, ['rev-parse', 'HEAD']);
  return ok({
    dry_run: false,
    applied: true,
    commit: head.stdout.trim(),
    previous_head: oldHead,
    expected_head: expectedHead ?? null,
    actual_head_before: actualHeadBefore,
    actual_head_at_commit: actualHeadAtCommit,
    actual_head_after: head.stdout.trim(),
    transaction_stage: 'committed',
    selected_path_count: paths.length,
    staged_path_count_before: stagedPathsBefore.length,
    staged_path_count: stagedPaths.length,
    index_clean_before: indexClean,
    staged_by_tool: stagedByTool,
    index_restored: false,
    message,
    paths,
    all,
    stdout: result.stdout.trim(),
    repo: targetMetadata(target),
    status: await gitStatusTool(ctx, key, { path: target.repoPath }),
    affected_files: paths.map(file => ({ path: file, operation: 'commit' })),
    warnings: []
  });
}

export async function gitPushTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const target = await resolveGitTarget(ctx, key, args);
  const mismatch = repoTargetMismatch(target, args.expected_repo_fingerprint);
  if (mismatch) return mismatch;
  await ensureExpectedHead(target, args.expected_head);
  const remote = validateGitRef(args.remote, 'origin');
  const branch = args.branch !== undefined
    ? validateGitRef(args.branch)
    : target.branch !== 'HEAD'
      ? target.branch
      : '';
  if (!branch) throw new Error('branch is required when HEAD is detached');
  const dryRun = args.dry_run === true;
  const argv = ['push'];
  if (dryRun) argv.push('--dry-run');
  if (args.set_upstream === true) argv.push('--set-upstream');
  argv.push(remote, branch);
  const result = await runGitAt(target.root, argv, 120_000);
  if (result.code !== 0) {
    const message = (result.stderr || result.stdout).trim();
    const lowered = message.toLowerCase();
    if (lowered.includes('authentication failed') || lowered.includes('failed to authenticate') || lowered.includes('could not read username')) {
      return {
        ok: false,
        error: {
          code: 'GIT_AUTHENTICATION_FAILED',
          message,
          category: 'authentication',
          retryable: true,
          details: {
            remote,
            branch,
            suggestion: 'Refresh the Git credential/token for this remote, then retry the same git_push request.'
          }
        }
      };
    }
    return {
      ok: false,
      error: {
        code: 'GIT_PUSH_FAILED',
        message,
        category: 'git',
        retryable: false,
        details: { remote, branch, stdout: result.stdout.trim() }
      }
    };
  }
  return ok({
    dry_run: dryRun,
    applied: !dryRun,
    remote,
    branch,
    stdout: result.stdout.trim(),
    stderr: result.stderr.trim(),
    repo: targetMetadata(target),
    status: await gitStatusTool(ctx, key, { path: target.repoPath }),
    affected_files: [],
    warnings: []
  });
}

export async function gitRuntimeRevisionInfo(ctx: ToolContext, key: string): Promise<JsonObject> {
  try {
    const target = await resolveGitTarget(ctx, key, { repo_path: '.' });
    const committed = await runGitAt(target.root, ['show', '-s', '--format=%ct', 'HEAD'], 5_000);
    const committedAtMs = committed.code === 0 ? Number.parseInt(committed.stdout.trim(), 10) * 1000 : NaN;
    return {
      workspace_git_head: target.head,
      workspace_head_committed_at_ms: Number.isFinite(committedAtMs) ? committedAtMs : null
    };
  } catch {
    return { workspace_git_head: null, workspace_head_committed_at_ms: null };
  }
}

export interface GitRestoreSnapshot {
  head: string;
  paths: string[];
  changedPaths: string[];
  addedPaths: string[];
  stagedPatch: string;
  worktreePatch: string;
  stagedBytes: number;
  worktreeBytes: number;
  totalBytes: number;
}

function nulPaths(value: string): string[] {
  return value.split('\0').filter(Boolean).map(item => item.replaceAll('\\', '/'));
}

export async function captureGitRestoreSnapshot(ctx: ToolContext, key: string, paths: string[], repoRoot = rootAndCwd(ctx, key).cwd): Promise<GitRestoreSnapshot> {
  const head = await runGitAt(repoRoot, ['rev-parse', 'HEAD']);
  if (head.code !== 0) throw new Error('GIT_RESTORE_SNAPSHOT_FAILED');
  const common = ['--binary', '--full-index', '--no-ext-diff', '--', ...paths];
  const staged = await runGitAt(repoRoot, ['diff', '--cached', ...common], 60_000);
  const worktree = await runGitAt(repoRoot, ['diff', ...common], 60_000);
  const stagedNames = await runGitAt(repoRoot, ['diff', '--cached', '--name-only', '-z', '--', ...paths], 60_000);
  const worktreeNames = await runGitAt(repoRoot, ['diff', '--name-only', '-z', '--', ...paths], 60_000);
  const headNames = await runGitAt(repoRoot, ['ls-tree', '-r', '--name-only', '-z', head.stdout.trim(), '--', ...paths], 60_000);
  if ([staged, worktree, stagedNames, worktreeNames, headNames].some(result => result.code !== 0)) {
    throw new Error('GIT_RESTORE_SNAPSHOT_FAILED');
  }
  const stagedBytes = Buffer.byteLength(staged.stdout);
  const worktreeBytes = Buffer.byteLength(worktree.stdout);
  const totalBytes = stagedBytes + worktreeBytes;
  if (totalBytes > MAX_GIT_RESTORE_SNAPSHOT_BYTES) throw new Error('GIT_RESTORE_SNAPSHOT_TOO_LARGE');
  const changedPaths = [...new Set([...nulPaths(stagedNames.stdout), ...nulPaths(worktreeNames.stdout)])].sort();
  const headPathSet = new Set(nulPaths(headNames.stdout));
  const addedPaths = changedPaths.filter(item => !headPathSet.has(item));
  const currentHead = await runGitAt(repoRoot, ['rev-parse', 'HEAD']);
  if (currentHead.code !== 0 || currentHead.stdout.trim() !== head.stdout.trim()) throw new Error('GIT_HEAD_CHANGED_DURING_SNAPSHOT');
  return {
    head: head.stdout.trim(),
    paths: [...paths],
    changedPaths,
    addedPaths,
    stagedPatch: staged.stdout,
    worktreePatch: worktree.stdout,
    stagedBytes,
    worktreeBytes,
    totalBytes
  };
}

export async function restoreGitSnapshot(
  ctx: ToolContext,
  key: string,
  snapshot: GitRestoreSnapshot,
  repoRoot = rootAndCwd(ctx, key).cwd
): Promise<{ ok: boolean; steps: JsonObject[] }> {
  const steps: JsonObject[] = [];
  if (!snapshot.changedPaths.length) return { ok: true, steps };
  const added = new Set(snapshot.addedPaths);
  const trackedPaths = snapshot.changedPaths.filter(item => !added.has(item));
  const baselineErrors: string[] = [];
  if (trackedPaths.length) {
    const tracked = await runGitAt(repoRoot, [
      'restore', `--source=${snapshot.head}`, '--staged', '--worktree', '--', ...trackedPaths
    ], 60_000);
    if (tracked.code !== 0) baselineErrors.push(tracked.stderr.trim() || tracked.stdout.trim());
  }
  for (const addedPath of snapshot.addedPaths) {
    const index = await runGitAt(repoRoot, ['rm', '--cached', '--force', '--ignore-unmatch', '--', addedPath], 60_000);
    if (index.code !== 0) baselineErrors.push(index.stderr.trim() || index.stdout.trim());
    try { await rm(resolveInside(repoRoot, addedPath), { force: true, recursive: true }); }
    catch (error) { baselineErrors.push(error instanceof Error ? error.message : String(error)); }
  }
  steps.push({ step: 'baseline', ok: baselineErrors.length === 0, stderr: baselineErrors.filter(Boolean).join('\n') });
  if (baselineErrors.length) return { ok: false, steps };
  if (snapshot.stagedPatch) {
    const staged = await runGitAt(repoRoot, ['apply', '--index', '--binary', '--whitespace=nowarn', '-'], 60_000, snapshot.stagedPatch);
    steps.push({ step: 'staged_patch', ok: staged.code === 0, exit_code: staged.code, stderr: staged.stderr.trim() });
    if (staged.code !== 0) return { ok: false, steps };
  }
  if (snapshot.worktreePatch) {
    const worktree = await runGitAt(repoRoot, ['apply', '--binary', '--whitespace=nowarn', '-'], 60_000, snapshot.worktreePatch);
    steps.push({ step: 'worktree_patch', ok: worktree.code === 0, exit_code: worktree.code, stderr: worktree.stderr.trim() });
    if (worktree.code !== 0) return { ok: false, steps };
  }
  return { ok: true, steps };
}

export async function gitRestoreTool(ctx: ToolContext, key: string, args: JsonObject): Promise<JsonObject> {
  const target = await resolveGitTarget(ctx, key, args);
  const mismatch = repoTargetMismatch(target, args.expected_repo_fingerprint);
  if (mismatch) return mismatch;
  await ensureExpectedHead(target, args.expected_head);
  if (ctx.config.securityPolicy.requireWriteConfirmation && args.confirm !== true) return fail('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION', 'git_restore discards or unstages changes and requires confirm=true');
  const paths = gitPaths(ctx, key, args, true, target);
  if (!paths.length) throw new Error('paths are required');
  const argv = ['restore'];
  const staged = args.staged === true;
  const worktree = args.worktree === undefined ? !staged : args.worktree === true;
  if (!staged && !worktree) throw new Error('git_restore requires staged=true or worktree=true');
  if (staged) argv.push('--staged');
  if (worktree) argv.push('--worktree');
  if (args.source) argv.push(`--source=${String(args.source)}`);
  argv.push('--', ...paths);
  if (args.dry_run === true) return ok({ dry_run: true, applied: false, paths, staged, worktree, rollback_protected: true, repo: targetMetadata(target), warnings: [] });
  let snapshot: GitRestoreSnapshot;
  try {
    snapshot = await captureGitRestoreSnapshot(ctx, key, paths, target.root);
  } catch (error) {
    const code = error instanceof Error && /^[A-Z][A-Z0-9_]+$/.test(error.message)
      ? error.message
      : 'GIT_RESTORE_SNAPSHOT_FAILED';
    return fail(code, 'Unable to capture a bounded path-scoped Git restore snapshot');
  }
  await ensureExpectedHead(target, snapshot.head);
  const result = await runGitAt(target.root, argv, 60_000);
  if (result.code !== 0) {
    const rollback = await restoreGitSnapshot(ctx, key, snapshot, target.root);
    return {
      ok: false,
      error: {
        code: 'GIT_RESTORE_FAILED',
        message: (result.stderr || result.stdout).trim(),
        category: 'runtime',
        retryable: false,
        details: {
          rollback_protected: true,
          rollback_ok: rollback.ok,
          rollback_steps: rollback.steps,
          snapshot_bytes: snapshot.totalBytes,
          snapshot_staged_bytes: snapshot.stagedBytes,
          snapshot_worktree_bytes: snapshot.worktreeBytes
        }
      }
    };
  }
  return ok({
    dry_run: false,
    applied: true,
    paths,
    staged,
    worktree,
    rollback_protected: true,
    snapshot_bytes: snapshot.totalBytes,
    snapshot_staged_bytes: snapshot.stagedBytes,
    snapshot_worktree_bytes: snapshot.worktreeBytes,
    repo: targetMetadata(target),
    status: await gitStatusTool(ctx, key, { path: target.repoPath }),
    affected_files: paths.map(file => ({ path: file, operation: 'restore' })),
    warnings: []
  });
}
