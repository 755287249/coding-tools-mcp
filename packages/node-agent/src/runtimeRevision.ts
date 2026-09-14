import { readFile, realpath, stat } from 'node:fs/promises';
import path from 'node:path';
import { BUILD_GIT_SHA, BUILD_SOURCE_CLEAN } from './version.js';

type GitMetadata = { root: string; gitDir: string; commonDir: string };

async function readText(value: string): Promise<string | null> {
  try {
    return await readFile(value, 'utf8');
  } catch {
    return null;
  }
}

function pathWithin(value: string, parent: string): boolean {
  const relative = path.relative(parent, value);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

async function canonical(value: string): Promise<string | null> {
  try {
    return await realpath(value);
  } catch {
    return null;
  }
}

async function gitMetadataForCanonicalWorkspace(workspace: string): Promise<GitMetadata | null> {
  const marker = path.join(workspace, '.git');
  try {
    if ((await stat(marker)).isDirectory()) {
      const gitDir = await canonical(marker);
      return gitDir && pathWithin(gitDir, workspace) ? { root: workspace, gitDir, commonDir: gitDir } : null;
    }
  } catch {
    // Linked worktrees use a .git pointer file instead of a directory.
  }

  const pointer = await readText(marker);
  const raw = pointer?.trim().match(/^gitdir:\s*(.+)$/i)?.[1]?.trim();
  if (!raw) return null;
  const requestedGitDir = path.isAbsolute(raw) ? raw : path.resolve(workspace, raw);
  const gitDir = await canonical(requestedGitDir);
  if (!gitDir) return null;
  const commonRaw = (await readText(path.join(gitDir, 'commondir')))?.trim();
  if (!commonRaw) return null;
  const requestedCommon = path.isAbsolute(commonRaw) ? commonRaw : path.resolve(gitDir, commonRaw);
  const commonDir = await canonical(requestedCommon);
  if (!commonDir || path.basename(commonDir).toLowerCase() !== '.git') return null;
  const repositoryRoot = path.dirname(commonDir);
  if (!pathWithin(workspace, repositoryRoot)) return null;
  if (!pathWithin(gitDir, path.join(commonDir, 'worktrees'))) return null;
  return { root: workspace, gitDir, commonDir };
}

async function gitMetadataForWorkspace(root: string): Promise<GitMetadata | null> {
  const workspace = await canonical(root);
  return workspace ? gitMetadataForCanonicalWorkspace(workspace) : null;
}

function validGitHash(value: string): boolean {
  return /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(value);
}

function validGitRef(value: string): boolean {
  if (!value.startsWith('refs/') || value.includes('\\')) return false;
  return value.split('/').every(part => part.length > 0 && part !== '.' && part !== '..');
}

function validGitHeadMarker(head: string): boolean {
  const reference = head.match(/^ref:\s*(.+)$/i)?.[1]?.trim();
  return reference ? validGitRef(reference) : validGitHash(head);
}

export async function fastContainingGitRoot(workspaceRoot: string, cwd: string): Promise<string | null> {
  const [workspace, start] = await Promise.all([canonical(workspaceRoot), canonical(cwd)]);
  if (!workspace || !start || !pathWithin(start, workspace)) return null;
  let current = start;
  for (;;) {
    const metadata = await gitMetadataForCanonicalWorkspace(current);
    if (metadata) {
      const head = (await readText(path.join(metadata.gitDir, 'HEAD')))?.trim();
      if (head && validGitHeadMarker(head)) return metadata.root;
    }
    if (path.relative(workspace, current) === '') return null;
    const parent = path.dirname(current);
    if (parent === current || !pathWithin(parent, workspace)) return null;
    current = parent;
  }
}

export type WorkspaceGitIdentity = { branch: string; head: string };

export async function fastWorkspaceGitIdentity(root: string): Promise<WorkspaceGitIdentity | null> {
  const metadata = await gitMetadataForWorkspace(root);
  if (!metadata) return null;
  const head = (await readText(path.join(metadata.gitDir, 'HEAD')))?.trim();
  if (!head) return null;
  const reference = head.match(/^ref:\s*(.+)$/i)?.[1]?.trim();
  if (!reference) return validGitHash(head) ? { branch: 'HEAD', head: head.toLowerCase() } : null;
  if (!validGitRef(reference)) return null;
  const branch = reference.startsWith('refs/heads/') ? reference.slice('refs/heads/'.length) : reference;

  for (const base of [metadata.gitDir, metadata.commonDir]) {
    const value = (await readText(path.join(base, reference)))?.trim();
    if (value && validGitHash(value)) return { branch, head: value.toLowerCase() };
  }
  for (const base of [metadata.gitDir, metadata.commonDir]) {
    const packed = await readText(path.join(base, 'packed-refs'));
    if (!packed) continue;
    for (const line of packed.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('^')) continue;
      const [value, name] = trimmed.split(/\s+/, 2);
      if (name === reference && value && validGitHash(value)) return { branch, head: value.toLowerCase() };
    }
  }
  return null;
}

export async function fastWorkspaceGitHead(root: string): Promise<string | null> {
  return (await fastWorkspaceGitIdentity(root))?.head ?? null;
}

async function isRuntimeSourceWorkspace(root: string): Promise<boolean> {
  const [nodePackage, cargoManifest] = await Promise.all([
    readText(path.join(root, 'packages', 'node-agent', 'package.json')),
    readText(path.join(root, 'src-tauri', 'Cargo.toml'))
  ]);
  return nodePackage?.includes('"name": "@coding-tools/node-agent"') === true
    && cargoManifest?.includes('name = "coding-tools-mcp-desktop"') === true;
}

export async function runtimeRevisionForWorkspace(root?: string): Promise<{
  build_git_sha: string;
  workspace_git_head: string | null;
  source_workspace: boolean;
  matches_workspace: boolean | null;
  source_clean: boolean | null;
  workspace_clean_verified: false;
  workspace_clean_verification_tool: 'git_status';
  trusted: false | null;
  trust_state: 'revision_match_unverified' | 'mismatch' | 'dirty_build' | 'unknown' | 'not_applicable';
  warning: string | null;
}> {
  const sourceWorkspace = root ? await isRuntimeSourceWorkspace(root) : false;
  const workspaceGitHead = root ? await fastWorkspaceGitHead(root) : null;
  const matchesWorkspace = sourceWorkspace && BUILD_GIT_SHA !== 'unknown' && workspaceGitHead
    ? BUILD_GIT_SHA.toLowerCase() === workspaceGitHead.toLowerCase()
    : null;
  const trustState = !sourceWorkspace
    ? 'not_applicable'
    : BUILD_SOURCE_CLEAN === false
      ? 'dirty_build'
      : BUILD_SOURCE_CLEAN === true && matchesWorkspace === true
        ? 'revision_match_unverified'
        : BUILD_SOURCE_CLEAN === true && matchesWorkspace === false
          ? 'mismatch'
          : 'unknown';
  const trusted: false | null = trustState === 'dirty_build' || trustState === 'mismatch' ? false : null;
  const warning = trustState === 'dirty_build'
    ? 'The Node Agent was built from a dirty worktree; rebuild from a clean commit before trusting live schemas or behavior.'
    : trustState === 'mismatch'
      ? `Running Node Agent build ${BUILD_GIT_SHA} differs from workspace HEAD ${workspaceGitHead}. Restart/rebuild before trusting live schemas or behavior.`
      : trustState === 'revision_match_unverified'
        ? 'Build commit matches workspace HEAD, but server_info does not inspect uncommitted worktree changes. Confirm git_status.clean before treating runtime and source as identical.'
        : null;
  return {
    build_git_sha: BUILD_GIT_SHA,
    workspace_git_head: workspaceGitHead,
    source_workspace: sourceWorkspace,
    matches_workspace: matchesWorkspace,
    source_clean: BUILD_SOURCE_CLEAN,
    workspace_clean_verified: false,
    workspace_clean_verification_tool: 'git_status',
    trusted,
    trust_state: trustState,
    warning
  };
}
