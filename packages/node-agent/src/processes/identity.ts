import { createHash } from 'node:crypto';
import path from 'node:path';
import type { JsonObject, SandboxConfig } from '../types.js';
import { normalizedSandboxConfig } from '../sandbox.js';

export interface ProcessCommandIdentitySpec {
  program: string;
  argv: string[];
  display: string;
  shell: boolean;
}

function stableValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, nested]) => [key, stableValue(nested)]));
  }
  return value;
}

function sandboxFingerprintMaterial(config: SandboxConfig | undefined): unknown {
  const normalized = normalizedSandboxConfig(config);
  if (!normalized.enabled) return null;
  return stableValue({
    backend: normalized.backend,
    external_paths: normalized.externalPaths,
    options: normalized.options
  });
}

export function commandFingerprint(
  cwd: string,
  spec: ProcessCommandIdentitySpec,
  args: JsonObject,
  timeoutMs: number,
  sandboxConfig?: SandboxConfig,
  resolvedSecrets?: { environment: Array<[string, string, string]>; stdinSha256: string | null; stdin: string }
): string {
  const env = Object.fromEntries(Object.entries((args.env as Record<string, unknown> | undefined) ?? {})
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, value]) => [name, String(value)]));
  const removeEnv = Array.isArray(args.remove_env) ? args.remove_env.map(String).sort() : [];
  const stdin = resolvedSecrets?.stdin ?? (typeof args.stdin === 'string' ? args.stdin : '');
  const material = stableValue({
    cwd,
    program: spec.program,
    argv: spec.argv,
    shell: spec.shell,
    env,
    secret_env: resolvedSecrets?.environment ?? [],
    remove_env: removeEnv,
    timeout_ms: timeoutMs,
    ...(Object.hasOwn(args, 'job_timeout_ms') ? { execution_mode: 'job' } : {}),
    tty: args.tty === true,
    stdin_sha256: createHash('sha256').update(stdin).digest('hex'),
    post_checks: Array.isArray(args.post_checks) ? args.post_checks.slice(0, 16) : [],
    resource_lock_group: String(args.lock_group ?? '').trim() || null,
    sandbox: sandboxFingerprintMaterial(sandboxConfig)
  });
  return createHash('sha256').update(JSON.stringify(material)).digest('hex');
}

export function safeAutomaticDedup(spec: ProcessCommandIdentitySpec): boolean {
  const executable = (spec.program.split(/[\\/]/).at(-1) ?? '').toLowerCase().replace(/\.exe$/, '').replace(/\.cmd$/, '');
  if (executable !== 'cargo') return false;
  return ['check', 'test', 'build', 'fmt', 'clippy'].includes(spec.argv[0] ?? '');
}

function commandArgumentValue(argv: string[], name: string): string | undefined {
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index] ?? '';
    if (argument === name) return argv[index + 1];
    if (argument.startsWith(`${name}=`)) return argument.slice(name.length + 1);
  }
  return undefined;
}

function isCargoCommand(spec: ProcessCommandIdentitySpec): boolean {
  const executable = (spec.program.split(/[\\/]/).at(-1) ?? '').toLowerCase().replace(/\.exe$/, '').replace(/\.cmd$/, '');
  const display = spec.display.toLowerCase();
  return executable === 'cargo' || display.includes('cargo ') || display.includes('tauri build');
}

function executableName(spec: ProcessCommandIdentitySpec): string {
  return (spec.program.split(/[\\/]/).at(-1) ?? '').toLowerCase().replace(/\.exe$/, '').replace(/\.cmd$/, '');
}

export function nodeGeneratedLock(
  cwd: string,
  spec: ProcessCommandIdentitySpec
): { group: string; target: string } | undefined {
  const executable = executableName(spec);
  const commandText = `${spec.display}\n${spec.argv.join('\n')}`.toLowerCase().replaceAll('\\', '/');
  const cwdNormalized = path.resolve(cwd);
  const cwdSlashes = cwdNormalized.replaceAll('\\', '/').toLowerCase();
  const cwdIsPackage = cwdSlashes.endsWith('/packages/node-agent');
  const referencesPackage = cwdIsPackage
    || commandText.includes('@coding-tools/node-agent')
    || commandText.includes('packages/node-agent/');
  if (!referencesPackage) return undefined;

  const packageManager = ['pnpm', 'npm', 'yarn', 'bun'].includes(executable);
  const nodeConsumer = executable === 'node'
    && (commandText.includes('packages/node-agent/test/') || commandText.includes('packages/node-agent/dist/'));
  const packageBuild = packageManager
    && (commandText.includes('build:server') || commandText.includes('node-agent:build'));
  if (!packageBuild && !nodeConsumer) return undefined;

  const packageRoot = cwdIsPackage ? cwdNormalized : path.resolve(cwdNormalized, 'packages/node-agent');
  const target = path.resolve(packageRoot, 'dist');
  const digest = createHash('sha256').update(target).digest('hex');
  return { group: `node-generated:${digest.slice(0, 24)}`, target };
}

export function cargoTargetLock(
  cwd: string,
  spec: ProcessCommandIdentitySpec,
  args: JsonObject
): { group: string; target: string } | undefined {
  if (!isCargoCommand(spec)) return undefined;
  const env = (args.env as Record<string, unknown> | undefined) ?? {};
  const envTarget = Object.entries(env).find(([key]) => key.toLowerCase() === 'cargo_target_dir')?.[1];
  const configuredTarget = commandArgumentValue(spec.argv, '--target-dir') ?? (envTarget === undefined ? undefined : String(envTarget));
  let target: string;
  if (configuredTarget) {
    target = path.isAbsolute(configuredTarget) ? configuredTarget : path.join(cwd, configuredTarget);
  } else {
    const manifest = commandArgumentValue(spec.argv, '--manifest-path');
    if (manifest) {
      const manifestPath = path.isAbsolute(manifest) ? manifest : path.join(cwd, manifest);
      target = path.join(path.dirname(manifestPath), 'target');
    } else if (spec.display.toLowerCase().includes('tauri') && path.basename(cwd).toLowerCase() !== 'src-tauri') {
      target = path.join(cwd, 'src-tauri', 'target');
    } else {
      target = path.join(cwd, 'target');
    }
  }
  const normalizedTarget = path.resolve(target);
  const digest = createHash('sha256').update(normalizedTarget).digest('hex');
  return { group: `cargo-target:${digest.slice(0, 24)}`, target: normalizedTarget };
}
