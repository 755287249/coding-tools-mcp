/**
 * CodeRabbit seeding plugin: log in once (desktop login window or a pasted
 * session), then create coding tasks by protocol only. Every request goes
 * through the backend `plugins.http` proxy, which allows only CodeRabbit tRPC
 * and the GitHub endpoints used to create a starter repository.
 */
export const CODERABBIT = 'https://app.coderabbit.ai';
export const DEFAULT_SEED_COUNT = 30;
const BATCH = 10;

export interface PluginHttpRequest { url: string; method: 'GET' | 'POST' | 'PUT'; headers?: Record<string, string>; body?: string }
export interface PluginHttpResponse { status: number; body: string }
export type PluginHttp = (request: PluginHttpRequest) => Promise<PluginHttpResponse>;

/** Values the CodeRabbit web app keeps in session/local storage after login. */
export interface CapturedStorage { accessToken: string; user: string; clerkGitProvider?: string; clerkSsoWorkspaceId?: string }
export interface CodeRabbitSession {
  access: string; account: string; accountName: string; providerUserId: string; provider: string;
  organization: string; workspace: string; orgName: string; providerOrg: string; orgType: string;
  /** Access-token expiry in ms when the token is a JWT, else 0. */
  expiresAt: number;
}
export interface SeedTicketLike { seed_id: string; ticket: string; url_key?: string; prefix?: string; repo_id: string; branch: string }
export interface Repository { id: string; name: string; branch: string }

export class SessionExpiredError extends Error { constructor() { super('CodeRabbit session expired; log in again'); this.name = 'SessionExpiredError'; } }
/** The task may exist on CodeRabbit; never retry it automatically. */
export class UncertainTaskError extends Error {}

const ID = /^[A-Za-z0-9_-]{1,200}$/;

/** Turn the captured web storage into a session. Same fields as the userscript reads. */
export function parseSession(raw: CapturedStorage | string): CodeRabbitSession {
  let value: CapturedStorage;
  try { value = typeof raw === 'string' ? JSON.parse(raw.trim()) : raw; } catch { throw new Error('Session text is not valid JSON'); }
  const access = String(value?.accessToken ?? '').trim();
  let state: any;
  try { state = JSON.parse(String(value?.user ?? ''))?.state; } catch { state = null; }
  const user = state?.user, org = state?.currentOrganization;
  if (!access || typeof user?.id !== 'string' || !user.id) throw new Error('Not logged in to CodeRabbit');
  if (!org?.id || !org?.organization_name) throw new Error('Select an organization in CodeRabbit first');
  const provider = String(value.clerkGitProvider ?? '');
  if (provider && !/^[a-z0-9-]{1,80}$/.test(provider)) throw new Error('Unexpected Git provider');
  const workspace = String(org.workspace_id ?? value.clerkSsoWorkspaceId ?? '');
  if (!ID.test(org.id) || (workspace && !ID.test(workspace))) throw new Error('Unexpected organization ID');
  return {
    access, account: user.id, accountName: String(user.providerLogin || user.userName || user.id),
    providerUserId: String(user.providerUserId ?? ''), provider, organization: org.id, workspace,
    orgName: String(org.organization_name), providerOrg: String(org.provider_organization_id ?? ''),
    orgType: String(org.type || provider || 'github'), expiresAt: jwtExpiry(access),
  };
}

export function jwtExpiry(token: string): number {
  const part = token.split('.')[1];
  if (!part) return 0;
  try {
    const json = JSON.parse(atob(part.replace(/-/g, '+').replace(/_/g, '/')));
    return typeof json.exp === 'number' ? json.exp * 1000 : 0;
  } catch { return 0; }
}

/** Console snippet for the Node Agent (no login window): copies the session JSON. */
export const SESSION_SNIPPET = "copy(JSON.stringify({accessToken:sessionStorage.getItem('accessToken'),user:sessionStorage.getItem('user'),clerkGitProvider:localStorage.getItem('clerkGitProvider')||'',clerkSsoWorkspaceId:localStorage.getItem('clerkSsoWorkspaceId')||''}))";

export function trpcHeaders(session: CodeRabbitSession): Record<string, string> {
  return {
    authorization: `Bearer ${session.access}`, 'content-type': 'application/json', 'x-trpc-source': 'react',
    ...(session.provider ? { 'x-clerk-git-provider': session.provider } : {}),
    'x-coderabbitai-organization': session.organization,
    ...(session.workspace ? { 'x-coderabbitai-workspace': session.workspace } : {}),
  };
}

export function unwrapTrpc(body: unknown): any {
  const row: any = Array.isArray(body) ? body[0] : body;
  if (!row || row.error) {
    const code = row?.error?.json?.data?.code ?? row?.error?.data?.code;
    if (code === 'UNAUTHORIZED') throw new SessionExpiredError();
    throw new Error('CodeRabbit returned an error; check login, account access, repository and quota');
  }
  if (!row.result || !Object.hasOwn(row.result, 'data')) throw new Error('CodeRabbit response format changed; stopped');
  const data = row.result.data;
  return data && typeof data === 'object' && Object.hasOwn(data, 'json') ? data.json : data;
}

export function createdTaskId(result: any): string {
  const id = [result?.taskId, result?.id, result?.task?.id, result?.task?.taskId]
    .find(v => typeof v === 'string' && /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(v));
  if (!id) throw new UncertainTaskError('CodeRabbit did not return a task ID; check the task list before continuing');
  return id;
}

/** Same short URL-first prompt the userscript sends (seed URL carries its own credential). */
export function seedPrompt(endpoint: string, folderId: string, seed: SeedTicketLike): string {
  if (!seed.url_key || !/^[a-f0-9]{64}$/.test(seed.url_key)) throw new Error('Seed URL missing; update the app');
  const url = `${endpoint}/seeds/${encodeURIComponent(folderId)}/${seed.url_key}`;
  return `${url} 快速连接这个 sandbox 的 MCP(URL)，明确使用规则，熟悉可用工具，做好处理接下来一系列工作的准备。chat_wait/chat_reply 工具在 mcp 内部，而非你的客户端内部，连上后使用，然后等待信息`;
}

export function creationPayload(endpoint: string, folderId: string, seed: SeedTicketLike) {
  return { branch_name: seed.branch, clientRequestId: seed.seed_id, repoId: seed.repo_id,
    source: { type: 'manual', instructions: seedPrompt(endpoint, folderId, seed) },
    title: `${seed.prefix ? seed.prefix + ' ' : ''}MCP Seed ${seed.seed_id}` };
}

export function publicMcpEndpoint(publicUrl: string | undefined | null): string {
  if (!publicUrl) throw new Error('Start the public MCP tunnel first: CodeRabbit must reach this app');
  const url = new URL(publicUrl.trim());
  if (url.protocol !== 'https:') throw new Error('The public MCP address must use HTTPS');
  url.pathname = url.pathname.replace(/\/$/, '').replace(/\/mcp$/, '') + '/mcp';
  url.search = ''; url.hash = '';
  return url.toString().replace(/\/$/, '');
}

export class CodeRabbitClient {
  private http: PluginHttp;
  session: CodeRabbitSession;
  constructor(http: PluginHttp, session: CodeRabbitSession) { this.http = http; this.session = session; }
  private async call(procedure: string, input: unknown, method: 'GET' | 'POST'): Promise<any> {
    const query = method === 'GET' ? '&input=' + encodeURIComponent(JSON.stringify({ 0: input })) : '';
    const response = await this.http({ url: `${CODERABBIT}/trpc/${procedure}?batch=1${query}`, method,
      headers: trpcHeaders(this.session), ...(method === 'POST' ? { body: JSON.stringify({ 0: input }) } : {}) });
    if (response.status === 401) throw new SessionExpiredError();
    let body: unknown;
    try { body = JSON.parse(response.body); } catch { throw new Error(`CodeRabbit ${procedure} failed (HTTP ${response.status})`); }
    if (response.status >= 400 && !Array.isArray(body)) throw new Error(`CodeRabbit ${procedure} failed (HTTP ${response.status})`);
    return unwrapTrpc(body);
  }
  query(procedure: string, input: unknown) { return this.call(procedure, input, 'GET'); }
  mutate(procedure: string, input: unknown) { return this.call(procedure, input, 'POST'); }

  async repositories(refresh = false): Promise<{ id: string; name: string; defaultBranch?: string }[]> {
    const s = this.session;
    const data = await this.query('repositories.getAllRepos', { provider: s.provider || 'github', orgIds: s.providerOrg, orgName: s.orgName,
      type: s.orgType, perPage: 100, page: 1, searchQuery: '', skipInstallEligibility: true, repoSort: 'activity', ...(refresh ? { refresh: true } : {}) });
    if (data?.loadFailed) throw new Error('CodeRabbit could not load repositories; try again');
    return (Array.isArray(data?.repos) ? data.repos : []).filter((r: any) => r?.id && r?.name)
      .map((r: any) => ({ id: String(r.id), name: String(r.name), defaultBranch: typeof r.defaultBranch === 'string' ? r.defaultBranch : undefined }));
  }

  /** The most recently active repository (CodeRabbit sorts by activity). */
  async latestRepository(createRepository?: () => Promise<void>): Promise<Repository> {
    let repos = await this.repositories();
    if (!repos.length && createRepository) {
      await createRepository();
      for (let attempt = 0; attempt < 6 && !repos.length; attempt++) {
        if (attempt) await new Promise(resolve => setTimeout(resolve, 5000));
        repos = await this.repositories(true);
      }
      if (!repos.length) throw new Error('The new repository is not visible in CodeRabbit yet. Give the CodeRabbit GitHub app access to it, then try again');
    }
    if (!repos.length) throw new Error('No repository in CodeRabbit');
    const repo = repos[0];
    let branch = repo.defaultBranch?.trim() ?? '';
    if (!branch) {
      try { branch = String((await this.query('codingAgent.getRepositoryDefaultBranch', { repoId: repo.id }))?.defaultBranch ?? '').trim(); }
      catch (error) { if (error instanceof SessionExpiredError) throw error; }
    }
    return { id: repo.id, name: repo.name, branch: branch || 'main' };
  }

  async enqueue(endpoint: string, folderId: string, seed: SeedTicketLike): Promise<string> {
    let result: unknown;
    try { result = await this.mutate('codingAgent.enqueueCodingTask', creationPayload(endpoint, folderId, seed)); }
    catch (error) {
      if (error instanceof SessionExpiredError) throw error;
      throw new UncertainTaskError(`Seed ${seed.seed_id}: task creation result unknown; check the CodeRabbit task list. ${error instanceof Error ? error.message : ''}`.trim());
    }
    return createdTaskId(result);
  }
}

export interface SeedingProgress { total: number; done: number; repo?: string; branch?: string; lastTask?: string; message?: string }
export interface SeedingOptions {
  count?: number; prefix?: string; endpoint: string; folderId: string;
  client: CodeRabbitClient;
  /** Library actions: `seed_batch` (returns tickets) and `seed_created`. */
  library: (args: Record<string, unknown>) => Promise<any>;
  createRepository?: () => Promise<void>;
  /** Called once when the access token expires; returns a fresh session or throws. */
  relogin?: () => Promise<CodeRabbitSession>;
  onProgress?: (progress: SeedingProgress) => void;
  signal?: AbortSignal;
  delayMs?: number;
}

/** Create `count` seeds (default 30) on the latest repository. Stops on the first uncertain task. */
export async function runSeeding(options: SeedingOptions): Promise<SeedingProgress> {
  const total = options.count ?? DEFAULT_SEED_COUNT;
  if (!Number.isInteger(total) || total < 1 || total > 1000) throw new Error('Seed count must be 1–1000');
  const prefix = (options.prefix ?? '').trim();
  if (/[\r\n\0]/.test(prefix)) throw new Error('Prefix cannot contain line breaks');
  const progress: SeedingProgress = { total, done: 0, message: 'Reading repositories…' };
  const report = () => options.onProgress?.({ ...progress });
  report();
  const retryOnce = async <T>(run: () => Promise<T>): Promise<T> => {
    try { return await run(); }
    catch (error) {
      if (!(error instanceof SessionExpiredError) || !options.relogin) throw error;
      progress.message = 'Session expired, logging in again…'; report();
      options.client.session = await options.relogin();
      return await run();
    }
  };
  const repo = await retryOnce(() => options.client.latestRepository(options.createRepository));
  Object.assign(progress, { repo: repo.name, branch: repo.branch, message: `Repository ${repo.name} · ${repo.branch}` }); report();
  const session = () => options.client.session;
  while (progress.done < total) {
    if (options.signal?.aborted) { progress.message = 'Stopped'; report(); break; }
    const s = session();
    const count = Math.min(BATCH, total - progress.done);
    const { batch } = await options.library({ action: 'seed_batch', source: 'coderabbit', prefix, count, account: s.accountName,
      repo_id: repo.id, branch: repo.branch, metadata: { account_id: s.account, provider_user_id: s.providerUserId, repo_name: repo.name,
        organization_id: s.organization, workspace_id: s.workspace } });
    if (!Array.isArray(batch) || batch.length !== count) throw new Error('Seed library returned an unexpected batch');
    for (const seed of batch as SeedTicketLike[]) {
      if (options.signal?.aborted) break;
      const taskId = await retryOnce(() => options.client.enqueue(options.endpoint, options.folderId, seed));
      await options.library({ action: 'seed_created', seed_id: seed.seed_id, ticket: seed.ticket, task_id: taskId });
      progress.done++; progress.lastTask = taskId; progress.message = `${progress.done} / ${total}`; report();
      if (progress.done < total && options.delayMs !== 0) await new Promise(resolve => setTimeout(resolve, options.delayMs ?? 1000));
    }
  }
  if (progress.done === total) { progress.message = 'Done'; report(); }
  return progress;
}
