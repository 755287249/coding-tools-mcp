import { randomBytes } from 'node:crypto';
import { homedir } from 'node:os';
import { chmod, mkdir, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { AgentSecrets } from './types.js';
import { protectAvailable, unwrapJson, wrapJson } from './protect.js';
import {
  parseCanonicalWorkspace,
  serializeCanonicalWorkspace,
  type CanonicalWorkspace
} from './workspaceDocument.js';

const COMMON_SECRET_KEYS = {
  oauthPassword: 'oauth_password',
  oauthClientSecret: 'oauth_client_secret',
  oauthTokenSecret: 'oauth_token_secret',
  tunnelEnrollmentUrl: 'builtin_tunnel_enrollment_url'
} as const;

type SharedSecretValueKey = typeof COMMON_SECRET_KEYS[keyof typeof COMMON_SECRET_KEYS];

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? { ...(value as Record<string, unknown>) }
    : {};
}

function optionalSecret(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

export function sharedWorkspacesRoot(): string {
  const configured = process.env.CTMCP_SHARED_WORKSPACES_ROOT?.trim();
  if (configured) return path.resolve(configured);
  const local = process.env.LOCALAPPDATA?.trim()
    || process.env.XDG_DATA_HOME?.trim()
    || path.join(homedir(), '.local', 'share');
  return path.join(local, 'CodingToolsMCP', 'workspaces');
}

export function sharedWorkspaceFile(id: string): string {
  return path.join(sharedWorkspacesRoot(), id, 'workspace.json');
}

export function sharedSecretsFile(id: string): string {
  return path.join(sharedWorkspacesRoot(), id, 'secrets.json');
}

export function sharedStoreAvailable(): boolean {
  return process.env.CTMCP_SHARED_STORE_DISABLED !== '1' && protectAvailable();
}

async function readWrappedFile(filePath: string): Promise<unknown | undefined> {
  try {
    const raw = JSON.parse(await readFile(filePath, 'utf8')) as unknown;
    return unwrapJson(raw, filePath);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

async function writeWrappedFile(filePath: string, value: unknown): Promise<void> {
  await mkdir(path.dirname(filePath), { recursive: true, mode: 0o700 });
  const wrapped = wrapJson(value, filePath);
  const temporary = `${filePath}.${process.pid}.${randomBytes(6).toString('hex')}.tmp`;
  await writeFile(temporary, `${JSON.stringify(wrapped, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  try {
    await rename(temporary, filePath);
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => undefined);
    throw error;
  }
  await chmod(filePath, 0o600).catch(() => undefined);
}

export async function readSharedWorkspace(id: string): Promise<CanonicalWorkspace | undefined> {
  if (!sharedStoreAvailable()) return undefined;
  const value = await readWrappedFile(sharedWorkspaceFile(id));
  return value === undefined ? undefined : parseCanonicalWorkspace(value);
}

function folderIdentity(canonical: CanonicalWorkspace): string {
  const normalize = (value: string): string => {
    let normalized = path.resolve(value).replaceAll('\\', '/').replace(/\/$/, '');
    if (process.platform === 'win32') normalized = normalized.toLowerCase();
    return normalized;
  };
  return canonical.folders.map(folder => normalize(folder.path)).sort().join('\n');
}

function hasHostState(canonical: CanonicalWorkspace, host: 'desktop' | 'node'): boolean {
  return Object.keys(canonical.host[host] ?? {}).length > 0;
}

export async function findCounterpartSharedWorkspaceId(
  canonical: CanonicalWorkspace,
  currentId: string,
  counterpart: 'desktop' | 'node'
): Promise<string | undefined> {
  if (!sharedStoreAvailable()) return undefined;
  const identity = folderIdentity(canonical);
  const current = await readSharedWorkspace(currentId);
  if (current && folderIdentity(current) === identity && hasHostState(current, counterpart)) return currentId;

  let entries;
  try {
    entries = await readdir(sharedWorkspacesRoot(), { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
  const matches: string[] = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.name === currentId) continue;
    const candidate = await readSharedWorkspace(entry.name);
    if (!candidate || !hasHostState(candidate, counterpart)) continue;
    if (folderIdentity(candidate) === identity) matches.push(entry.name);
  }
  return matches.length === 1 ? matches[0] : undefined;
}

export async function writeSharedWorkspace(id: string, canonical: CanonicalWorkspace): Promise<string | undefined> {
  if (!sharedStoreAvailable()) return undefined;
  const filePath = sharedWorkspaceFile(id);
  await writeWrappedFile(filePath, serializeCanonicalWorkspace(canonical));
  return filePath;
}

function valuesFromDocument(document: Record<string, unknown>): Record<string, unknown> {
  const values = record(document.values);
  const legacy: Record<SharedSecretValueKey, unknown> = {
    oauth_password: document.oauthPassword,
    oauth_client_secret: document.oauthClientSecret,
    oauth_token_secret: document.oauthTokenSecret,
    builtin_tunnel_enrollment_url: document.tunnelEnrollmentUrl
  };
  for (const [key, value] of Object.entries(legacy)) {
    if (values[key] === undefined && typeof value === 'string' && value.length > 0) values[key] = value;
  }
  return values;
}

function agentSecretsFromValues(values: Record<string, unknown>): AgentSecrets {
  const named = Object.fromEntries(Object.entries(values)
    .filter(([key, value]) => !Object.values(COMMON_SECRET_KEYS).includes(key as SharedSecretValueKey)
      && /^[A-Za-z0-9._-]{1,128}$/.test(key)
      && typeof value === 'string'
      && value.length > 0)
    .map(([key, value]) => [key, String(value)]));
  return {
    oauthPassword: optionalSecret(values.oauth_password),
    oauthClientSecret: optionalSecret(values.oauth_client_secret),
    oauthTokenSecret: optionalSecret(values.oauth_token_secret),
    tunnelEnrollmentUrl: optionalSecret(values.builtin_tunnel_enrollment_url),
    ...(Object.keys(named).length > 0 ? { named } : {})
  };
}

export interface SharedFileSnapshot {
  filePath: string;
  content?: string;
}

export async function snapshotSharedSecrets(id: string): Promise<SharedFileSnapshot | undefined> {
  if (!sharedStoreAvailable()) return undefined;
  const filePath = sharedSecretsFile(id);
  try {
    return { filePath, content: await readFile(filePath, 'utf8') };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { filePath };
    throw error;
  }
}

export async function restoreSharedSecrets(snapshot: SharedFileSnapshot | undefined): Promise<void> {
  if (!snapshot) return;
  if (snapshot.content === undefined) {
    await rm(snapshot.filePath, { force: true });
    return;
  }
  await mkdir(path.dirname(snapshot.filePath), { recursive: true, mode: 0o700 });
  const temporary = `${snapshot.filePath}.${process.pid}.${randomBytes(6).toString('hex')}.rollback.tmp`;
  await writeFile(temporary, snapshot.content, { flag: 'wx', mode: 0o600 });
  try {
    await rename(temporary, snapshot.filePath);
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => undefined);
    throw error;
  }
  await chmod(snapshot.filePath, 0o600).catch(() => undefined);
}

export async function readSharedAgentSecrets(id: string): Promise<AgentSecrets | undefined> {
  if (!sharedStoreAvailable()) return undefined;
  const value = await readWrappedFile(sharedSecretsFile(id));
  if (value === undefined) return undefined;
  return agentSecretsFromValues(valuesFromDocument(record(value)));
}

export async function writeSharedAgentSecrets(id: string, secrets: AgentSecrets): Promise<string | undefined> {
  if (!sharedStoreAvailable()) return undefined;
  const filePath = sharedSecretsFile(id);
  const existing = record(await readWrappedFile(filePath));
  const values = valuesFromDocument(existing);
  for (const [key, value] of Object.entries(secrets.named ?? {})) {
    if (/^[A-Za-z0-9._-]{1,128}$/.test(key) && value) values[key] = value;
  }
  for (const [property, key] of Object.entries(COMMON_SECRET_KEYS) as Array<[keyof AgentSecrets, SharedSecretValueKey]>) {
    const value = secrets[property];
    if (typeof value === 'string' && value.length > 0) values[key] = value;
    else delete values[key];
  }
  const next: Record<string, unknown> = { ...existing, schemaVersion: 1, values };
  delete next.oauthPassword;
  delete next.oauthClientSecret;
  delete next.oauthTokenSecret;
  delete next.tunnelEnrollmentUrl;
  await writeWrappedFile(filePath, next);
  return filePath;
}
