import { access, realpath, stat } from 'node:fs/promises';
import path from 'node:path';
import type { AgentConfig, JsonObject, ToolContext } from './types.js';
import { relativeInside, resolveFromWorkspace, resolveInside, rootAndCwd } from './workspace.js';
import { parseWslUncPath, validateWslExecPaths, WslRoutingError, wslUncPath } from './wsl.js';
import { DEFAULT_COMMAND_TIMEOUT_MAX_MS } from './executionLimits.js';
import { resolveProcessTimeout } from './processes/timeoutPolicy.js';

const BASIC_READ_ONLY_COMMANDS = ['pwd', 'ls', 'dir', 'cat', 'head', 'tail', 'grep', 'find', 'which', 'echo'];
const DEFAULT_ALLOWED_COMMANDS = [
  'pytest', 'python', 'python3', 'npm', 'npx', 'node', 'pnpm', 'yarn', 'make', 'mvn', 'mvnw',
  'gradle', 'gradlew', 'cargo', 'go', 'ruff', 'mypy', 'eslint', 'tsc', 'msbuild', 'dotnet',
  'deno', 'bun', 'ruby', 'java', 'javac', 'cmake', 'clang', 'gcc', 'g++', 'git', 'cmd',
  'powershell', 'pwsh', 'sh'
];
const DEFAULT_SCRIPT_EXTENSIONS = ['.exe', '.bat', '.cmd', '.ps1'];
const BLOCKED_ENVIRONMENT_KEYS = new Set([
  'PATH', 'PATHEXT', 'COMSPEC', 'LD_PRELOAD', 'LD_LIBRARY_PATH',
  'DYLD_INSERT_LIBRARIES', 'DYLD_LIBRARY_PATH'
]);
const NETWORK_PATTERN = /(https?:\/\/|urllib\.request|requests\.|http\.client|\bcurl\b|\bwget\b|\bssh\b|\bscp\b|\bftp\b)/i;
const DANGEROUS_PATTERN = /(git\s+reset\s+--hard|git\s+clean\s+-[^\r\n]*f|git\s+checkout\s+--\s+\.|(^|\s)rm\s+(-[^\r\n]*r[^\r\n]*f|--recursive)|remove-item\s+[^\r\n]*-recurse|(^|\s)(rmdir|del)\s+\/s\b)/i;
const INTERPRETER_MUTATION_PATTERN = /(shutil\.(rmtree|move)|os\.(remove|unlink|rmdir)|pathlib\.[^\s;]+\.(unlink|rename)|write_text|write_bytes|fs\.(writefile|writefilesync|unlink|rm)|set-content|out-file|new-item|files?\.(write|delete)|open\([^)]*['"]w)/i;

export interface ResolvedCommandSpec {
  program: string;
  argv: string[];
  display: string;
  shell: false;
}

export class PolicyError extends Error {
  readonly code: string;
  constructor(message: string, code = 'POLICY_REJECTED') {
    super(message);
    this.name = 'PolicyError';
    this.code = code;
  }
}

export function defaultPolicy(): AgentConfig['policy'] {
  return {
    allowedCommands: [...new Set([...DEFAULT_ALLOWED_COMMANDS, ...BASIC_READ_ONLY_COMMANDS])],
    workspaceLocalEntries: true,
    workspaceScriptExtensions: [...DEFAULT_SCRIPT_EXTENSIONS],
    maxPatchBytes: 200_000
  };
}

export function mergeAllowedCommands(configured: unknown): string[] {
  const defaults = defaultPolicy().allowedCommands;
  const values = Array.isArray(configured)
    ? configured
    : typeof configured === 'string'
      ? configured.split(',')
      : [];
  return [...new Set([...defaults, ...values.map(String).map(value => value.trim().toLowerCase()).filter(Boolean)])];
}

export function normalizeScriptExtensions(configured: unknown): string[] {
  const values = Array.isArray(configured)
    ? configured
    : typeof configured === 'string'
      ? configured.split(',')
      : [];
  const normalized = values.map(String).map(value => value.trim().toLowerCase()).filter(Boolean)
    .map(value => value.startsWith('.') ? value : `.${value}`);
  return normalized.length ? [...new Set(normalized)] : [...DEFAULT_SCRIPT_EXTENSIONS];
}

export function splitShellWords(command: string): string[] {
  const parts: string[] = [];
  let current = '';
  let quote: 'single' | 'double' | undefined;
  let escaped = false;
  for (const character of command) {
    if (escaped) {
      current += character;
      escaped = false;
      continue;
    }
    if (character === '\\' && quote !== 'single') {
      escaped = true;
      continue;
    }
    if (quote === 'single') {
      if (character === "'") quote = undefined;
      else current += character;
      continue;
    }
    if (quote === 'double') {
      if (character === '"') quote = undefined;
      else current += character;
      continue;
    }
    if (character === "'") { quote = 'single'; continue; }
    if (character === '"') { quote = 'double'; continue; }
    if (/\s/.test(character)) {
      if (current) { parts.push(current); current = ''; }
      continue;
    }
    current += character;
  }
  if (escaped || quote) throw new PolicyError('Invalid command syntax', 'INVALID_ARGUMENT');
  if (current) parts.push(current);
  return parts;
}

function hasForbiddenShellSyntax(command: string): boolean {
  if (/[\r\n]/.test(command)) return true;
  let quote: 'single' | 'double' | undefined;
  let escaped = false;
  for (let index = 0; index < command.length; index += 1) {
    const character = command[index];
    if (escaped) { escaped = false; continue; }
    if (quote === 'single') {
      if (character === "'") quote = undefined;
      continue;
    }
    if (quote === 'double') {
      if (character === '\\') escaped = true;
      else if (character === '"') quote = undefined;
      continue;
    }
    if (character === '\\') { escaped = true; continue; }
    if (character === "'") { quote = 'single'; continue; }
    if (character === '"') { quote = 'double'; continue; }
    if (';&|><`'.includes(character)) return true;
    if (character === '$' && ['(', '{'].includes(command[index + 1] ?? '')) return true;
  }
  return false;
}

function relativePathAllowed(value: string): boolean {
  if (path.isAbsolute(value)) return false;
  return !value.replaceAll('\\', '/').split('/').includes('..');
}

function commandTargetsProtectedRepositoryAsset(command: string): boolean {
  const normalized = command.toLowerCase().replaceAll('\\', '/');
  if (!normalized.includes('.git') && !normalized.includes('.github')) return false;
  return ['rm ', 'remove-item', 'rmdir', 'del ', 'unlink', 'rmtree', 'write_text', 'writefile', 'rename', 'move', 'checkout', 'clean ']
    .some(value => normalized.includes(value));
}

function commandContainsExternalPath(command: string): boolean {
  const normalized = command.replaceAll('\\', '/');
  return normalized.includes('../') || /(^|["'\s])\/(?!\/)/.test(normalized) || /\b[A-Z]:\//i.test(normalized);
}

function environmentKey(value: string): void {
  if (!/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(value)) throw new PolicyError(`Invalid environment key: ${value}`, 'INVALID_ARGUMENT');
}

function secretReferenceKey(value: unknown): string {
  if (typeof value !== 'string' || !/^[A-Za-z0-9._-]{1,128}$/.test(value.trim())) {
    throw new PolicyError('Secret references must use 1-128 letters, numbers, dot, underscore, or hyphen', 'INVALID_ARGUMENT');
  }
  return value.trim();
}

function validateEnvironment(argumentsValue: JsonObject, protectEnvironment: boolean, enforceResourceLimits: boolean): void {
  if (argumentsValue.env !== undefined) {
    if (!argumentsValue.env || typeof argumentsValue.env !== 'object' || Array.isArray(argumentsValue.env)) {
      throw new PolicyError('env must be an object of string values', 'INVALID_ARGUMENT');
    }
    const entries = Object.entries(argumentsValue.env as JsonObject);
    if (enforceResourceLimits && entries.length > 64) throw new PolicyError('env contains too many entries', 'INVALID_ARGUMENT');
    for (const [key, raw] of entries) {
      environmentKey(key);
      if (protectEnvironment && [...BLOCKED_ENVIRONMENT_KEYS].some(blocked => blocked.toLowerCase() === key.toLowerCase())) {
        throw new PolicyError(`Environment variable is protected: ${key}`, 'ENVIRONMENT_VARIABLE_PROTECTED');
      }
      if (typeof raw !== 'string') throw new PolicyError('env values must be strings', 'INVALID_ARGUMENT');
      if ((enforceResourceLimits && raw.length > 4096) || raw.includes('\0')) throw new PolicyError(`Invalid environment value for ${key}`, 'INVALID_ARGUMENT');
    }
  }
  if (argumentsValue.secret_env !== undefined) {
    if (!argumentsValue.secret_env || typeof argumentsValue.secret_env !== 'object' || Array.isArray(argumentsValue.secret_env)) {
      throw new PolicyError('secret_env must be an object mapping environment names to secret references', 'INVALID_ARGUMENT');
    }
    const entries = Object.entries(argumentsValue.secret_env as JsonObject);
    if (enforceResourceLimits && entries.length > 64) throw new PolicyError('secret_env contains too many entries', 'INVALID_ARGUMENT');
    const direct = new Set(Object.keys((argumentsValue.env as JsonObject | undefined) ?? {}).map(key => key.toLowerCase()));
    const removed = new Set((Array.isArray(argumentsValue.remove_env) ? argumentsValue.remove_env : []).filter(value => typeof value === 'string').map(value => value.toLowerCase()));
    for (const [key, raw] of entries) {
      environmentKey(key);
      if (protectEnvironment && [...BLOCKED_ENVIRONMENT_KEYS].some(blocked => blocked.toLowerCase() === key.toLowerCase())) {
        throw new PolicyError(`Environment variable is protected: ${key}`, 'ENVIRONMENT_VARIABLE_PROTECTED');
      }
      secretReferenceKey(raw);
      if (direct.has(key.toLowerCase()) || removed.has(key.toLowerCase())) {
        throw new PolicyError(`Environment variable ${key} cannot be configured by both env/remove_env and secret_env`, 'INVALID_ARGUMENT');
      }
    }
  }
  if (argumentsValue.stdin_secret !== undefined) {
    secretReferenceKey(argumentsValue.stdin_secret);
    if (typeof argumentsValue.stdin === 'string' && argumentsValue.stdin.length > 0) {
      throw new PolicyError('stdin and stdin_secret cannot both be provided', 'INVALID_ARGUMENT');
    }
  }
  if (argumentsValue.remove_env !== undefined) {
    if (!Array.isArray(argumentsValue.remove_env)) throw new PolicyError('remove_env must be an array', 'INVALID_ARGUMENT');
    if (enforceResourceLimits && argumentsValue.remove_env.length > 64) throw new PolicyError('remove_env contains too many entries', 'INVALID_ARGUMENT');
    for (const raw of argumentsValue.remove_env) {
      if (typeof raw !== 'string') throw new PolicyError('remove_env entries must be strings', 'INVALID_ARGUMENT');
      environmentKey(raw);
    }
  }
}

function commandParts(argumentsValue: JsonObject, requireShellConfirmation = true, enforceWorkspaceBoundary = true, enforceResourceLimits = true): { command: string; executable: string; shell: string; argv: string[] } {
  const cmd = typeof argumentsValue.cmd === 'string' ? argumentsValue.cmd : undefined;
  const script = typeof argumentsValue.script === 'string' ? argumentsValue.script : undefined;
  const program = typeof argumentsValue.program === 'string' ? argumentsValue.program : undefined;
  if ([cmd, script, program].filter(value => value !== undefined).length !== 1) {
    throw new PolicyError('exec_command requires exactly one of cmd, script, or program', 'INVALID_ARGUMENT');
  }
  const shell = String(argumentsValue.shell ?? 'none').toLowerCase();
  if (!['none', 'cmd', 'powershell', 'sh'].includes(shell)) throw new PolicyError('shell must be none, cmd, powershell, or sh', 'INVALID_ARGUMENT');
  if (program !== undefined && shell !== 'none') throw new PolicyError('program/args mode requires shell=none', 'INVALID_ARGUMENT');
  if (script !== undefined && shell === 'none') throw new PolicyError('script mode requires shell=powershell, cmd, or sh', 'INVALID_ARGUMENT');
  if (requireShellConfirmation && shell !== 'none' && argumentsValue.confirm !== true) {
    throw new PolicyError('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: explicit shell execution requires confirm=true', 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
  }
  let argv: string[] = [];
  let executable: string;
  let command: string;
  if (program !== undefined) {
    if (!program.trim()) throw new PolicyError('exec_command requires a non-empty command', 'INVALID_ARGUMENT');
    if (argumentsValue.args !== undefined && !Array.isArray(argumentsValue.args)) throw new PolicyError('args must be an array', 'INVALID_ARGUMENT');
    argv = Array.isArray(argumentsValue.args) ? argumentsValue.args.map(value => {
      if (typeof value !== 'string') throw new PolicyError('args entries must be strings', 'INVALID_ARGUMENT');
      return value;
    }) : [];
    executable = program;
    command = [program, ...argv].join(' ');
  } else {
    command = (cmd ?? script ?? '');
    if (!command.trim()) throw new PolicyError('exec_command requires a non-empty command', 'INVALID_ARGUMENT');
    if (shell === 'none') {
      const parts = splitShellWords(command);
      if (!parts.length) throw new PolicyError('Empty command', 'INVALID_ARGUMENT');
      [executable, ...argv] = parts;
    } else {
      executable = shell;
    }
  }
  if (enforceResourceLimits && command.length > 64_000) throw new PolicyError('Command is too long', 'INVALID_ARGUMENT');
  if (enforceWorkspaceBoundary && String(argumentsValue.filesystem_scope ?? 'workspace') !== 'workspace') {
    throw new PolicyError('EXTERNAL_EXECUTION_NOT_ALLOWED: exec_command only allows Workspace execution', 'EXTERNAL_EXECUTION_NOT_ALLOWED');
  }
  for (const key of ['workdir', 'cwd']) {
    const value = argumentsValue[key];
    if (enforceWorkspaceBoundary && value !== undefined && (typeof value !== 'string' || !relativePathAllowed(value))) {
      throw new PolicyError('workdir must stay inside the configured workspace', 'PATH_OUTSIDE_WORKSPACE');
    }
  }
  if (shell === 'none' && cmd !== undefined && hasForbiddenShellSyntax(cmd)) {
    throw new PolicyError('Shell chaining, redirection and expansion require an explicit shell mode', 'SHELL_MODE_REQUIRED');
  }
  return { command, executable, shell, argv };
}

function executableStem(executable: string): string {
  const base = executable.trim().replace(/^\.\//, '').split(/[\\/]/).at(-1)?.toLowerCase() ?? '';
  return base.replace(/\.(?:exe|cmd|bat)$/i, '');
}

function normalizeWslAbsoluteProgramPath(value: string): string | undefined {
  if (!value.startsWith('/') || value.includes('\\') || value.includes('\0') || [...value].some(character => /[\u0000-\u001f\u007f]/.test(character))) {
    return undefined;
  }
  const segments: string[] = [];
  for (const segment of value.split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      if (!segments.length) return undefined;
      segments.pop();
    } else {
      segments.push(segment);
    }
  }
  return segments.length ? `/${segments.join('/')}` : '/';
}

function trustedWslSystemProgram(value: string): boolean {
  const parent = path.posix.dirname(value);
  const name = path.posix.basename(value);
  return Boolean(name) && [
    '/bin', '/sbin', '/usr/bin', '/usr/sbin',
    '/usr/local/bin', '/usr/local/sbin', '/snap/bin'
  ].includes(parent);
}

async function resolveWslAbsoluteProgram(raw: string, root: string, ctx: ToolContext): Promise<string> {
  const normalized = normalizeWslAbsoluteProgramPath(raw);
  const location = parseWslUncPath(root);
  if (!normalized || !location) throw new PolicyError(`Workspace external executable rejected: ${raw}`, 'EXECUTABLE_OUTSIDE_WORKSPACE');

  const hostCandidate = wslUncPath(location.distro, normalized);
  if (await insideWorkspace(hostCandidate, root)) {
    const extension = path.posix.extname(normalized).toLowerCase();
    if (ctx.config.policy.workspaceLocalEntries
      && (!extension || ctx.config.policy.workspaceScriptExtensions.includes(extension))) {
      return normalized;
    }
    throw new PolicyError(`Workspace local entry is not allowed: ${raw}`, 'COMMAND_REJECTED');
  }

  if (ctx.config.policy.allowedCommands.includes(executableStem(normalized)) && trustedWslSystemProgram(normalized)) {
    return normalized;
  }
  throw new PolicyError(`Workspace external executable rejected: ${raw}`, 'EXECUTABLE_OUTSIDE_WORKSPACE');
}

async function insideWorkspace(candidate: string, root: string): Promise<boolean> {
  try {
    const [resolved, resolvedRoot, info] = await Promise.all([realpath(candidate), realpath(root), stat(candidate)]);
    if (!info.isFile()) return false;
    relativeInside(resolvedRoot, resolved);
    return true;
  } catch {
    return false;
  }
}

function commandCwd(ctx: ToolContext, key: string, argumentsValue: JsonObject): { root: string; cwd: string } {
  const { root, cwd: defaultCwd } = rootAndCwd(ctx, key);
  const requested = String(argumentsValue.workdir ?? argumentsValue.cwd ?? relativeInside(root, defaultCwd));
  if (ctx.config.securityPolicy.enforceWorkspaceBoundary || parseWslUncPath(root)) {
    return { root, cwd: resolveInside(root, requested) };
  }
  return { root, cwd: path.isAbsolute(requested) ? path.resolve(requested) : path.resolve(defaultCwd, requested) };
}

async function workspaceCandidate(ctx: ToolContext, key: string, argumentsValue: JsonObject, executable: string): Promise<boolean> {
  if (!ctx.config.policy.workspaceLocalEntries) return false;
  const { root, cwd } = commandCwd(ctx, key, argumentsValue);
  let candidate: string;
  try {
    candidate = parseWslUncPath(executable)
      ? executable
      : resolveFromWorkspace(root, cwd, executable);
  } catch (error) {
    if (error instanceof WslRoutingError) throw error;
    return false;
  }
  if (await insideWorkspace(candidate, root)) return true;
  const base = path.basename(executable).toLowerCase();
  return (executable.includes('/') || executable.includes('\\'))
    && ctx.config.policy.workspaceScriptExtensions.some(extension => base.endsWith(extension));
}

async function validateCommand(ctx: ToolContext, key: string, argumentsValue: JsonObject): Promise<void> {
  const security = ctx.config.securityPolicy;
  const parts = commandParts(argumentsValue, security.requireShellConfirmation, security.enforceWorkspaceBoundary, security.enforceResourceLimits);
  const command = parts.command;
  const { root, cwd } = commandCwd(ctx, key, argumentsValue);
  if (parseWslUncPath(root)) {
    if (parts.shell === 'cmd') throw new PolicyError('shell=cmd is unavailable for WSL workspaces; use shell=sh', 'INVALID_ARGUMENT');
    if (parts.shell === 'powershell') throw new PolicyError('shell=powershell is unavailable for WSL workspaces; use shell=sh', 'INVALID_ARGUMENT');
    validateWslExecPaths(cwd, parts.executable, parts.argv);
  }
  if (security.protectRepositoryMetadata && (DANGEROUS_PATTERN.test(command) || INTERPRETER_MUTATION_PATTERN.test(command)) && commandTargetsProtectedRepositoryAsset(command)) {
    throw new PolicyError('PROTECTED_REPOSITORY_ASSET: deleting or recursively clearing .git/.github is forbidden', 'PROTECTED_REPOSITORY_ASSET');
  }
  if (security.enforceWorkspaceBoundary && ctx.config.sandbox?.enabled !== true && INTERPRETER_MUTATION_PATTERN.test(command) && commandContainsExternalPath(command)) {
    throw new PolicyError('WORKSPACE_PATH_PROTECTED: subprocess writes outside the Workspace are forbidden', 'WORKSPACE_PATH_PROTECTED');
  }
  if (security.requireDangerousConfirmation && DANGEROUS_PATTERN.test(command) && argumentsValue.confirm !== true) {
    throw new PolicyError('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: dangerous command requires confirm=true', 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
  }
  if (security.blockNetworkCommands && NETWORK_PATTERN.test(command)) {
    throw new PolicyError('Network-looking commands are blocked in safe permission mode', 'NETWORK_COMMAND_BLOCKED');
  }
  const stem = executableStem(parts.executable);
  const allowed = !security.enforceCommandAllowlist || ctx.config.policy.allowedCommands.includes(stem)
    || await workspaceCandidate(ctx, key, argumentsValue, parts.executable);
  if (!allowed) throw new PolicyError(`Command is not allowlisted: ${stem}`, 'COMMAND_REJECTED');
  validateEnvironment(argumentsValue, security.protectEnvironmentVariables, security.enforceResourceLimits);
  const commandTimeoutMaxMs = ctx.config.limits.commandTimeoutMaxMs ?? DEFAULT_COMMAND_TIMEOUT_MAX_MS;
  resolveProcessTimeout(argumentsValue, command, commandTimeoutMaxMs);
  if (argumentsValue.post_checks !== undefined) {
    if (!Array.isArray(argumentsValue.post_checks)) throw new PolicyError('post_checks must be an array', 'INVALID_ARGUMENT');
    if (security.enforceResourceLimits && argumentsValue.post_checks.length > 16) throw new PolicyError('post_checks supports at most 16 checks', 'INVALID_ARGUMENT');
    for (let index = 0; index < argumentsValue.post_checks.length; index += 1) {
      const value = argumentsValue.post_checks[index];
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new PolicyError(`post_checks[${index}] must be an object`, 'INVALID_ARGUMENT');
      if ('post_checks' in value) throw new PolicyError('nested post_checks are not allowed', 'INVALID_ARGUMENT');
      if ('job_timeout_ms' in value) throw new PolicyError('post_checks cannot use job_timeout_ms; use a separate managed command.', 'INVALID_ARGUMENT');
      try { await validateCommand(ctx, key, value as JsonObject); }
      catch (error) { throw new PolicyError(`post_checks[${index}] rejected: ${error instanceof Error ? error.message : String(error)}`, error instanceof PolicyError || error instanceof WslRoutingError ? error.code : 'POLICY_REJECTED'); }
    }
  }
}

function serializedBytes(value: unknown): number {
  return Buffer.byteLength(JSON.stringify(value));
}

function validateFormatFiles(argumentsValue: JsonObject, maxPatchBytes: number, enforceWorkspaceBoundary: boolean, requireWriteConfirmation: boolean, enforceResourceLimits: boolean): void {
  if (enforceResourceLimits && serializedBytes(argumentsValue) > maxPatchBytes * 4) throw new PolicyError('format_files payload is too large', 'PAYLOAD_TOO_LARGE');
  const mode = String(argumentsValue.mode ?? 'plan');
  const scope = String(argumentsValue.scope ?? 'files');
  if (!['plan', 'check', 'apply'].includes(mode)) throw new PolicyError('format_files mode must be plan, check, or apply', 'INVALID_ARGUMENT');
  if (!['files', 'changed', 'staged', 'project'].includes(scope)) throw new PolicyError('format_files scope must be files, changed, staged, or project', 'INVALID_ARGUMENT');
  const pathValues: unknown[] = [];
  if (argumentsValue.paths !== undefined) {
    if (!Array.isArray(argumentsValue.paths)) throw new PolicyError('format_files paths must be an array', 'INVALID_ARGUMENT');
    pathValues.push(...argumentsValue.paths);
  }
  if (argumentsValue.expected_sha256 !== undefined) {
    if (!argumentsValue.expected_sha256 || typeof argumentsValue.expected_sha256 !== 'object' || Array.isArray(argumentsValue.expected_sha256)) {
      throw new PolicyError('format_files expected_sha256 must be an object', 'INVALID_ARGUMENT');
    }
    pathValues.push(...Object.keys(argumentsValue.expected_sha256 as JsonObject));
  }
  for (const value of pathValues) {
    if (typeof value !== 'string' || !value.trim() || (enforceWorkspaceBoundary && !relativePathAllowed(value))) {
      throw new PolicyError('format_files paths must stay inside the configured workspace', 'PATH_OUTSIDE_WORKSPACE');
    }
  }
  for (const key of ['include_patterns', 'exclude_patterns']) {
    const values = argumentsValue[key];
    if (values === undefined) continue;
    if (!Array.isArray(values)) throw new PolicyError(`format_files ${key} must be an array`, 'INVALID_ARGUMENT');
    for (const value of values) {
      if (typeof value !== 'string' || (enforceWorkspaceBoundary && !relativePathAllowed(value))) throw new PolicyError(`format_files ${key} must stay inside the configured workspace`, 'PATH_OUTSIDE_WORKSPACE');
    }
  }
  if (requireWriteConfirmation && mode === 'apply' && scope === 'project' && argumentsValue.confirm !== true) {
    throw new PolicyError('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: project-wide formatting requires confirm=true', 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
  }
  if (requireWriteConfirmation && mode === 'apply' && Number(argumentsValue.max_files ?? 500) > 2_000 && argumentsValue.confirm !== true) {
    throw new PolicyError('DANGEROUS_OPERATION_REQUIRES_CONFIRMATION: formatting more than 2000 files requires confirm=true', 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
  }
}

export type CommandGraphCommand = JsonObject & { id: string };

export function normalizeCommandGraphCommands(value: unknown): CommandGraphCommand[] {
  if (!Array.isArray(value)) return [];
  return value.map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new Error(`commands[${index}] must be an object`);
    }
    const command = entry as JsonObject;
    const idValue = command.id ?? `command-${index + 1}`;
    if (typeof idValue !== 'string') throw new Error(`commands[${index}].id must be a string`);
    if (!idValue.trim() || idValue.length > 128) {
      throw new Error(`commands[${index}].id must contain between 1 and 128 characters`);
    }
    return { ...command, id: idValue };
  });
}

export function validateCommandGraphStructure(commands: CommandGraphCommand[], validateCycle: boolean): void {
  const ids = new Set<string>();
  for (const command of commands) {
    if (ids.has(command.id)) throw new Error(`duplicate exec_many command id: ${command.id}`);
    ids.add(command.id);
  }

  const dependencies = new Map<string, string[]>();
  for (let index = 0; index < commands.length; index += 1) {
    const command = commands[index];
    if (command.run_if !== undefined && (typeof command.run_if !== 'string' || !['success', 'failure', 'always'].includes(command.run_if))) {
      throw new Error(`commands[${index}].run_if must be success, failure, or always`);
    }
    if (command.depends_on !== undefined && !Array.isArray(command.depends_on)) {
      throw new Error(`commands[${index}].depends_on must be an array`);
    }
    const values = Array.isArray(command.depends_on) ? command.depends_on : [];
    const parsed = values.map(value => {
      if (typeof value !== 'string') throw new Error(`commands[${index}].depends_on must contain strings`);
      if (!value.trim() || value.length > 128) {
        throw new Error(`commands[${index}].depends_on values must contain between 1 and 128 characters`);
      }
      return value;
    });
    if ((command.run_if === 'failure' || command.run_if === 'always') && parsed.length === 0) {
      throw new Error(`commands[${index}].run_if=${command.run_if} requires depends_on`);
    }
    for (const dependency of parsed) {
      if (dependency === command.id) throw new Error(`command ${command.id} cannot depend on itself`);
      if (!ids.has(dependency)) throw new Error(`command ${command.id} depends on unknown command ${dependency}`);
    }
    dependencies.set(command.id, parsed);
  }

  if (!validateCycle) return;
  const indegree = new Map(commands.map(command => [command.id, dependencies.get(command.id)?.length ?? 0]));
  const dependents = new Map<string, string[]>();
  for (const [id, commandDependencies] of dependencies) {
    for (const dependency of commandDependencies) {
      const values = dependents.get(dependency) ?? [];
      values.push(id);
      dependents.set(dependency, values);
    }
  }
  const ready = [...indegree].filter(([, degree]) => degree === 0).map(([id]) => id);
  let visited = 0;
  while (ready.length) {
    const id = ready.pop()!;
    visited += 1;
    for (const dependent of dependents.get(id) ?? []) {
      const degree = (indegree.get(dependent) ?? 0) - 1;
      indegree.set(dependent, degree);
      if (degree === 0) ready.push(dependent);
    }
  }
  if (visited !== commands.length) throw new Error('exec_many DAG contains a dependency cycle');
}

export async function validateToolPolicy(ctx: ToolContext, key: string, toolName: string, argumentsValue: JsonObject): Promise<void> {
  const max = ctx.config.policy.maxPatchBytes;
  const security = ctx.config.securityPolicy;
  switch (toolName) {
    case 'exec_command':
      await validateCommand(ctx, key, argumentsValue);
      return;
    case 'exec_many': {
      const operationId = typeof argumentsValue.operation_id === 'string' ? argumentsValue.operation_id.trim() : '';
      const action = String(argumentsValue.action ?? 'run').trim().toLowerCase();
      if (!['run', 'status', 'cancel', 'forget'].includes(action)) {
        throw new PolicyError('exec_many action must be run, status, cancel, or forget', 'INVALID_ARGUMENT');
      }
      const resultMode = String(argumentsValue.result_mode ?? '').trim().toLowerCase();
      if (resultMode && !['full', 'summary', 'none'].includes(resultMode)) {
        throw new PolicyError('exec_many result_mode must be full, summary, or none', 'INVALID_ARGUMENT');
      }
      if (argumentsValue.operation_id !== undefined && (!operationId || operationId.length > 128)) {
        throw new PolicyError('exec_many operation_id must contain between 1 and 128 characters', 'INVALID_ARGUMENT');
      }
      if (action !== 'run') {
        if (!operationId) throw new PolicyError(`exec_many action=${action} requires operation_id`, 'INVALID_ARGUMENT');
        if (argumentsValue.commands !== undefined) throw new PolicyError(`exec_many action=${action} does not accept commands`, 'INVALID_ARGUMENT');
        return;
      }
      if (argumentsValue.commands === undefined) {
        if (!operationId) throw new PolicyError('exec_many requires commands or operation_id', 'INVALID_ARGUMENT');
        return;
      }
      if (!Array.isArray(argumentsValue.commands) || !argumentsValue.commands.length) throw new PolicyError('commands must be a non-empty array', 'INVALID_ARGUMENT');
      if (security.enforceResourceLimits && argumentsValue.commands.length > 256) throw new PolicyError('exec_many supports at most 256 commands', 'INVALID_ARGUMENT');
      let commands: CommandGraphCommand[];
      try {
        commands = normalizeCommandGraphCommands(argumentsValue.commands);
        const hasDependencies = commands.some(command => Array.isArray(command.depends_on) && command.depends_on.length > 0);
        validateCommandGraphStructure(commands, hasDependencies);
        const requestedMode = String(argumentsValue.mode ?? 'auto');
        const effectiveMode = requestedMode === 'auto' ? (hasDependencies ? 'dag' : 'sequential') : requestedMode;
        if (effectiveMode !== 'dag' && commands.some(command => command.run_if === 'failure' || command.run_if === 'always')) {
          throw new Error('run_if=failure or run_if=always requires dag mode (or auto with dependencies)');
        }
      } catch (error) {
        throw new PolicyError(error instanceof Error ? error.message : String(error), 'INVALID_ARGUMENT');
      }
      for (let index = 0; index < commands.length; index += 1) {
        const child: JsonObject = { ...commands[index] };
        delete child.id;
        delete child.depends_on;
        delete child.run_if;
        try { await validateCommand(ctx, key, child); }
        catch (error) { throw new PolicyError(`commands[${index}] rejected: ${error instanceof Error ? error.message : String(error)}`, error instanceof PolicyError || error instanceof WslRoutingError ? error.code : 'POLICY_REJECTED'); }
      }
      return;
    }
    case 'apply_patch':
    case 'patch_check': {
      if (typeof argumentsValue.patch !== 'string' || !argumentsValue.patch.trim()) throw new PolicyError('apply_patch requires a patch', 'INVALID_ARGUMENT');
      if (security.enforceResourceLimits && Buffer.byteLength(argumentsValue.patch) > max) throw new PolicyError('Patch is too large', 'PAYLOAD_TOO_LARGE');
      return;
    }
    case 'edit': {
      const files = Array.isArray(argumentsValue.files) ? argumentsValue.files : [];
      if (!files.length || (security.enforceResourceLimits && files.length > 100)) throw new PolicyError('edit requires between 1 and 100 files', 'INVALID_ARGUMENT');
      for (let index = 0; index < files.length; index += 1) {
        const file = files[index];
        if (!file || typeof file !== 'object' || Array.isArray(file)) throw new PolicyError(`edit files[${index}] must be an object`, 'INVALID_ARGUMENT');
        const item = file as JsonObject;
        if (typeof item.path !== 'string' || !item.path.trim()) throw new PolicyError(`edit files[${index}].path is required`, 'INVALID_ARGUMENT');
        if (security.enforceResourceLimits && Array.isArray(item.edits) && item.edits.length > 100) {
          throw new PolicyError(`edit files[${index}].edits supports at most 100 operations`, 'INVALID_ARGUMENT');
        }
      }
      if (security.enforceResourceLimits && serializedBytes(argumentsValue) > max * 4) throw new PolicyError('edit payload is too large', 'PAYLOAD_TOO_LARGE');
      return;
    }
    case 'edit_file': {
      if (typeof argumentsValue.path !== 'string' || !argumentsValue.path.trim()) throw new PolicyError('edit_file requires a non-empty path', 'INVALID_ARGUMENT');
      const edits = Array.isArray(argumentsValue.edits) && argumentsValue.edits.length > 0;
      const proposal = Boolean(argumentsValue.apply_proposal && typeof argumentsValue.apply_proposal === 'object' && !Array.isArray(argumentsValue.apply_proposal));
      if (!edits && !proposal) throw new PolicyError('edit_file requires non-empty edits or apply_proposal', 'INVALID_ARGUMENT');
      if (security.enforceResourceLimits && serializedBytes(argumentsValue) > max) throw new PolicyError('Edit payload is too large', 'PAYLOAD_TOO_LARGE');
      return;
    }
    case 'edit_many':
    case 'file_ops':
      if (security.enforceResourceLimits && serializedBytes(argumentsValue) > max * 4) throw new PolicyError(`${toolName} payload is too large`, 'PAYLOAD_TOO_LARGE');
      return;
    case 'format_files':
      validateFormatFiles(argumentsValue, max, security.enforceWorkspaceBoundary, security.requireWriteConfirmation, security.enforceResourceLimits);
      return;
    default:
      return;
  }
}

function shellCommand(shell: string, script: string, wslWorkspace: boolean): ResolvedCommandSpec {
  if (wslWorkspace) {
    if (shell !== 'sh') throw new PolicyError(`shell=${shell} is unavailable for WSL workspaces; use shell=sh`, 'INVALID_ARGUMENT');
    return { program: 'sh', argv: ['-c', script], display: script, shell: false };
  }
  if (process.platform === 'win32') {
    if (shell === 'cmd') return { program: 'cmd.exe', argv: ['/d', '/s', '/c', script], display: script, shell: false };
    if (shell === 'sh') return { program: 'sh.exe', argv: ['-lc', script], display: script, shell: false };
    return { program: 'powershell.exe', argv: ['-NoProfile', '-NonInteractive', '-Command', script], display: script, shell: false };
  }
  if (shell === 'powershell') return { program: 'pwsh', argv: ['-NoProfile', '-NonInteractive', '-Command', script], display: script, shell: false };
  if (shell === 'cmd') throw new PolicyError('cmd shell is unavailable on this platform', 'COMMAND_REJECTED');
  return { program: '/bin/sh', argv: ['-lc', script], display: script, shell: false };
}

function windowsOnlySandboxProgram(program: string): boolean {
  const normalized = program.trim().toLowerCase();
  return /\.(?:exe|cmd|bat|ps1)$/.test(normalized);
}

export async function resolvePortableCommandSpec(ctx: ToolContext, key: string, argumentsValue: JsonObject): Promise<ResolvedCommandSpec> {
  const security = ctx.config.securityPolicy;
  const parts = commandParts(argumentsValue, security.requireShellConfirmation, security.enforceWorkspaceBoundary, security.enforceResourceLimits);
  const { root, cwd } = commandCwd(ctx, key, argumentsValue);
  if (parts.shell !== 'none') {
    if (parts.shell !== 'sh') {
      throw new PolicyError(`shell=${parts.shell} is unavailable inside a Linux sandbox; use shell=sh`, 'SANDBOX_COMMAND_UNSUPPORTED');
    }
    return { program: 'sh', argv: ['-c', parts.command], display: parts.command, shell: false };
  }

  const raw = parts.executable.trim();
  if (windowsOnlySandboxProgram(raw)) {
    throw new PolicyError(`Windows host executable cannot run inside a Linux sandbox: ${raw}`, 'SANDBOX_COMMAND_UNSUPPORTED');
  }
  if (raw.startsWith('/') && !raw.startsWith('//')) {
    return { program: raw, argv: parts.argv, display: [raw, ...parts.argv].join(' '), shell: false };
  }
  const explicit = path.isAbsolute(raw) || raw.includes('/') || raw.includes('\\');
  if (!explicit) return { program: raw, argv: parts.argv, display: [raw, ...parts.argv].join(' '), shell: false };

  const candidate = resolveFromWorkspace(root, cwd, raw);
  let resolved: string;
  try {
    await access(candidate);
    resolved = await realpath(candidate);
  } catch {
    throw new PolicyError(`Program not found: ${raw}`, 'COMMAND_REJECTED');
  }
  if (!await insideWorkspace(resolved, root)) {
    throw new PolicyError(`Workspace external executable rejected: ${raw}`, 'EXECUTABLE_OUTSIDE_WORKSPACE');
  }
  if (windowsOnlySandboxProgram(resolved)) {
    throw new PolicyError(`Windows host executable cannot run inside a Linux sandbox: ${raw}`, 'SANDBOX_COMMAND_UNSUPPORTED');
  }
  return { program: resolved, argv: parts.argv, display: [raw, ...parts.argv].join(' '), shell: false };
}

export async function resolveCommandSpec(ctx: ToolContext, key: string, argumentsValue: JsonObject): Promise<ResolvedCommandSpec> {
  const security = ctx.config.securityPolicy;
  const parts = commandParts(argumentsValue, security.requireShellConfirmation, security.enforceWorkspaceBoundary, security.enforceResourceLimits);
  const { root, cwd } = commandCwd(ctx, key, argumentsValue);
  const wslWorkspace = Boolean(parseWslUncPath(root));
  if (parts.shell !== 'none') {
    const spec = shellCommand(parts.shell, parts.command, wslWorkspace);
    validateWslExecPaths(cwd, spec.program, spec.argv);
    return spec;
  }
  const raw = parts.executable.trim();
  validateWslExecPaths(cwd, raw, parts.argv);
  if (wslWorkspace && raw.startsWith('/')) {
    const program = await resolveWslAbsoluteProgram(raw, root, ctx);
    return { program, argv: parts.argv, display: [raw, ...parts.argv].join(' '), shell: false };
  }
  const explicit = path.isAbsolute(raw) || raw.includes('/') || raw.includes('\\');
  if (!explicit) return { program: raw, argv: parts.argv, display: [raw, ...parts.argv].join(' '), shell: false };
  const candidate = security.enforceWorkspaceBoundary || wslWorkspace ? resolveFromWorkspace(root, cwd, raw) : path.resolve(cwd, raw);
  let resolved: string;
  try {
    await access(candidate);
    resolved = await realpath(candidate);
  } catch {
    throw new PolicyError(`Program not found: ${raw}`, 'COMMAND_REJECTED');
  }
  if (security.enforceWorkspaceBoundary && !await insideWorkspace(resolved, root)) throw new PolicyError(`Workspace external executable rejected: ${raw}`, 'EXECUTABLE_OUTSIDE_WORKSPACE');
  const extension = (wslWorkspace ? path.posix.extname(raw.replaceAll('\\', '/')) : path.extname(resolved)).toLowerCase();
  if (security.enforceCommandAllowlist && (!ctx.config.policy.workspaceLocalEntries || (extension && !ctx.config.policy.workspaceScriptExtensions.includes(extension)))) {
    throw new PolicyError(`Workspace local entry is not allowed: ${raw}`, 'COMMAND_REJECTED');
  }
  return { program: resolved, argv: parts.argv, display: [raw, ...parts.argv].join(' '), shell: false };
}
