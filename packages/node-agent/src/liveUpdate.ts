import { spawn } from 'node:child_process';
import { createHash, createPublicKey, verify } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const NODE_UPDATE_PUBLIC_KEY_ENV = 'CTMCP_UPDATE_PUBLIC_KEY';
export const NODE_UPDATE_SIGNATURE_DOMAIN = 'coding-tools-node-update-v1';
export const MAX_NODE_UPDATE_BYTES = 512 * 1024 * 1024;

export interface NodeUpdateOffer {
  update_id: string;
  version: string;
  url: string;
  sha256: string;
  signature: string;
}

export interface PreparedNodeUpdate {
  offer: NodeUpdateOffer;
  archivePath: string;
  packageRoot: string;
  gitCommit: string;
}

interface CommandResult {
  code: number;
  stdout: string;
  stderr: string;
}

export interface NodeUpdateRuntimeOptions {
  fetchImpl?: typeof fetch;
  trustedKeyRaw?: string;
  currentPackageRoot?: string;
  commandRunner?: (program: string, args: readonly string[]) => Promise<CommandResult>;
}

function requireString(value: unknown, field: string, maxLength: number): string {
  if (typeof value !== 'string') throw new Error(`update ${field} must be a string`);
  const normalized = value.trim();
  if (!normalized || normalized.length > maxLength) throw new Error(`update ${field} is invalid`);
  return normalized;
}

export function normalizeNodeUpdateOffer(value: unknown): NodeUpdateOffer {
  if (!value || typeof value !== 'object') throw new Error('update offer must be an object');
  const record = value as Record<string, unknown>;
  const updateId = requireString(record.update_id, 'id', 128);
  if (!/^[A-Za-z0-9._-]+$/.test(updateId)) throw new Error('update id contains unsupported characters');
  const version = requireString(record.version, 'version', 64);
  if (!/^[0-9A-Za-z.+-]+$/.test(version)) throw new Error('update version contains unsupported characters');
  const url = requireString(record.url, 'URL', 2048);
  let parsed: URL;
  try { parsed = new URL(url); } catch { throw new Error('update URL is invalid'); }
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password) {
    throw new Error('update URL must use HTTPS without embedded credentials');
  }
  const sha256 = requireString(record.sha256, 'sha256', 64).toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(sha256)) throw new Error('update sha256 must be 64 hexadecimal characters');
  const signature = requireString(record.signature, 'signature', 128);
  let signatureBytes: Buffer;
  try { signatureBytes = Buffer.from(signature, 'base64url'); } catch { throw new Error('update signature is not valid base64url'); }
  if (signatureBytes.length !== 64 || signatureBytes.toString('base64url') !== signature) {
    throw new Error('update signature must be a canonical 64-byte Ed25519 base64url value');
  }
  return { update_id: updateId, version, url: parsed.toString(), sha256, signature };
}

export function nodeUpdateSigningPayload(version: string, sha256: string): Buffer {
  return Buffer.from(`${NODE_UPDATE_SIGNATURE_DOMAIN}\n${version}\n${sha256.toLowerCase()}\n`, 'utf8');
}

export function trustedNodeUpdatePublicKey(raw = process.env[NODE_UPDATE_PUBLIC_KEY_ENV]): ReturnType<typeof createPublicKey> {
  if (!raw?.trim()) throw new Error(`${NODE_UPDATE_PUBLIC_KEY_ENV} is required before remote Node Agent updates can be accepted`);
  const encoded = raw.trim();
  let bytes: Buffer;
  try { bytes = Buffer.from(encoded, 'base64url'); } catch { throw new Error(`${NODE_UPDATE_PUBLIC_KEY_ENV} must be base64url`); }
  if (bytes.length !== 32 || bytes.toString('base64url') !== encoded) {
    throw new Error(`${NODE_UPDATE_PUBLIC_KEY_ENV} must be a canonical raw 32-byte Ed25519 public key`);
  }
  const spki = Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), bytes]);
  return createPublicKey({ key: spki, format: 'der', type: 'spki' });
}

export function verifyNodeUpdateOfferSignature(offerValue: unknown, trustedKeyRaw?: string): NodeUpdateOffer {
  const offer = normalizeNodeUpdateOffer(offerValue);
  const publicKey = trustedNodeUpdatePublicKey(trustedKeyRaw);
  const valid = verify(
    null,
    nodeUpdateSigningPayload(offer.version, offer.sha256),
    publicKey,
    Buffer.from(offer.signature, 'base64url')
  );
  if (!valid) throw new Error('Node Agent update signature verification failed');
  return offer;
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

export function verifyNodeUpdateArtifact(offerValue: unknown, bytes: Uint8Array, trustedKeyRaw?: string): NodeUpdateOffer {
  const offer = verifyNodeUpdateOfferSignature(offerValue, trustedKeyRaw);
  const actual = sha256Hex(bytes);
  if (actual !== offer.sha256) throw new Error(`Node Agent update SHA-256 mismatch: expected ${offer.sha256}, got ${actual}`);
  return offer;
}

function assertHttpsUrl(value: string, label: string): void {
  let parsed: URL;
  try { parsed = new URL(value); } catch { throw new Error(`${label} is invalid`); }
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password) throw new Error(`${label} must use HTTPS without embedded credentials`);
}

export async function downloadVerifiedNodeUpdateArchive(
  offerValue: unknown,
  destination: string,
  options: NodeUpdateRuntimeOptions = {}
): Promise<NodeUpdateOffer> {
  const offer = verifyNodeUpdateOfferSignature(offerValue, options.trustedKeyRaw);
  const response = await (options.fetchImpl ?? fetch)(offer.url, { redirect: 'follow' });
  if (!response.ok) throw new Error(`Node Agent update download failed with HTTP ${response.status}`);
  assertHttpsUrl(response.url || offer.url, 'final update URL');
  const declaredLength = Number(response.headers.get('content-length') ?? '0');
  if (Number.isFinite(declaredLength) && declaredLength > MAX_NODE_UPDATE_BYTES) throw new Error('Node Agent update exceeds the maximum archive size');
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > MAX_NODE_UPDATE_BYTES) throw new Error('Node Agent update exceeds the maximum archive size');
  verifyNodeUpdateArtifact(offer, bytes, options.trustedKeyRaw);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, bytes);
  return offer;
}

export function assertSafeArchiveEntries(entries: readonly string[]): void {
  if (!entries.length) throw new Error('Node Agent update archive is empty');
  for (const raw of entries) {
    const entry = raw.trim().replaceAll('\\', '/');
    if (!entry) continue;
    if (entry.startsWith('/') || /^[A-Za-z]:\//.test(entry)) throw new Error(`update archive contains an absolute path: ${entry}`);
    const parts = entry.split('/').filter(Boolean);
    if (parts.some(part => part === '..')) throw new Error(`update archive contains path traversal: ${entry}`);
  }
}

async function runCommand(program: string, args: readonly string[]): Promise<CommandResult> {
  return await new Promise((resolve, reject) => {
    const child = spawn(program, [...args], { windowsHide: true });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', chunk => { stdout += Buffer.from(chunk).toString('utf8'); });
    child.stderr.on('data', chunk => { stderr += Buffer.from(chunk).toString('utf8'); });
    child.once('error', reject);
    child.once('close', code => resolve({ code: code ?? 1, stdout, stderr }));
  });
}

function safePackagePath(root: string, relative: string): string {
  const normalized = relative.replaceAll('\\', '/');
  if (!normalized || normalized.startsWith('/') || /^[A-Za-z]:\//.test(normalized) || normalized.split('/').includes('..')) {
    throw new Error(`portable checksum contains unsafe path: ${relative}`);
  }
  const resolvedRoot = path.resolve(root);
  const resolved = path.resolve(resolvedRoot, ...normalized.split('/'));
  const prefix = `${resolvedRoot}${path.sep}`;
  if (resolved !== resolvedRoot && !resolved.toLowerCase().startsWith(prefix.toLowerCase())) throw new Error(`portable checksum path escaped package root: ${relative}`);
  return resolved;
}

export async function verifyPortableNodePackage(packageRoot: string, expectedVersion: string): Promise<{ gitCommit: string }> {
  const manifestPath = path.join(packageRoot, 'portable-manifest.json');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as Record<string, unknown>;
  if (manifest.nodeAgentVersion !== expectedVersion) {
    throw new Error(`portable manifest version mismatch: expected ${expectedVersion}, got ${String(manifest.nodeAgentVersion ?? '')}`);
  }
  const gitCommit = requireString(manifest.gitCommit, 'manifest gitCommit', 128);
  if (!/^[0-9a-fA-F]{40,64}$/.test(gitCommit)) throw new Error('portable manifest gitCommit is invalid');
  const checksumText = await readFile(path.join(packageRoot, 'SHA256SUMS.txt'), 'utf8');
  const entries = new Map<string, string>();
  for (const line of checksumText.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = /^([0-9a-fA-F]{64})  (.+)$/.exec(line);
    if (!match) throw new Error(`portable checksum line is malformed: ${line}`);
    const relative = match[2]!.replaceAll('\\', '/');
    if (entries.has(relative)) throw new Error(`portable checksum contains duplicate path: ${relative}`);
    entries.set(relative, match[1]!.toLowerCase());
  }
  const critical = ['start-node-agent.bat', 'portable-manifest.json', 'app/dist/cli.js', 'app/dist/server.js', 'app/dist/ctmcp-protect.exe', 'update-handoff.ps1'];
  if (manifest.nodeRuntimeBundled === true) critical.push('runtime/node.exe');
  for (const relative of critical) {
    const expected = entries.get(relative);
    if (!expected) throw new Error(`portable checksum is missing critical file: ${relative}`);
    const actual = sha256Hex(await readFile(safePackagePath(packageRoot, relative)));
    if (actual !== expected) throw new Error(`portable critical file checksum mismatch: ${relative}`);
  }
  return { gitCommit };
}

export function defaultCurrentPortableRoot(): string {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
}

export async function prepareNodeUpdatePackage(
  offerValue: unknown,
  dataDir: string,
  options: NodeUpdateRuntimeOptions = {}
): Promise<PreparedNodeUpdate> {
  if (process.platform !== 'win32' && !options.commandRunner) throw new Error('Node Agent live update currently supports Windows portable packages only');
  const offer = verifyNodeUpdateOfferSignature(offerValue, options.trustedKeyRaw);
  const stageRoot = path.join(dataDir, 'updates', offer.update_id);
  const archivePath = path.join(stageRoot, 'update.zip');
  const packageRoot = path.join(stageRoot, 'package');
  await rm(stageRoot, { recursive: true, force: true });
  await mkdir(stageRoot, { recursive: true });
  await downloadVerifiedNodeUpdateArchive(offer, archivePath, options);
  const runner = options.commandRunner ?? runCommand;
  const listed = await runner('tar.exe', ['-tf', archivePath]);
  if (listed.code !== 0) throw new Error(`unable to inspect Node Agent update archive: ${listed.stderr.trim() || listed.stdout.trim()}`);
  assertSafeArchiveEntries(listed.stdout.split(/\r?\n/).filter(Boolean));
  await mkdir(packageRoot, { recursive: true });
  const extracted = await runner('tar.exe', ['-xf', archivePath, '-C', packageRoot]);
  if (extracted.code !== 0) throw new Error(`unable to extract Node Agent update archive: ${extracted.stderr.trim() || extracted.stdout.trim()}`);
  const { gitCommit } = await verifyPortableNodePackage(packageRoot, offer.version);
  return { offer, archivePath, packageRoot, gitCommit };
}

export async function scheduleNodeUpdateHandoff(
  prepared: PreparedNodeUpdate,
  dataDir: string,
  options: NodeUpdateRuntimeOptions = {}
): Promise<void> {
  if (process.platform !== 'win32' && !options.commandRunner) throw new Error('Node Agent live update handoff currently supports Windows only');
  const currentRoot = path.resolve(options.currentPackageRoot ?? defaultCurrentPortableRoot());
  const handoffScript = path.join(currentRoot, 'update-handoff.ps1');
  await readFile(handoffScript);
  const runner = options.commandRunner ?? runCommand;
  const result = await runner('powershell.exe', [
    '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', handoffScript,
    '-PackageRoot', prepared.packageRoot,
    '-FallbackPackageRoot', currentRoot,
    '-DataDir', dataDir,
    '-DelaySeconds', '30'
  ]);
  if (result.code !== 0) throw new Error(`Node Agent update handoff could not be scheduled: ${result.stderr.trim() || result.stdout.trim()}`);
  let payload: Record<string, unknown>;
  try { payload = JSON.parse(result.stdout) as Record<string, unknown>; } catch { throw new Error('Node Agent update handoff returned invalid JSON'); }
  if (payload.ok !== true || payload.handoffScheduled !== true) throw new Error('Node Agent update handoff was not accepted');
}
