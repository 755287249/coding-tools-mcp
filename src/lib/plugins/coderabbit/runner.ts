import { get } from 'svelte/store';
import { getBackend } from '$lib/backend';
import { seedRequest } from '$lib/api/seeds';
import { CodeRabbitClient, parseSession, publicMcpEndpoint, runSeeding, type CodeRabbitSession } from './engine';
import { createStarterRepository, pollDeviceToken, startDeviceFlow } from './github';
import { beginJob, coderabbitSession, githubDeviceCode, seedingJob } from './store';

const LOGIN_URL = 'https://app.coderabbit.ai/login';

export function canOpenLoginWindow(): boolean { return typeof getBackend().plugins.loginWindow === 'function'; }

/** Desktop: open the CodeRabbit login window (GitHub login) and keep the session in memory. */
export async function loginWithWindow(silent = false): Promise<CodeRabbitSession> {
  const open = getBackend().plugins.loginWindow;
  if (!open) throw new Error('Login window is only available in the desktop app');
  const captured = await open({ url: LOGIN_URL, capture: true, auto: silent, hidden: silent });
  if (!captured) throw new Error('CodeRabbit login was not completed');
  const session = parseSession(captured);
  coderabbitSession.set(session);
  return session;
}

export function loginWithText(text: string): CodeRabbitSession {
  const session = parseSession(text);
  coderabbitSession.set(session);
  return session;
}

async function createRepository(signal: AbortSignal): Promise<void> {
  const http = getBackend().plugins.http;
  const code = await startDeviceFlow(http);
  githubDeviceCode.set({ user_code: code.user_code, verification_uri: code.verification_uri });
  try {
    if (canOpenLoginWindow()) void getBackend().plugins.loginWindow!({ url: 'https://github.com/login/device', capture: false }).catch(() => {});
    const token = await pollDeviceToken(http, code, signal);
    seedingJob.update(job => job && { ...job, message: 'Creating a starter GitHub repository…' });
    await createStarterRepository(http, token);
  } finally { githubDeviceCode.set(null); }
}

/** Start seeding in the background; the top progress bar follows `seedingJob`. */
export function startSeeding(options: { workspaceId: string; folderId: string; publicUrl: string | undefined | null; count: number; prefix: string }): void {
  const session = get(coderabbitSession);
  if (!session) throw new Error('Log in to CodeRabbit first');
  const endpoint = publicMcpEndpoint(options.publicUrl);
  const signal = beginJob();
  const id = Date.now();
  seedingJob.set({ id, total: options.count, done: 0, status: 'running', message: '', minimized: false, workspaceId: options.workspaceId, folderId: options.folderId });
  const client = new CodeRabbitClient(getBackend().plugins.http, session);
  void runSeeding({
    count: options.count, prefix: options.prefix, endpoint, folderId: options.folderId, client, signal,
    library: args => seedRequest(options.workspaceId, options.folderId, args),
    createRepository: () => createRepository(signal),
    relogin: canOpenLoginWindow() ? () => loginWithWindow(true) : undefined,
    onProgress: progress => seedingJob.update(job => job?.id === id ? { ...job, done: progress.done, total: progress.total, repo: progress.repo, message: progress.message ?? '' } : job),
  }).then(progress => {
    seedingJob.update(job => job?.id === id ? { ...job, done: progress.done, status: signal.aborted && progress.done < progress.total ? 'stopped' : 'done' } : job);
  }).catch(error => {
    if (error?.name === 'SessionExpiredError' || /session expired/i.test(String(error?.message))) coderabbitSession.set(null);
    seedingJob.update(job => job?.id === id ? { ...job, status: 'error', message: error instanceof Error ? error.message : String(error) } : job);
  });
}
