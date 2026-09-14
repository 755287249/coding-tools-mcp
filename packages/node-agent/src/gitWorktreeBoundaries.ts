import path from 'node:path';
import { runGitBuffered } from './gitProcess.js';
import { parseWslUncPath } from './wsl.js';

function relativeWorktreePath(root: string, candidate: string): string | undefined {
  const rootWsl = parseWslUncPath(root);
  if (rootWsl) {
    if (!candidate.startsWith('/')) return undefined;
    const relative = path.posix.relative(rootWsl.linuxPath, path.posix.normalize(candidate));
    if (!relative || relative === '.' || relative === '..' || relative.startsWith('../') || path.posix.isAbsolute(relative)) return undefined;
    return relative;
  }
  const relative = path.relative(path.resolve(root), path.resolve(candidate)).replaceAll('\\', '/');
  if (!relative || relative === '.' || relative === '..' || relative.startsWith('../') || path.isAbsolute(relative)) return undefined;
  return relative;
}

export function worktreeContainerPrefixesFromPorcelain(root: string, porcelain: string): string[] {
  const prefixes = new Set<string>();
  for (const field of porcelain.split('\0')) {
    if (!field.startsWith('worktree ')) continue;
    const relative = relativeWorktreePath(root, field.slice('worktree '.length).trim());
    if (!relative) continue;
    const firstSegment = relative.split('/')[0];
    if (firstSegment) prefixes.add(firstSegment);
  }
  return [...prefixes].sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)));
}

export async function nestedWorktreeContainerPrefixes(root: string): Promise<string[]> {
  const listed = await runGitBuffered(root, ['worktree', 'list', '--porcelain', '-z']);
  if (listed.code !== 0) return [];
  return worktreeContainerPrefixesFromPorcelain(root, listed.stdout);
}

export async function scanExcludedWorktreeContainerPrefixes(root: string): Promise<string[]> {
  const prefixes = await nestedWorktreeContainerPrefixes(root);
  if (!prefixes.length) return [];
  const tracked = await Promise.all(prefixes.map(async prefix => ({
    prefix,
    result: await runGitBuffered(root, ['ls-files', '--cached', '-z', '--', `:(literal)${prefix}`])
  })));
  return tracked
    .filter(item => item.result.code === 0 && item.result.stdout.length === 0)
    .map(item => item.prefix);
}
