import { realpathSync, statSync } from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';

/** Local UI only: resolve a clicked path, never execute its contents. */
export function resolveChatPath(root: string, value: unknown): {path: string; is_directory: boolean} {
  if (typeof value !== 'string' || !value.trim() || value.length > 4096) throw new Error('Invalid local path');
  const input = value.trim().replace(/\\/g, '/');
  if (/[\x00-\x1f\x7f<>"|?*]/.test(input) || input.startsWith('//')) throw new Error('Only local filesystem paths are supported');
  const drive = /^[A-Za-z]:\//.test(input);
  if ((drive ? input.slice(2) : input).includes(':') || (drive && process.platform !== 'win32')) throw new Error('Unsupported local path');
  if (input.split('/').includes('..')) throw new Error('Parent traversal is not allowed');
  const base = realpathSync(root), absolute = path.isAbsolute(input);
  const selected = realpathSync(absolute ? input : path.join(base, input));
  if (!absolute) {
    const relative = path.relative(base, selected);
    if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) throw new Error('Path escapes the selected folder');
  }
  if (process.platform === 'win32' && !/^[A-Za-z]:\\/.test(selected)) throw new Error('Only local drive paths are supported');
  const metadata = statSync(selected);
  if (!metadata.isDirectory() && !metadata.isFile()) throw new Error('Select a regular file or folder');
  return {path: selected, is_directory: metadata.isDirectory()};
}
export function fileManagerPlan(platform: string, selected: string, directory: boolean): {command: string; args: string[]} {
  if (platform === 'win32') return {command: 'explorer.exe', args: directory ? [selected] : ['/select,', selected]};
  if (platform === 'darwin') return {command: 'open', args: directory ? [selected] : ['-R', selected]};
  if (platform === 'linux') return {command: 'xdg-open', args: [directory ? selected : path.dirname(selected)]};
  throw new Error('File manager is not supported on this platform');
}
export async function revealChatPath(selected: string, directory: boolean): Promise<void> {
  const plan = fileManagerPlan(process.platform, selected, directory);
  const command = process.platform === 'win32' ? path.join(process.env.SystemRoot || 'C:\\Windows', plan.command) : plan.command;
  await new Promise<void>((resolve, reject) => {
    const child = spawn(command, plan.args, {shell: false, detached: true, stdio: 'ignore'});
    child.once('error', reject);
    child.once('spawn', () => {child.unref(); resolve();});
  });
}
