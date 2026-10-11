import { execFileSync, spawn } from 'node:child_process';
import { copyFile, existsSync, mkdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ResolvedCommandSpec } from './policy.js';
import type { SandboxConfig, SandboxPathAccess, SandboxPathGrant } from './types.js';
import type { SandboxLaunch } from './sandbox.js';
import { wslcProvisionerCompilerCandidates } from './sandboxWslcProvisioner.js';

export const APPCONTAINER_BACKEND_ID = 'appcontainer';
export const APPCONTAINER_NETWORK_OPTION_ID = 'appcontainer.network';
export const APPCONTAINER_DEFAULT_NETWORK = 'none';

export function appContainerHostAvailable(): boolean {
  return process.platform === 'win32';
}

export interface AppContainerPreparedSandbox {
  helper: string;
  workspaceRoot: string;
  stateRoot: string;
  externalPaths: SandboxPathGrant[];
  environment: Array<[string, string]>;
  network: 'none' | 'internet';
  leaseRoot: string;
}

interface ProcessResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

interface AppContainerLeaseJournal {
  schemaVersion: 1;
  ownerPid: number;
  helperPid?: number;
  createdAtMs: number;
  profileName: string;
  workspaceRoot: string;
  stateRoot: string;
  externalPaths: SandboxPathGrant[];
}

export class AppContainerSandboxError extends Error {
  readonly code: string;
  readonly category: 'security' | 'runtime';
  readonly retryable = false;
  readonly details: Record<string, unknown>;

  constructor(code: string, message: string, stage: string, details: Record<string, unknown> = {}) {
    super(message);
    this.name = 'AppContainerSandboxError';
    this.code = code;
    this.category = code.includes('PATH') || code.includes('COMMAND') || code.includes('ACL') || code.includes('INTEGRITY') ? 'security' : 'runtime';
    this.details = {
      sandbox_backend: APPCONTAINER_BACKEND_ID,
      stage,
      fallback_allowed: false,
      ...details
    };
  }
}

function comparablePath(value: string): string {
  return path.resolve(value).replaceAll('\\', '/').replace(/\/+$/, '').toLowerCase();
}

function isNetworkPath(value: string): boolean {
  const normalized = value.replaceAll('/', '\\');
  return normalized.startsWith('\\\\') || /^\\\\\?\\UNC\\/i.test(normalized);
}

function appContainerStateRoot(dataDir: string, workspaceRoot: string): string {
  const key = createHash('sha256').update(comparablePath(workspaceRoot)).digest('hex').slice(0, 24);
  return path.join(path.resolve(dataDir), 'sandbox', 'appcontainer', key, 'state');
}

function appContainerLeaseRoot(dataDir: string): string {
  return path.join(path.resolve(dataDir), 'sandbox', 'appcontainer', 'leases');
}

let packagedHelperVerification: Promise<string | undefined> | undefined;

async function packagedHelperPath(): Promise<string | undefined> {
  const candidate = fileURLToPath(new URL(`./appcontainer-helper-${APPCONTAINER_HELPER_SOURCE_HASH}.exe`, import.meta.url));
  if (!existsSync(candidate)) return undefined;
  if (packagedHelperVerification) return packagedHelperVerification;
  const digestFile = `${candidate}.sha256`;
  const current = (async () => {
    let expectedDigest: string;
    try {
      expectedDigest = (await readFile(digestFile, 'utf8')).trim().toLowerCase();
    } catch (error) {
      throw new AppContainerSandboxError(
        'SANDBOX_APPCONTAINER_HELPER_INTEGRITY_FAILED',
        `Packaged AppContainer helper digest is unavailable: ${error instanceof Error ? error.message : String(error)}`,
        'helper_integrity'
      );
    }
    if (!/^[a-f0-9]{64}$/.test(expectedDigest)) {
      throw new AppContainerSandboxError(
        'SANDBOX_APPCONTAINER_HELPER_INTEGRITY_FAILED',
        'Packaged AppContainer helper digest manifest is invalid.',
        'helper_integrity'
      );
    }
    let actualDigest: string;
    try {
      actualDigest = createHash('sha256').update(await readFile(candidate)).digest('hex');
    } catch (error) {
      throw new AppContainerSandboxError(
        'SANDBOX_APPCONTAINER_HELPER_INTEGRITY_FAILED',
        `Packaged AppContainer helper cannot be hashed: ${error instanceof Error ? error.message : String(error)}`,
        'helper_integrity'
      );
    }
    if (actualDigest !== expectedDigest) {
      throw new AppContainerSandboxError(
        'SANDBOX_APPCONTAINER_HELPER_INTEGRITY_FAILED',
        'Packaged AppContainer helper digest does not match its build manifest.',
        'helper_integrity',
        { expected_sha256: expectedDigest, actual_sha256: actualDigest }
      );
    }
    return candidate;
  })();
  packagedHelperVerification = current;
  try {
    return await current;
  } catch (error) {
    packagedHelperVerification = undefined;
    throw error;
  }
}

async function existingPath(value: string, label: string): Promise<string> {
  if (!path.isAbsolute(value) || isNetworkPath(value)) {
    throw new AppContainerSandboxError('SANDBOX_EXTERNAL_PATH_UNSUPPORTED', `${label} must be an absolute local path: ${value}`, 'mounts');
  }
  try {
    return await realpath(value);
  } catch (error) {
    throw new AppContainerSandboxError(
      'SANDBOX_EXTERNAL_PATH_INVALID',
      `${label} does not exist or cannot be resolved: ${value}: ${error instanceof Error ? error.message : String(error)}`,
      'mounts'
    );
  }
}

async function canonicalExternalPaths(grants: SandboxPathGrant[]): Promise<SandboxPathGrant[]> {
  const merged = new Map<string, SandboxPathGrant>();
  for (const grant of grants) {
    const raw = grant.path.trim();
    if (!raw) throw new AppContainerSandboxError('SANDBOX_EXTERNAL_PATH_INVALID', 'External sandbox path cannot be empty.', 'mounts');
    const resolved = await existingPath(raw, 'External sandbox path');
    const key = comparablePath(resolved);
    const current = merged.get(key);
    merged.set(key, {
      path: resolved,
      access: current?.access === 'modify' || grant.access === 'modify' ? 'modify' : 'read_only'
    });
  }
  return [...merged.values()].sort((left, right) => comparablePath(left.path).localeCompare(comparablePath(right.path)));
}

function pathWithin(parent: string, candidate: string): boolean {
  const root = comparablePath(parent);
  const value = comparablePath(candidate);
  return value === root || value.startsWith(`${root}/`);
}

async function rejectProtectedExternalPaths(workspace: string, grants: SandboxPathGrant[]): Promise<void> {
  for (const relative of ['.git', '.github']) {
    const requested = path.join(workspace, relative);
    let protectedPath: string;
    try {
      protectedPath = await realpath(requested);
    } catch {
      continue;
    }
    const grant = grants.find(candidate => pathWithin(protectedPath, candidate.path));
    if (grant) {
      throw new AppContainerSandboxError(
        'SANDBOX_EXTERNAL_PATH_PROTECTED',
        `External sandbox path cannot target protected repository metadata: ${grant.path}`,
        'mounts'
      );
    }
  }
}

async function runProcess(program: string, args: string[], timeoutMs: number): Promise<ProcessResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args, { windowsHide: true, shell: false, stdio: ['ignore', 'pipe', 'pipe'] });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      try { child.kill('SIGKILL'); } catch { /* best effort */ }
      reject(new AppContainerSandboxError('SANDBOX_APPCONTAINER_HELPER_TIMEOUT', `AppContainer helper exceeded ${timeoutMs} ms.`, 'helper'));
    }, Math.max(1, timeoutMs));
    timer.unref();
    child.stdout.on('data', chunk => stdout.push(Buffer.from(chunk)));
    child.stderr.on('data', chunk => stderr.push(Buffer.from(chunk)));
    child.once('error', error => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(new AppContainerSandboxError('SANDBOX_APPCONTAINER_HELPER_FAILED', `Failed to start AppContainer helper: ${error.message}`, 'helper'));
    });
    child.once('close', code => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ code, stdout: Buffer.concat(stdout).toString('utf8'), stderr: Buffer.concat(stderr).toString('utf8') });
    });
  });
}

async function discoverCompiler(): Promise<string> {
  for (const candidate of wslcProvisionerCompilerCandidates()) {
    if (existsSync(candidate)) return candidate;
  }
  throw new AppContainerSandboxError(
    'SANDBOX_APPCONTAINER_HELPER_UNAVAILABLE',
    'Windows .NET Framework csc.exe is required to prepare the AppContainer launch helper.',
    'helper_compile'
  );
}

const helperBuilds = new Map<string, Promise<string>>();
const LEASE_SCAVENGE_INTERVAL_MS = 60_000;
const leaseScavenges = new Map<string, { completedAt?: number; promise?: Promise<void> }>();

function processAlive(pid: number | undefined): boolean {
  if (typeof pid !== 'number' || !Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    const code = (error as NodeJS.ErrnoException)?.code;
    return code === 'EPERM' || code === 'EACCES';
  }
}

function leaseDirectoryPath(prepared: AppContainerPreparedSandbox, profileName: string): string {
  return path.join(prepared.leaseRoot, profileName);
}

function leaseJournalPath(prepared: AppContainerPreparedSandbox, profileName: string): string {
  return path.join(leaseDirectoryPath(prepared, profileName), 'lease.json');
}

function leaseCompletionMarkerPath(prepared: AppContainerPreparedSandbox, profileName: string): string {
  return path.join(leaseDirectoryPath(prepared, profileName), 'complete');
}

function leaseProtectedStatePath(prepared: AppContainerPreparedSandbox, profileName: string): string {
  return path.join(leaseDirectoryPath(prepared, profileName), 'cleanup.state');
}

function writeLeaseJournal(filePath: string, journal: AppContainerLeaseJournal): void {
  mkdirSync(path.dirname(filePath), { recursive: true });
  const temporary = `${filePath}.${process.pid}.${randomUUID()}.tmp`;
  try {
    writeFileSync(temporary, `${JSON.stringify(journal)}\n`, { encoding: 'utf8', mode: 0o600 });
    renameSync(temporary, filePath);
  } catch (error) {
    try { rmSync(temporary, { force: true }); } catch { /* best-effort temp cleanup */ }
    throw error;
  }
}

function parseLeaseJournal(value: unknown): AppContainerLeaseJournal | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const journal = value as Partial<AppContainerLeaseJournal>;
  if (journal.schemaVersion !== 1
      || typeof journal.ownerPid !== 'number' || !Number.isSafeInteger(journal.ownerPid) || journal.ownerPid <= 0
      || typeof journal.createdAtMs !== 'number'
      || typeof journal.profileName !== 'string' || !/^CodingToolsMcp\.Node\.[A-Za-z0-9.]+$/.test(journal.profileName)
      || typeof journal.workspaceRoot !== 'string' || !path.isAbsolute(journal.workspaceRoot) || isNetworkPath(journal.workspaceRoot)
      || typeof journal.stateRoot !== 'string' || !path.isAbsolute(journal.stateRoot) || isNetworkPath(journal.stateRoot)
      || !Array.isArray(journal.externalPaths) || journal.externalPaths.length > 64) return undefined;
  const externalPaths: SandboxPathGrant[] = [];
  for (const grant of journal.externalPaths) {
    if (!grant || typeof grant !== 'object') return undefined;
    const candidate = grant as Partial<SandboxPathGrant>;
    if (typeof candidate.path !== 'string' || !path.isAbsolute(candidate.path) || isNetworkPath(candidate.path)) return undefined;
    if (candidate.access !== 'read_only' && candidate.access !== 'modify') return undefined;
    externalPaths.push({ path: candidate.path, access: candidate.access });
  }
  return {
    schemaVersion: 1,
    ownerPid: journal.ownerPid,
    helperPid: typeof journal.helperPid === 'number' && Number.isSafeInteger(journal.helperPid) && journal.helperPid > 0 ? journal.helperPid : undefined,
    createdAtMs: journal.createdAtMs,
    profileName: journal.profileName,
    workspaceRoot: journal.workspaceRoot,
    stateRoot: journal.stateRoot,
    externalPaths
  };
}

async function scavengeStaleAppContainerLeases(prepared: AppContainerPreparedSandbox): Promise<void> {
  await mkdir(prepared.leaseRoot, { recursive: true, mode: 0o700 });
  let entries: Array<{ isFile(): boolean; isDirectory(): boolean; name: string }>;
  try {
    entries = await readdir(prepared.leaseRoot, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries.filter(value => value.isDirectory() && /^CodingToolsMcp\.Node\.[A-Za-z0-9.]+$/.test(value.name)).slice(0, 128)) {
    const leaseDirectory = path.join(prepared.leaseRoot, entry.name);
    const filePath = path.join(leaseDirectory, 'lease.json');
    let journal: AppContainerLeaseJournal | undefined;
    try {
      const metadata = await stat(filePath);
      if (!metadata.isFile() || metadata.size > 64 * 1024) continue;
      journal = parseLeaseJournal(JSON.parse(await readFile(filePath, 'utf8')));
    } catch {
      continue;
    }
    if (!journal || journal.profileName !== entry.name || processAlive(journal.ownerPid)) continue;
    const stalePrepared: AppContainerPreparedSandbox = {
      ...prepared,
      workspaceRoot: journal.workspaceRoot,
      stateRoot: journal.stateRoot,
      externalPaths: journal.externalPaths
    };
    const completionMarker = leaseCompletionMarkerPath(stalePrepared, journal.profileName);
    try {
      if (existsSync(completionMarker)) {
        await rm(leaseDirectory, { recursive: true, force: true });
        continue;
      }
      // Without a completion marker, only cleanup after a recorded helper PID is
      // also gone. Missing helper PID fails safe because a helper may still be live.
      if (!journal.helperPid || processAlive(journal.helperPid)) continue;
      await cleanupAppContainer(stalePrepared, journal.profileName);
      await rm(leaseDirectory, { recursive: true, force: true });
    } catch {
      // Keep the journal and cleanup-state file so a later healthy runtime can retry cleanup.
    }
  }

  // Compatibility with the flat lease journal used by older Node Agent builds.
  // Those journals did not carry exact protected-DACL/runtime-grant state, so this is
  // necessarily best-effort; still, do not abandon profiles that can be safely retried.
  for (const entry of entries.filter(value => value.isFile() && value.name.endsWith('.json')).slice(0, 128)) {
    const filePath = path.join(prepared.leaseRoot, entry.name);
    let journal: AppContainerLeaseJournal | undefined;
    try {
      const metadata = await stat(filePath);
      if (!metadata.isFile() || metadata.size > 64 * 1024) continue;
      journal = parseLeaseJournal(JSON.parse(await readFile(filePath, 'utf8')));
    } catch {
      continue;
    }
    if (!journal
        || entry.name !== `${journal.profileName}.json`
        || processAlive(journal.ownerPid)
        || !journal.helperPid
        || processAlive(journal.helperPid)) continue;
    const stalePrepared: AppContainerPreparedSandbox = {
      ...prepared,
      workspaceRoot: journal.workspaceRoot,
      stateRoot: journal.stateRoot,
      externalPaths: journal.externalPaths
    };
    try {
      await cleanupAppContainer(stalePrepared, journal.profileName);
      await rm(filePath, { force: true });
    } catch {
      // Retain the legacy journal for another healthy runtime to retry.
    }
  }
}

async function ensureLeaseScavenged(prepared: AppContainerPreparedSandbox): Promise<void> {
  const key = comparablePath(prepared.leaseRoot);
  const state = leaseScavenges.get(key) ?? {};
  leaseScavenges.set(key, state);
  if (state.promise) {
    await state.promise;
    return;
  }
  if (state.completedAt !== undefined && Date.now() - state.completedAt < LEASE_SCAVENGE_INTERVAL_MS) return;
  const current = scavengeStaleAppContainerLeases(prepared);
  state.promise = current;
  try {
    await current;
    state.completedAt = Date.now();
  } finally {
    state.promise = undefined;
  }
}

async function ensureHelper(dataDir: string): Promise<string> {
  const packaged = await packagedHelperPath();
  if (packaged) return packaged;
  const root = path.join(path.resolve(dataDir), 'sandbox', 'appcontainer');
  const target = path.join(root, `helper-${APPCONTAINER_HELPER_SOURCE_HASH}.exe`);
  if (existsSync(target)) return target;
  const key = target.toLowerCase();
  const previous = helperBuilds.get(key);
  if (previous) return previous;
  const current = (async () => {
    if (existsSync(target)) return target;
    await mkdir(root, { recursive: true, mode: 0o700 });
    const temporary = await mkdtemp(path.join(tmpdir(), 'ctmcp-appcontainer-'));
    const sourcePath = path.join(temporary, 'AppContainerHelper.cs');
    const compiledPath = path.join(temporary, 'AppContainerHelper.exe');
    try {
      await writeFile(sourcePath, APPCONTAINER_HELPER_SOURCE, { encoding: 'utf8', mode: 0o600 });
      const compiler = await discoverCompiler();
      const compiled = await runProcess(compiler, [
        '/nologo',
        '/optimize+',
        '/target:exe',
        '/platform:anycpu',
        '/r:System.dll',
        '/r:System.Core.dll',
        '/r:System.Security.dll',
        `/out:${compiledPath}`,
        sourcePath
      ], 30_000);
      if (compiled.code !== 0 || !existsSync(compiledPath)) {
        throw new AppContainerSandboxError(
          'SANDBOX_APPCONTAINER_HELPER_COMPILE_FAILED',
          'Failed to compile the fixed AppContainer launch helper.',
          'helper_compile',
          { exit_code: compiled.code, stdout: compiled.stdout, stderr: compiled.stderr }
        );
      }
      await new Promise<void>((resolve, reject) => copyFile(compiledPath, target, error => error ? reject(error) : resolve()));
      return target;
    } finally {
      await rm(temporary, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 }).catch(() => undefined);
    }
  })();
  helperBuilds.set(key, current);
  try {
    return await current;
  } finally {
    helperBuilds.delete(key);
  }
}

async function stateEnvironment(root: string): Promise<Array<[string, string]>> {
  const home = path.join(root, 'home');
  const temp = path.join(root, 'tmp');
  const cache = path.join(root, 'cache');
  const cargoHome = path.join(root, 'cargo-home');
  const cargoTarget = path.join(root, 'cargo-target');
  const npmCache = path.join(root, 'npm-cache');
  const npmPrefix = path.join(root, 'npm-prefix');
  const pycache = path.join(root, 'pycache');
  const appData = path.join(home, 'AppData', 'Roaming');
  const localAppData = path.join(home, 'AppData', 'Local');
  await Promise.all([home, temp, cache, cargoHome, cargoTarget, npmCache, npmPrefix, pycache, appData, localAppData].map(value => mkdir(value, { recursive: true, mode: 0o700 })));
  return [
    ['TEMP', temp],
    ['TMP', temp],
    ['TMPDIR', temp],
    ['HOME', home],
    ['USERPROFILE', home],
    ['APPDATA', appData],
    ['LOCALAPPDATA', localAppData],
    ['XDG_CACHE_HOME', cache],
    ['CARGO_HOME', cargoHome],
    ['CARGO_TARGET_DIR', cargoTarget],
    ['NPM_CONFIG_CACHE', npmCache],
    ['NPM_CONFIG_PREFIX', npmPrefix],
    ['PYTHONPYCACHEPREFIX', pycache]
  ];
}

export async function prepareAppContainer(config: SandboxConfig, workspaceRoot: string, dataDir: string): Promise<AppContainerPreparedSandbox> {
  if (process.platform !== 'win32') throw new AppContainerSandboxError('SANDBOX_BACKEND_UNSUPPORTED', 'Windows AppContainer is only supported on Windows hosts.', 'prepare');
  const workspace = await existingPath(workspaceRoot, 'Workspace root');
  const workspaceStat = await stat(workspace);
  if (!workspaceStat.isDirectory()) throw new AppContainerSandboxError('SANDBOX_STATE_PREPARE_FAILED', `Workspace root must be a directory: ${workspace}`, 'prepare');
  const stateRoot = appContainerStateRoot(dataDir, workspace);
  await mkdir(stateRoot, { recursive: true, mode: 0o700 });
  const externalPaths = await canonicalExternalPaths(config.externalPaths);
  await rejectProtectedExternalPaths(workspace, externalPaths);
  const environment = await stateEnvironment(stateRoot);
  const helper = await ensureHelper(dataDir);
  const network = selectedNetwork(config.options);
  const prepared = {
    helper,
    workspaceRoot: workspace,
    stateRoot,
    externalPaths,
    environment,
    network,
    leaseRoot: appContainerLeaseRoot(dataDir)
  } satisfies AppContainerPreparedSandbox;
  await ensureLeaseScavenged(prepared);
  return prepared;
}

function existingRegularFile(value: string): boolean {
  try {
    return statSync(value).isFile();
  } catch {
    return false;
  }
}

export function resolveAppContainerPathProgram(
  program: string,
  searchPath = process.env.PATH ?? '',
  pathExt = process.env.PATHEXT ?? '.COM;.EXE;.BAT;.CMD',
  cwd = process.cwd()
): string | undefined {
  if (path.isAbsolute(program)) return existingRegularFile(program) ? path.normalize(program) : undefined;
  const extensions = path.extname(program)
    ? ['']
    : ['', ...pathExt.split(';').map(value => value.trim()).filter(Boolean)];
  const directories = [cwd, ...searchPath.split(path.delimiter).map(value => value.trim().replace(/^"|"$/g, ''))];
  for (const directory of directories) {
    const base = directory || cwd;
    for (const extension of extensions) {
      const candidate = path.resolve(base, `${program}${extension}`);
      if (existingRegularFile(candidate)) return candidate;
    }
  }
  return undefined;
}

function findOnPath(program: string): string | undefined {
  return resolveAppContainerPathProgram(program);
}

function selectedNetwork(options: Record<string, string> | undefined): 'none' | 'internet' {
  const raw = String(options?.[APPCONTAINER_NETWORK_OPTION_ID] ?? APPCONTAINER_DEFAULT_NETWORK).trim().toLowerCase();
  if (!raw || raw === 'none' || raw === 'deny' || raw === 'false' || raw === 'off') return 'none';
  if (raw === 'internet' || raw === 'allow' || raw === 'true' || raw === 'on') return 'internet';
  throw new AppContainerSandboxError(
    'SANDBOX_APPCONTAINER_NETWORK_INVALID',
    `AppContainer network option must be 'none' or 'internet', got '${raw}'.`,
    'prepare'
  );
}

function windowsSystemCmd(): string {
  return path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'cmd.exe');
}

function detectedPowershell(): string {
  return findOnPath('pwsh.exe')
    ?? findOnPath('powershell.exe')
    ?? path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
}

function windowsBatchToken(value: string): string {
  return `"${value.replaceAll('"', '""')}"`;
}

function powershellLiteral(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function rustupProgram(program: string): string | undefined {
  const name = path.basename(program).toLowerCase();
  if (name !== 'cargo' && name !== 'cargo.exe' && name !== 'rustc' && name !== 'rustc.exe') return undefined;
  const tool = name.startsWith('cargo') ? 'cargo' : 'rustc';
  try {
    const output = execFileSync('rustup.exe', ['which', tool], { encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    return output || undefined;
  } catch {
    return undefined;
  }
}

export interface AppContainerLaunchSpec {
  program: string;
  argv: string[];
  display: string;
  rawArg?: string;
}

export function normalizeAppContainerSpec(spec: ResolvedCommandSpec): AppContainerLaunchSpec {
  const name = path.basename(spec.program).toLowerCase();
  if (name === 'npm' || name === 'npm.cmd') {
    const node = findOnPath('node.exe') ?? findOnPath('node');
    if (!node) throw new AppContainerSandboxError('SANDBOX_RUNTIME_NOT_FOUND', 'Node runtime was not found for npm AppContainer normalization.', 'runtime');
    const nodeRoot = path.dirname(node);
    const npmCli = path.join(nodeRoot, 'node_modules', 'npm', 'bin', 'npm-cli.js');
    if (!existsSync(npmCli)) throw new AppContainerSandboxError('SANDBOX_RUNTIME_NOT_FOUND', `npm runtime path not found: ${npmCli}`, 'runtime');
    return {
      program: path.join(nodeRoot, 'node.exe'),
      argv: ['--preserve-symlinks', '--preserve-symlinks-main', npmCli, ...spec.argv],
      display: spec.display
    };
  }
  const rustup = rustupProgram(spec.program);
  if (rustup) return { program: rustup, argv: spec.argv, display: spec.display };
  if (name.endsWith('.cmd') || name.endsWith('.bat')) {
    const programPath = spec.program.replace(/^\\\\\?\\/i, '');
    return {
      program: windowsSystemCmd(),
      argv: ['/d', '/s', '/c'],
      display: spec.display,
      rawArg: `call ${windowsBatchToken(programPath)}${spec.argv.map(value => ` ${windowsBatchToken(value)}`).join('')}`
    };
  }
  if (name.endsWith('.ps1')) {
    const programPath = spec.program.replace(/^\\\\\?\\/i, '');
    let invocation = `& ${powershellLiteral(programPath)}`;
    for (const argument of spec.argv) invocation += ` ${powershellLiteral(argument)}`;
    return {
      program: detectedPowershell(),
      argv: ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', invocation],
      display: spec.display
    };
  }
  return { program: spec.program, argv: spec.argv, display: spec.display };
}

function helperArguments(
  mode: 'run' | 'cleanup',
  prepared: AppContainerPreparedSandbox,
  profileName: string,
  program?: string,
  cwd?: string,
  args: string[] = [],
  rawArg?: string,
  completionMarker?: string
): string[] {
  const values = mode === 'run'
    ? ['--run', '--profile', profileName, '--program', program ?? '', '--cwd', cwd ?? '', '--network', prepared.network]
    : ['--cleanup', '--profile', profileName];
  values.push(
    '--workspace', prepared.workspaceRoot,
    '--state', prepared.stateRoot,
    '--protected-state', leaseProtectedStatePath(prepared, profileName)
  );
  for (const grant of prepared.externalPaths) values.push('--external', grant.access, grant.path);
  if (mode === 'run') {
    if (completionMarker) values.push('--completion-marker', completionMarker);
    for (const [key, value] of prepared.environment) values.push('--sandbox-env', `${key}=${value}`);
    if (rawArg) values.push('--raw-arg', rawArg);
    values.push('--', ...args);
  }
  return values;
}

async function cleanupAppContainer(prepared: AppContainerPreparedSandbox, profileName: string): Promise<void> {
  const result = await runProcess(prepared.helper, helperArguments('cleanup', prepared, profileName), 30_000);
  if (result.code !== 0) {
    throw new AppContainerSandboxError(
      'SANDBOX_APPCONTAINER_CLEANUP_FAILED',
      'AppContainer helper failed to clean up its profile or ACL grants.',
      'cleanup',
      { exit_code: result.code, stdout: result.stdout, stderr: result.stderr }
    );
  }
}

export function prepareAppContainerLaunch(
  prepared: AppContainerPreparedSandbox,
  cwd: string,
  spec: ResolvedCommandSpec,
  environment: Array<[string, string]>,
  removeEnvironment: string[]
): SandboxLaunch {
  const normalized = normalizeAppContainerSpec(spec);
  const profileName = `CodingToolsMcp.Node.${process.pid}.${randomUUID().replaceAll('-', '')}`;
  const journalPath = leaseJournalPath(prepared, profileName);
  const completionMarker = leaseCompletionMarkerPath(prepared, profileName);
  const args = helperArguments(
    'run',
    prepared,
    profileName,
    normalized.program,
    cwd,
    normalized.argv,
    normalized.rawArg,
    completionMarker
  );
  const journal: AppContainerLeaseJournal = {
    schemaVersion: 1,
    ownerPid: process.pid,
    createdAtMs: Date.now(),
    profileName,
    workspaceRoot: prepared.workspaceRoot,
    stateRoot: prepared.stateRoot,
    externalPaths: prepared.externalPaths.map(grant => ({ ...grant }))
  };
  writeLeaseJournal(journalPath, journal);
  let cleanupPromise: Promise<void> | undefined;
  return {
    program: prepared.helper,
    args,
    environmentMode: 'command',
    onSpawn: pid => {
      if (typeof pid !== 'number' || !Number.isSafeInteger(pid) || pid <= 0) return;
      journal.helperPid = pid;
      try { writeLeaseJournal(journalPath, journal); } catch { /* missing helper PID makes the scavenger retain this lease */ }
    },
    // The launched helper owns a kill-on-close Job Object. Session cancellation
    // taskkills the helper, which closes the job and terminates descendants.
    kill: async () => undefined,
    cleanup: () => {
      if (cleanupPromise) return cleanupPromise;
      cleanupPromise = (async () => {
        const selfCleaned = existsSync(completionMarker);
        if (!selfCleaned) await cleanupAppContainer(prepared, profileName);
        await rm(leaseDirectoryPath(prepared, profileName), { recursive: true, force: true });
      })();
      return cleanupPromise;
    },
    processTreeContained: true,
    processTreeControl: 'appcontainer_job'
  };
}

export const APPCONTAINER_HELPER_SOURCE = String.raw`
using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;
using System.Threading.Tasks;

public static class Program {
    private const uint CREATE_NO_WINDOW = 0x08000000;
    private const uint CREATE_SUSPENDED = 0x00000004;
    private const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    private const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    private const uint HANDLE_FLAG_INHERIT = 1;
    private const int PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES = 0x00020009;
    private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
    private const uint INFINITE = 0xffffffff;
    private const uint WAIT_FAILED = 0xffffffff;
    private const uint TOKEN_QUERY = 0x0008;
    private const int TOKEN_IS_APPCONTAINER = 29;
    private const int TOKEN_APPCONTAINER_SID = 31;

    private sealed class Grant {
        public string Path;
        public string Access;
        public Grant(string path, string access) { Path = path; Access = access; }
    }

    private sealed class Options {
        public bool Cleanup;
        public string Profile;
        public string Program;
        public string Cwd;
        public string Workspace;
        public string State;
        public string CompletionMarker;
        public string ProtectedState;
        public string Network = "none";
        public string RawArg;
        public readonly List<Grant> External = new List<Grant>();
        public readonly Dictionary<string, string> SandboxEnvironment = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        public readonly List<string> Arguments = new List<string>();
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct SecurityAttributes { public int Length; public IntPtr Descriptor; public int InheritHandle; }

    [StructLayout(LayoutKind.Sequential)]
    private struct StartupInfo { public int Cb; public IntPtr Reserved; public IntPtr Desktop; public IntPtr Title; public int X; public int Y; public int XSize; public int YSize; public int XCountChars; public int YCountChars; public int Fill; public int Flags; public short ShowWindow; public short Reserved2; public IntPtr Reserved2Ptr; public IntPtr StdInput; public IntPtr StdOutput; public IntPtr StdError; }

    [StructLayout(LayoutKind.Sequential)]
    private struct StartupInfoEx { public StartupInfo StartupInfo; public IntPtr AttributeList; }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInformation { public IntPtr Process; public IntPtr Thread; public int ProcessId; public int ThreadId; }

    [StructLayout(LayoutKind.Sequential)]
    private struct SidAndAttributes { public IntPtr Sid; public uint Attributes; }

    [StructLayout(LayoutKind.Sequential)]
    private struct SecurityCapabilities { public IntPtr AppContainerSid; public IntPtr Capabilities; public uint CapabilityCount; public uint Reserved; }

    [StructLayout(LayoutKind.Sequential)]
    private struct TokenAppContainerInformation { public IntPtr TokenAppContainer; }

    [StructLayout(LayoutKind.Sequential)]
    private struct JobBasicLimitInformation { public long ProcessUserTime; public long JobUserTime; public uint LimitFlags; public UIntPtr MinimumWorkingSet; public UIntPtr MaximumWorkingSet; public uint ActiveProcessLimit; public UIntPtr Affinity; public uint PriorityClass; public uint SchedulingClass; }

    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters { public ulong ReadOperations; public ulong WriteOperations; public ulong OtherOperations; public ulong ReadBytes; public ulong WriteBytes; public ulong OtherBytes; }

    [StructLayout(LayoutKind.Sequential)]
    private struct JobExtendedLimitInformation {
        public JobBasicLimitInformation Basic;
        public IoCounters Io;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    [DllImport("userenv.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int CreateAppContainerProfile(string name, string display, string description, IntPtr capabilities, uint capabilityCount, out IntPtr sid);
    [DllImport("userenv.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int DeriveAppContainerSidFromAppContainerName(string name, out IntPtr sid);
    [DllImport("userenv.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int DeleteAppContainerProfile(string name);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool ConvertSidToStringSid(IntPtr sid, out IntPtr stringSid);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr processHandle, uint desiredAccess, out IntPtr tokenHandle);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(IntPtr tokenHandle, int tokenInformationClass, IntPtr tokenInformation, uint tokenInformationLength, out uint returnLength);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool EqualSid(IntPtr sid1, IntPtr sid2);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern IntPtr FreeSid(IntPtr sid);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LocalFree(IntPtr value);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CreatePipe(out IntPtr readHandle, out IntPtr writeHandle, ref SecurityAttributes attributes, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute, ref SecurityCapabilities value, IntPtr size, IntPtr previous, IntPtr returnedSize);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcess(string applicationName, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment, string currentDirectory, ref StartupInfoEx startup, out ProcessInformation information);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(IntPtr process, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(IntPtr job, int informationClass, ref JobExtendedLimitInformation information, uint length);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint SearchPath(string path, string file, string extension, int length, StringBuilder buffer, IntPtr filePart);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetSystemDirectory(StringBuilder buffer, int length);
    [DllImport("kernelbase.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool DeriveCapabilitySidsFromName(string capName, out IntPtr groupSids, out uint groupCount, out IntPtr capabilitySids, out uint capabilityCount);

    public static int Main(string[] args) {
        try {
            Options options = Parse(args);
            if (options.Cleanup) { Cleanup(options); return 0; }
            return Run(options);
        } catch (Exception error) {
            Console.Error.WriteLine(error.ToString());
            return 20;
        }
    }

    private static Options Parse(string[] args) {
        Options options = new Options();
        bool commandTail = false;
        for (int i = 0; i < args.Length; i++) {
            if (commandTail) { options.Arguments.Add(args[i]); continue; }
            string key = args[i];
            if (key == "--") { commandTail = true; continue; }
            if (key == "--run") continue;
            if (key == "--cleanup") { options.Cleanup = true; continue; }
            if (key == "--profile") options.Profile = args[++i];
            else if (key == "--program") options.Program = args[++i];
            else if (key == "--cwd") options.Cwd = args[++i];
            else if (key == "--workspace") options.Workspace = args[++i];
            else if (key == "--state") options.State = args[++i];
            else if (key == "--completion-marker") options.CompletionMarker = args[++i];
            else if (key == "--protected-state") options.ProtectedState = args[++i];
            else if (key == "--external") {
                string access = args[++i];
                string path = args[++i];
                options.External.Add(new Grant(path, access));
            }
            else if (key == "--network") options.Network = args[++i];
            else if (key == "--raw-arg") options.RawArg = args[++i];
            else if (key == "--sandbox-env") {
                string value = args[++i];
                int split = value.IndexOf('=');
                if (split <= 0) throw new InvalidOperationException("invalid sandbox environment entry");
                options.SandboxEnvironment[value.Substring(0, split)] = value.Substring(split + 1);
            } else throw new InvalidOperationException("unknown helper argument: " + key);
        }
        if (String.IsNullOrWhiteSpace(options.Profile) || String.IsNullOrWhiteSpace(options.Workspace) || String.IsNullOrWhiteSpace(options.State) || String.IsNullOrWhiteSpace(options.ProtectedState)) throw new InvalidOperationException("profile, workspace, state and protected-state are required");
        if (!options.Cleanup && (String.IsNullOrWhiteSpace(options.Program) || String.IsNullOrWhiteSpace(options.Cwd))) throw new InvalidOperationException("program and cwd are required");
        return options;
    }

    private static int Run(Options options) {
        Directory.CreateDirectory(options.State);
        IntPtr sid = IntPtr.Zero;
        SecurityIdentifier sidIdentity = null;
        List<string> granted = new List<string>();
        IntPtr profileSid = IntPtr.Zero;
        bool profileCreated = false;
        try {
            int result = CreateAppContainerProfile(options.Profile, "Coding Tools MCP Sandbox", "Coding Tools MCP isolated workspace process", IntPtr.Zero, 0, out profileSid);
            if (result < 0) throw new InvalidOperationException("CreateAppContainerProfile failed: 0x" + result.ToString("X8"));
            profileCreated = true;
            sid = profileSid;
            sidIdentity = new SecurityIdentifier(SidText(sid));
            ProtectLeaseDirectory(options, sidIdentity);
            // Protect sensitive workspace metadata before granting inheritable workspace
            // modify access so the broad workspace ACE cannot flow into .git/.github.
            RestrictProtected(options, sidIdentity, granted);
            ApplyGrant(options, options.Workspace, sidIdentity, "modify", granted);
            ApplyGrant(options, options.State, sidIdentity, "modify", granted);
            foreach (Grant grant in options.External) ApplyGrant(options, grant.Path, sidIdentity, grant.Access, granted);
            string program = ResolveProgram(options.Program);
            GrantRuntime(options, program, sidIdentity, granted);
            int exitCode = Launch(options, sid, program);
            bool cleanupOk = RemoveGrants(options, sidIdentity, granted);
            int deleteResult = DeleteAppContainerProfile(options.Profile);
            FreeSid(sid);
            sid = IntPtr.Zero;
            if (cleanupOk && deleteResult >= 0) TryMarkCompleted(options);
            return exitCode;
        } catch {
            bool cleanupOk = true;
            if (sidIdentity != null) cleanupOk = RemoveGrants(options, sidIdentity, granted);
            else cleanupOk = RestoreProtected(options);
            if (sid != IntPtr.Zero) { FreeSid(sid); sid = IntPtr.Zero; }
            if (profileCreated) {
                int deleteResult = DeleteAppContainerProfile(options.Profile);
                if (cleanupOk && deleteResult >= 0) TryMarkCompleted(options);
            }
            throw;
        }
    }

    private static void TryMarkCompleted(Options options) {
        if (String.IsNullOrWhiteSpace(options.CompletionMarker)) return;
        try { File.WriteAllText(options.CompletionMarker, "clean"); } catch { }
    }

    private static void Cleanup(Options options) {
        IntPtr sid = IntPtr.Zero;
        bool cleaned = true;
        int deleteResult = -1;
        try {
            int result = DeriveAppContainerSidFromAppContainerName(options.Profile, out sid);
            if (result >= 0 && sid != IntPtr.Zero) {
                SecurityIdentifier sidIdentity = new SecurityIdentifier(SidText(sid));
                List<string> paths = new List<string> { options.Workspace, options.State };
                foreach (Grant grant in options.External) paths.Add(grant.Path);
                cleaned = CleanupRecordedGrants(options, sidIdentity) && cleaned;
                foreach (string path in paths) cleaned = RemoveSid(path, sidIdentity) && cleaned;
                cleaned = RemoveSid(Path.Combine(options.Workspace, ".git"), sidIdentity) && cleaned;
                cleaned = RemoveSid(Path.Combine(options.Workspace, ".github"), sidIdentity) && cleaned;
            } else {
                cleaned = false;
            }
        } catch {
            cleaned = false;
        } finally {
            if (sid != IntPtr.Zero) FreeSid(sid);
            deleteResult = DeleteAppContainerProfile(options.Profile);
        }
        cleaned = deleteResult >= 0 && cleaned;
        cleaned = RestoreProtected(options) && cleaned;
        if (!cleaned) throw new InvalidOperationException("AppContainer ACL cleanup or protected-DACL restoration was incomplete.");
    }

    private static string SidText(IntPtr sid) {
        IntPtr value;
        if (!ConvertSidToStringSid(sid, out value)) throw new InvalidOperationException("ConvertSidToStringSid failed");
        try { return Marshal.PtrToStringUni(value); } finally { LocalFree(value); }
    }

    private static string LeaseDirectory(Options options) {
        string directory = Path.GetDirectoryName(options.ProtectedState);
        if (String.IsNullOrWhiteSpace(directory) || !Directory.Exists(directory)) throw new InvalidOperationException("lease directory does not exist");
        return Path.GetFullPath(directory);
    }

    private static bool PathContains(string parent, string child) {
        string normalizedParent = Path.GetFullPath(parent).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        string normalizedChild = Path.GetFullPath(child).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        if (String.Equals(normalizedParent, normalizedChild, StringComparison.OrdinalIgnoreCase)) return true;
        return normalizedChild.StartsWith(normalizedParent + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private static void ProtectLeaseDirectory(Options options, SecurityIdentifier sid) {
        string leaseDirectory = LeaseDirectory(options);
        DirectoryInfo info = new DirectoryInfo(leaseDirectory);
        DirectorySecurity security = info.GetAccessControl(AccessControlSections.Access);
        security.SetAccessRuleProtection(true, true);
        security.PurgeAccessRules(sid);
        security.AddAccessRule(new FileSystemAccessRule(
            sid,
            FileSystemRights.FullControl,
            InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
            PropagationFlags.None,
            AccessControlType.Deny
        ));
        info.SetAccessControl(security);
    }

    private static void EnsureGrantDoesNotCoverLeaseDirectory(Options options, string grantPath) {
        string leaseDirectory = LeaseDirectory(options);
        string fullGrant = Path.GetFullPath(grantPath);
        if (PathContains(leaseDirectory, fullGrant)) {
            throw new InvalidOperationException("sandbox ACL grant targets the protected lease directory: " + fullGrant);
        }
    }

    private static void EnsureGrantDoesNotTargetProtectedMetadata(Options options, string grantPath) {
        string fullGrant = Path.GetFullPath(grantPath);
        foreach (string name in new[] { ".git", ".github" }) {
            string protectedPath = Path.GetFullPath(Path.Combine(options.Workspace, name));
            if ((Directory.Exists(protectedPath) || File.Exists(protectedPath)) && PathContains(protectedPath, fullGrant)) {
                throw new InvalidOperationException("sandbox ACL grant targets protected repository metadata: " + fullGrant);
            }
        }
    }

    private static void ApplyGrant(Options options, string path, SecurityIdentifier sid, string access, List<string> granted) {
        if (!Directory.Exists(path) && !File.Exists(path)) throw new InvalidOperationException("ACL target does not exist: " + path);
        EnsureGrantDoesNotCoverLeaseDirectory(options, path);
        EnsureGrantDoesNotTargetProtectedMetadata(options, path);
        GrantAncestorTraversal(options, path, sid, granted);
        if (!HasPath(granted, path)) {
            RecordGrant(options, path);
            granted.Add(path);
        }
        AddRule(path, sid, access);
    }

    private static string EncodeStateValue(string value) {
        return Convert.ToBase64String(Encoding.UTF8.GetBytes(value));
    }

    private static string DecodeStateValue(string value) {
        return Encoding.UTF8.GetString(Convert.FromBase64String(value));
    }

    private static void RecordGrant(Options options, string path) {
        File.AppendAllLines(options.ProtectedState, new[] { "G\t" + EncodeStateValue(Path.GetFullPath(path)) });
    }

    private static bool HasPath(List<string> paths, string value) {
        foreach (string path in paths) if (String.Equals(path, value, StringComparison.OrdinalIgnoreCase)) return true;
        return false;
    }

    private sealed class ProtectedAclTarget {
        public readonly string Path;
        public readonly bool IsDirectory;
        public readonly FileSystemSecurity Security;

        public ProtectedAclTarget(string path, bool isDirectory, FileSystemSecurity security) {
            Path = path;
            IsDirectory = isDirectory;
            Security = security;
        }
    }

    private static void RestrictProtected(Options options, SecurityIdentifier sid, List<string> granted) {
        List<ProtectedAclTarget> targets = new List<ProtectedAclTarget>();
        List<string> state = new List<string>();
        foreach (string name in new[] { ".git", ".github" }) {
            string path = Path.Combine(options.Workspace, name);
            string sddl;
            if (Directory.Exists(path)) {
                DirectorySecurity security = new DirectoryInfo(path).GetAccessControl(AccessControlSections.Access);
                sddl = security.GetSecurityDescriptorSddlForm(AccessControlSections.Access);
                targets.Add(new ProtectedAclTarget(path, true, security));
            } else if (File.Exists(path)) {
                FileSecurity security = new FileInfo(path).GetAccessControl(AccessControlSections.Access);
                sddl = security.GetSecurityDescriptorSddlForm(AccessControlSections.Access);
                targets.Add(new ProtectedAclTarget(path, false, security));
            } else {
                continue;
            }
            state.Add("D\t" + name + "\t" + EncodeStateValue(sddl));
            state.Add("G\t" + EncodeStateValue(Path.GetFullPath(path)));
        }
        File.WriteAllLines(options.ProtectedState, state.ToArray());
        foreach (ProtectedAclTarget target in targets) {
            string path = target.Path;
            FileSystemSecurity security = target.Security;
            security.SetAccessRuleProtection(true, true);
            security.PurgeAccessRules(sid);
            security.AddAccessRule(new FileSystemAccessRule(
                sid,
                target.IsDirectory ? FileSystemRights.ReadAndExecute : FileSystemRights.Read,
                target.IsDirectory ? InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit : InheritanceFlags.None,
                PropagationFlags.None,
                AccessControlType.Allow
            ));
            if (target.IsDirectory) {
                if (!Directory.Exists(path)) throw new InvalidOperationException("protected workspace metadata disappeared before ACL restriction: " + path);
                new DirectoryInfo(path).SetAccessControl((DirectorySecurity)security);
            } else {
                if (!File.Exists(path)) throw new InvalidOperationException("protected workspace metadata disappeared before ACL restriction: " + path);
                new FileInfo(path).SetAccessControl((FileSecurity)security);
            }
            if (!HasPath(granted, path)) granted.Add(path);
        }
    }

    private static bool RestoreProtected(Options options) {
        if (String.IsNullOrWhiteSpace(options.ProtectedState) || !File.Exists(options.ProtectedState)) return true;
        bool restored = true;
        string[] lines;
        try { lines = File.ReadAllLines(options.ProtectedState); } catch { return false; }
        foreach (string line in lines) {
            if (String.IsNullOrWhiteSpace(line) || line.StartsWith("G\t", StringComparison.Ordinal)) continue;
            if (!line.StartsWith("D\t", StringComparison.Ordinal)) { restored = false; continue; }
            string[] parts = line.Split(new[] { '\t' }, 3);
            if (parts.Length != 3) { restored = false; continue; }
            string name = parts[1];
            string encoded = parts[2];
            if (name != ".git" && name != ".github") { restored = false; continue; }
            string path = Path.Combine(options.Workspace, name);
            if (!Directory.Exists(path) && !File.Exists(path)) { restored = false; continue; }
            try {
                string sddl = DecodeStateValue(encoded);
                if (Directory.Exists(path)) {
                    DirectoryInfo info = new DirectoryInfo(path);
                    DirectorySecurity security = new DirectorySecurity();
                    security.SetSecurityDescriptorSddlForm(sddl, AccessControlSections.Access);
                    info.SetAccessControl(security);
                } else {
                    FileInfo info = new FileInfo(path);
                    FileSecurity security = new FileSecurity();
                    security.SetSecurityDescriptorSddlForm(sddl, AccessControlSections.Access);
                    info.SetAccessControl(security);
                }
            } catch { restored = false; }
        }
        return restored;
    }

    private static bool CleanupRecordedGrants(Options options, SecurityIdentifier sid) {
        if (String.IsNullOrWhiteSpace(options.ProtectedState) || !File.Exists(options.ProtectedState)) return true;
        bool removed = true;
        string[] lines;
        try { lines = File.ReadAllLines(options.ProtectedState); } catch { return false; }
        foreach (string line in lines) {
            if (String.IsNullOrWhiteSpace(line) || line.StartsWith("D\t", StringComparison.Ordinal)) continue;
            if (!line.StartsWith("G\t", StringComparison.Ordinal)) { removed = false; continue; }
            try {
                string path = DecodeStateValue(line.Substring(2));
                if (!Path.IsPathRooted(path)) { removed = false; continue; }
                removed = RemoveSid(path, sid) && removed;
            } catch { removed = false; }
        }
        return removed;
    }

    private static void GrantAncestorTraversal(Options options, string targetPath, SecurityIdentifier sid, List<string> granted) {
        DirectoryInfo current = Directory.GetParent(Path.GetFullPath(targetPath));
        while (current != null) {
            string ancestor = current.FullName;
            if (!HasPath(granted, ancestor)) {
                RecordGrant(options, ancestor);
                granted.Add(ancestor);
                AddDirectoryRule(
                    ancestor,
                    sid,
                    FileSystemRights.Traverse | FileSystemRights.ReadAttributes,
                    InheritanceFlags.None
                );
            }
            current = current.Parent;
        }
    }

    private static void AddDirectoryRule(string path, SecurityIdentifier sid, FileSystemRights rights, InheritanceFlags inheritance) {
        DirectoryInfo info = new DirectoryInfo(path);
        DirectorySecurity security = info.GetAccessControl(AccessControlSections.Access);
        security.AddAccessRule(new FileSystemAccessRule(sid, rights, inheritance, PropagationFlags.None, AccessControlType.Allow));
        info.SetAccessControl(security);
    }

    private static void AddRule(string path, SecurityIdentifier sid, string access) {
        FileSystemRights rights = access == "modify" ? FileSystemRights.Modify : FileSystemRights.ReadAndExecute;
        if (Directory.Exists(path)) {
            AddDirectoryRule(path, sid, rights, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit);
        } else {
            FileInfo info = new FileInfo(path);
            FileSecurity security = info.GetAccessControl(AccessControlSections.Access);
            security.AddAccessRule(new FileSystemAccessRule(sid, rights, InheritanceFlags.None, PropagationFlags.None, AccessControlType.Allow));
            info.SetAccessControl(security);
        }
    }

    private static bool IsProtectedMetadataRoot(Options options, string value) {
        string path = Path.GetFullPath(value);
        foreach (string name in new[] { ".git", ".github" }) {
            if (String.Equals(path, Path.GetFullPath(Path.Combine(options.Workspace, name)), StringComparison.OrdinalIgnoreCase)) return true;
        }
        return false;
    }

    private static bool RemoveGrants(Options options, SecurityIdentifier sid, List<string> paths) {
        bool removed = true;
        List<string> protectedPaths = new List<string>();
        foreach (string path in paths) {
            if (IsProtectedMetadataRoot(options, path)) protectedPaths.Add(path);
            else removed = RemoveSid(path, sid) && removed;
        }
        bool restored = RestoreProtected(options);
        if (!restored) {
            foreach (string path in protectedPaths) removed = RemoveSid(path, sid) && removed;
        }
        return removed && restored;
    }

    private static bool RemoveSid(string path, SecurityIdentifier sid) {
        if (!Directory.Exists(path) && !File.Exists(path)) return true;
        try {
            if (Directory.Exists(path)) {
                DirectoryInfo info = new DirectoryInfo(path);
                DirectorySecurity security = info.GetAccessControl(AccessControlSections.Access);
                security.PurgeAccessRules(sid);
                info.SetAccessControl(security);
            } else {
                FileInfo info = new FileInfo(path);
                FileSecurity security = info.GetAccessControl(AccessControlSections.Access);
                security.PurgeAccessRules(sid);
                info.SetAccessControl(security);
            }
            return true;
        } catch { return false; }
    }

    private static string ResolveProgram(string value) {
        if (Path.IsPathRooted(value)) {
            string full = Path.GetFullPath(value);
            if (!File.Exists(full)) throw new InvalidOperationException("program does not exist: " + value);
            return full;
        }
        StringBuilder buffer = new StringBuilder(32768);
        uint length = SearchPath(null, value, null, buffer.Capacity, buffer, IntPtr.Zero);
        if (length == 0 || length >= buffer.Capacity) throw new InvalidOperationException("program was not found on PATH: " + value);
        return buffer.ToString();
    }

    private static void GrantRuntime(Options options, string program, SecurityIdentifier sid, List<string> granted) {
        string system = SystemDirectory();
        if (program.StartsWith(system, StringComparison.OrdinalIgnoreCase)) return;
        string root = Path.GetDirectoryName(program);
        if (String.IsNullOrEmpty(root) || !Directory.Exists(root)) throw new InvalidOperationException("program runtime directory does not exist: " + program);
        ApplyGrant(options, root, sid, "read_execute", granted);
        string name = Path.GetFileName(program).ToLowerInvariant();
        if (name == "cargo.exe" || name == "rustc.exe") {
            string toolchain = Directory.GetParent(root).FullName;
            string lib = Path.Combine(toolchain, "lib");
            if (Directory.Exists(lib)) ApplyGrant(options, lib, sid, "read_execute", granted);
        }
        if (name.StartsWith("python")) {
            string cfg = Path.Combine(Directory.GetParent(root).FullName, "pyvenv.cfg");
            if (File.Exists(cfg)) {
                foreach (string line in File.ReadAllLines(cfg)) {
                    if (!line.TrimStart().StartsWith("home", StringComparison.OrdinalIgnoreCase)) continue;
                    int split = line.IndexOf('=');
                    if (split < 0) continue;
                    string home = line.Substring(split + 1).Trim();
                    if (Directory.Exists(home)) ApplyGrant(options, home, sid, "read_execute", granted);
                    break;
                }
            }
        }
    }

    private static string SystemDirectory() {
        StringBuilder buffer = new StringBuilder(32768);
        uint length = GetSystemDirectory(buffer, buffer.Capacity);
        if (length == 0 || length >= buffer.Capacity) return Environment.GetFolderPath(Environment.SpecialFolder.System);
        return buffer.ToString();
    }

    private static void VerifyAppContainerToken(IntPtr process, IntPtr expectedSid) {
        IntPtr token = IntPtr.Zero;
        IntPtr isAppContainer = IntPtr.Zero;
        IntPtr sidInfo = IntPtr.Zero;
        try {
            if (!OpenProcessToken(process, TOKEN_QUERY, out token)) throw new InvalidOperationException("OpenProcessToken failed: " + Marshal.GetLastWin32Error());
            isAppContainer = Marshal.AllocHGlobal(sizeof(int));
            Marshal.WriteInt32(isAppContainer, 0);
            uint returned;
            if (!GetTokenInformation(token, TOKEN_IS_APPCONTAINER, isAppContainer, sizeof(int), out returned)) throw new InvalidOperationException("TokenIsAppContainer query failed: " + Marshal.GetLastWin32Error());
            if (Marshal.ReadInt32(isAppContainer) == 0) throw new InvalidOperationException("Created process is not running with an AppContainer token.");

            int infoSize = Marshal.SizeOf(typeof(TokenAppContainerInformation));
            sidInfo = Marshal.AllocHGlobal(infoSize);
            if (!GetTokenInformation(token, TOKEN_APPCONTAINER_SID, sidInfo, (uint)infoSize, out returned)) throw new InvalidOperationException("TokenAppContainerSid query failed: " + Marshal.GetLastWin32Error());
            TokenAppContainerInformation info = (TokenAppContainerInformation)Marshal.PtrToStructure(sidInfo, typeof(TokenAppContainerInformation));
            if (info.TokenAppContainer == IntPtr.Zero || !EqualSid(info.TokenAppContainer, expectedSid)) throw new InvalidOperationException("Created process AppContainer SID does not match the requested sandbox profile.");
        } finally {
            if (sidInfo != IntPtr.Zero) Marshal.FreeHGlobal(sidInfo);
            if (isAppContainer != IntPtr.Zero) Marshal.FreeHGlobal(isAppContainer);
            if (token != IntPtr.Zero) CloseHandle(token);
        }
    }

    // Temporary Object Manager links are owned by handles, so cancellation or
    // host termination cannot leave a persistent DefineDosDevice mapping.
    private sealed class GitDriveAlias : IDisposable {
        [StructLayout(LayoutKind.Sequential)]
        private struct UnicodeString { public ushort Length, MaximumLength; public IntPtr Buffer; }
        [StructLayout(LayoutKind.Sequential)]
        private struct ObjectAttributes {
            public uint Length; public IntPtr RootDirectory, ObjectName; public uint Attributes;
            public IntPtr SecurityDescriptor, SecurityQualityOfService;
        }
        [DllImport("ntdll.dll")]
        private static extern int NtCreateSymbolicLinkObject(out IntPtr handle, uint access, ref ObjectAttributes attributes, ref UnicodeString target);
        [DllImport("ntdll.dll")]
        private static extern uint RtlNtStatusToDosError(int status);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint GetLogicalDrives();
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateFileW(string path, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern uint GetFinalPathNameByHandleW(IntPtr handle, StringBuilder path, uint length, uint flags);
        private IntPtr handle;
        private string root, drive;
        private static string Canonical(string path) {
            IntPtr file = CreateFileW(path, 0, 7, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero);
            if (file == new IntPtr(-1)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            try {
                StringBuilder result = new StringBuilder(32768);
                uint length = GetFinalPathNameByHandleW(file, result, (uint)result.Capacity, 0);
                if (length == 0 || length >= result.Capacity) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
                return result.ToString().TrimEnd('\\');
            } finally { CloseHandle(file); }
        }
        public static GitDriveAlias Create(string workspace) {
            string root = Canonical(workspace);
            string dos = root.StartsWith(@"\\?\", StringComparison.Ordinal) ? root.Substring(4) : root;
            if (dos.Length < 2 || dos[1] != ':' || !Char.IsLetter(dos[0])) throw new InvalidOperationException("Git DOS alias requires a local drive workspace.");
            string targetText = @"\??\" + dos;
            IntPtr targetBuffer = Marshal.StringToHGlobalUni(targetText);
            try {
                UnicodeString target = new UnicodeString { Length = checked((ushort)(targetText.Length * 2)), MaximumLength = checked((ushort)(targetText.Length * 2)), Buffer = targetBuffer };
                uint used = GetLogicalDrives();
                if (used == 0) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
                for (char letter = 'Z'; letter >= 'D'; letter--) {
                    if ((used & (1u << (letter - 'A'))) != 0) continue;
                    string nameText = @"\??\" + letter + ":";
                    IntPtr nameBuffer = Marshal.StringToHGlobalUni(nameText);
                    IntPtr namePointer = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(UnicodeString)));
                    try {
                        UnicodeString name = new UnicodeString { Length = checked((ushort)(nameText.Length * 2)), MaximumLength = checked((ushort)(nameText.Length * 2)), Buffer = nameBuffer };
                        Marshal.StructureToPtr(name, namePointer, false);
                        ObjectAttributes attributes = new ObjectAttributes { Length = (uint)Marshal.SizeOf(typeof(ObjectAttributes)), ObjectName = namePointer, Attributes = 0x40 };
                        IntPtr link;
                        int status = NtCreateSymbolicLinkObject(out link, 1, ref attributes, ref target);
                        if (status == unchecked((int)0xc0000035)) continue; // Never replace an existing name.
                        if (status < 0) throw new System.ComponentModel.Win32Exception((int)RtlNtStatusToDosError(status));
                        GitDriveAlias alias = new GitDriveAlias { handle = link, root = root, drive = letter + @":\" };
                        try {
                            if (!String.Equals(Canonical(alias.drive), root, StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Git DOS alias resolved to an unexpected workspace.");
                            return alias;
                        } catch { alias.Dispose(); throw; }
                    } finally { Marshal.FreeHGlobal(namePointer); Marshal.FreeHGlobal(nameBuffer); }
                }
                throw new InvalidOperationException("No free DOS drive letter for sandboxed Git.");
            } finally { Marshal.FreeHGlobal(targetBuffer); }
        }
        public string Cwd(string path) {
            string physical = Canonical(path);
            if (String.Equals(physical, root, StringComparison.OrdinalIgnoreCase)) return drive;
            if (!physical.StartsWith(root + @"\", StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Git cwd is outside its workspace.");
            return Path.Combine(drive, physical.Substring(root.Length + 1));
        }
        public void Dispose() {
            if (handle != IntPtr.Zero) { CloseHandle(handle); handle = IntPtr.Zero; }
        }
    }

    private static int Launch(Options options, IntPtr appContainerSid, string program) {
        SecurityAttributes attributes = new SecurityAttributes { Length = Marshal.SizeOf(typeof(SecurityAttributes)), InheritHandle = 1 };
        IntPtr childInput, parentInput, parentOutput, childOutput, parentError, childError;
        if (!CreatePipe(out childInput, out parentInput, ref attributes, 0)) throw new InvalidOperationException("CreatePipe stdin failed");
        if (!CreatePipe(out parentOutput, out childOutput, ref attributes, 0)) throw new InvalidOperationException("CreatePipe stdout failed");
        if (!CreatePipe(out parentError, out childError, ref attributes, 0)) throw new InvalidOperationException("CreatePipe stderr failed");
        SetHandleInformation(parentInput, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(parentOutput, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(parentError, HANDLE_FLAG_INHERIT, 0);
        IntPtr attributeList = IntPtr.Zero;
        IntPtr environment = IntPtr.Zero;
        IntPtr capabilityBlock = IntPtr.Zero;
        ProcessInformation process = new ProcessInformation();
        IntPtr job = IntPtr.Zero;
        GitDriveAlias gitAlias = null;
        try {
            IntPtr size = IntPtr.Zero;
            InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
            attributeList = Marshal.AllocHGlobal(size.ToInt32());
            if (!InitializeProcThreadAttributeList(attributeList, 1, 0, ref size)) throw new InvalidOperationException("InitializeProcThreadAttributeList failed");
            SecurityCapabilities capabilities = new SecurityCapabilities { AppContainerSid = appContainerSid, Capabilities = IntPtr.Zero, CapabilityCount = 0, Reserved = 0 };
            if (UsesInternet(options.Network)) {
                SidAndAttributes internet = new SidAndAttributes { Sid = DeriveInternetCapabilitySid(), Attributes = 0x00000004 };
                capabilityBlock = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SidAndAttributes)));
                Marshal.StructureToPtr(internet, capabilityBlock, false);
                capabilities.Capabilities = capabilityBlock;
                capabilities.CapabilityCount = 1;
            }
            if (!UpdateProcThreadAttribute(attributeList, 0, (IntPtr)PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, ref capabilities, (IntPtr)Marshal.SizeOf(typeof(SecurityCapabilities)), IntPtr.Zero, IntPtr.Zero)) throw new InvalidOperationException("UpdateProcThreadAttribute failed");
            StartupInfoEx startup = new StartupInfoEx();
            startup.StartupInfo.Cb = Marshal.SizeOf(typeof(StartupInfoEx));
            startup.StartupInfo.Flags = 0x00000100;
            startup.StartupInfo.StdInput = childInput;
            startup.StartupInfo.StdOutput = childOutput;
            startup.StartupInfo.StdError = childError;
            startup.AttributeList = attributeList;
            environment = BuildEnvironment(options);
            StringBuilder commandLine = new StringBuilder(Quote(program));
            foreach (string argument in options.Arguments) commandLine.Append(' ').Append(Quote(argument));
            if (!String.IsNullOrEmpty(options.RawArg)) commandLine.Append(' ').Append(options.RawArg);
            uint flags = CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
            string launchCwd = options.Cwd;
            if (String.Equals(Path.GetFileName(program), "git.exe", StringComparison.OrdinalIgnoreCase)) {
                gitAlias = GitDriveAlias.Create(options.Workspace);
                launchCwd = gitAlias.Cwd(options.Cwd);
            }
            if (!CreateProcess(program, commandLine, IntPtr.Zero, IntPtr.Zero, true, flags, environment, launchCwd, ref startup, out process)) throw new InvalidOperationException("CreateProcessW failed: " + Marshal.GetLastWin32Error());
            try {
                VerifyAppContainerToken(process.Process, appContainerSid);
            } catch {
                if (process.Process != IntPtr.Zero) TerminateProcess(process.Process, 1);
                throw;
            }
            CloseHandle(childInput); childInput = IntPtr.Zero;
            CloseHandle(childOutput); childOutput = IntPtr.Zero;
            CloseHandle(childError); childError = IntPtr.Zero;
            job = CreateJobObject(IntPtr.Zero, null);
            JobExtendedLimitInformation limits = new JobExtendedLimitInformation();
            limits.Basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            bool jobAssigned = job != IntPtr.Zero && SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf(typeof(JobExtendedLimitInformation))) && AssignProcessToJobObject(job, process.Process);
            if (!jobAssigned) {
                if (process.Process != IntPtr.Zero) TerminateProcess(process.Process, 1);
                throw new InvalidOperationException("Failed to assign the suspended AppContainer process to its kill-on-close Job Object.");
            }
            if (ResumeThread(process.Thread) == 0xffffffff) {
                if (process.Process != IntPtr.Zero) TerminateProcess(process.Process, 1);
                throw new InvalidOperationException("ResumeThread failed");
            }
            Task stdin = Pump(Console.OpenStandardInput(), new FileStream(new Microsoft.Win32.SafeHandles.SafeFileHandle(parentInput, true), FileAccess.Write, 4096, false));
            Task stdout = Pump(new FileStream(new Microsoft.Win32.SafeHandles.SafeFileHandle(parentOutput, true), FileAccess.Read, 4096, false), Console.OpenStandardOutput());
            Task stderr = Pump(new FileStream(new Microsoft.Win32.SafeHandles.SafeFileHandle(parentError, true), FileAccess.Read, 4096, false), Console.OpenStandardError());
            uint wait = WaitForSingleObject(process.Process, INFINITE);
            if (wait == WAIT_FAILED) throw new InvalidOperationException("WaitForSingleObject failed");
            Task.WaitAll(new[] { stdin, stdout, stderr });
            uint code;
            if (!GetExitCodeProcess(process.Process, out code)) throw new InvalidOperationException("GetExitCodeProcess failed");
            return unchecked((int)code);
        } finally {
            if (childInput != IntPtr.Zero) CloseHandle(childInput);
            if (childOutput != IntPtr.Zero) CloseHandle(childOutput);
            if (childError != IntPtr.Zero) CloseHandle(childError);
            if (process.Process != IntPtr.Zero) CloseHandle(process.Process);
            if (process.Thread != IntPtr.Zero) CloseHandle(process.Thread);
            if (job != IntPtr.Zero) CloseHandle(job);
            if (gitAlias != null) gitAlias.Dispose();
            if (attributeList != IntPtr.Zero) { DeleteProcThreadAttributeList(attributeList); Marshal.FreeHGlobal(attributeList); }
            if (capabilityBlock != IntPtr.Zero) Marshal.FreeHGlobal(capabilityBlock);
            if (environment != IntPtr.Zero) Marshal.FreeHGlobal(environment);
        }
    }

    private static IntPtr BuildEnvironment(Options options) {
        Dictionary<string, string> values = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (DictionaryEntry entry in Environment.GetEnvironmentVariables()) values[(string)entry.Key] = (string)entry.Value;
        foreach (KeyValuePair<string, string> pair in options.SandboxEnvironment) values[pair.Key] = pair.Value;
        List<string> entries = new List<string>();
        foreach (KeyValuePair<string, string> pair in values) entries.Add(pair.Key + "=" + pair.Value);
        entries.Sort(StringComparer.OrdinalIgnoreCase);
        string block = String.Join("\0", entries) + "\0\0";
        return Marshal.StringToHGlobalUni(block);
    }

    private static async Task Pump(Stream source, Stream target) {
        try { await source.CopyToAsync(target); await target.FlushAsync(); } catch { } finally { source.Dispose(); target.Dispose(); }
    }

    private static bool UsesInternet(string value) {
        if (String.IsNullOrWhiteSpace(value)) return false;
        string normalized = value.Trim().ToLowerInvariant();
        return normalized == "internet" || normalized == "allow" || normalized == "true" || normalized == "on";
    }

    private static IntPtr DeriveInternetCapabilitySid() {
        IntPtr groupSids;
        IntPtr capabilitySids;
        uint groupCount;
        uint capabilityCount;
        if (!DeriveCapabilitySidsFromName("internetClient", out groupSids, out groupCount, out capabilitySids, out capabilityCount) || capabilityCount < 1 || capabilitySids == IntPtr.Zero) {
            throw new InvalidOperationException("DeriveCapabilitySidsFromName(internetClient) failed: " + Marshal.GetLastWin32Error());
        }
        return Marshal.ReadIntPtr(capabilitySids);
    }

    private static string Quote(string value) {
        if (value.Length == 0) return "\"\"";
        if (value.IndexOfAny(new[] { ' ', '\t', '\"' }) < 0) return value;
        StringBuilder result = new StringBuilder("\"");
        int slashes = 0;
        foreach (char character in value) {
            if (character == '\\') { slashes++; continue; }
            if (character == '\"') { result.Append(new string('\\', slashes * 2 + 1)); result.Append('\"'); slashes = 0; continue; }
            result.Append(new string('\\', slashes)); result.Append(character); slashes = 0;
        }
        result.Append(new string('\\', slashes * 2)); result.Append('\"');
        return result.ToString();
    }
}
`;

export const APPCONTAINER_HELPER_SOURCE_HASH = createHash('sha256').update(APPCONTAINER_HELPER_SOURCE).digest('hex').slice(0, 16);
