import { createHash } from 'node:crypto';
import type { JsonObject, ToolContext } from '../types.js';
import { ProcessToolError } from './errors.js';

export interface ResolvedSecretInputs {
  environment: Array<[string, string]>;
  stdin: string;
  fingerprint: {
    environment: Array<[string, string, string]>;
    stdinSha256: string | null;
  };
}

function secretReference(value: unknown): string {
  const reference = typeof value === 'string' ? value.trim() : '';
  if (!/^[A-Za-z0-9._-]{1,128}$/.test(reference)) {
    throw new ProcessToolError('INVALID_ARGUMENT', 'Secret references must use 1-128 letters, numbers, dot, underscore, or hyphen.', 'validation');
  }
  return reference;
}

function resolveSecret(ctx: ToolContext, reference: string): string {
  const value = ctx.resolveSecret?.(reference);
  if (typeof value !== 'string' || value.length === 0) {
    throw new ProcessToolError('SECRET_NOT_FOUND', `Workspace secret is not configured: ${reference}`, 'validation', false, {
      reference,
      suggestion: 'Store the secret locally in the selected workspace, then retry using the same secret reference.'
    });
  }
  return value;
}

export function resolveSecretInputs(ctx: ToolContext, args: JsonObject): ResolvedSecretInputs {
  const directEnvironment = new Set(explicitEnvironment(args).map(([name]) => name.toLowerCase()));
  const removed = new Set(removedEnvironment(args).map(name => name.toLowerCase()));
  const environment: Array<[string, string]> = [];
  const fingerprintEnvironment: Array<[string, string, string]> = [];
  const requested = (args.secret_env as Record<string, unknown> | undefined) ?? {};
  for (const [name, rawReference] of Object.entries(requested)) {
    const normalizedName = name.toLowerCase();
    if (directEnvironment.has(normalizedName) || removed.has(normalizedName)) {
      throw new ProcessToolError('INVALID_ARGUMENT', `Environment variable ${name} cannot be configured by both env/remove_env and secret_env.`, 'validation');
    }
    const reference = secretReference(rawReference);
    const value = resolveSecret(ctx, reference);
    environment.push([name, value]);
    fingerprintEnvironment.push([name, reference, createHash('sha256').update(value).digest('hex')]);
  }
  fingerprintEnvironment.sort(([left], [right]) => left.localeCompare(right));

  const directStdin = typeof args.stdin === 'string' ? args.stdin : '';
  const rawStdinReference = args.stdin_secret;
  if (rawStdinReference !== undefined && directStdin.length > 0) {
    throw new ProcessToolError('INVALID_ARGUMENT', 'stdin and stdin_secret cannot both be provided.', 'validation');
  }
  if (rawStdinReference === undefined) {
    return {
      environment,
      stdin: directStdin,
      fingerprint: { environment: fingerprintEnvironment, stdinSha256: null }
    };
  }
  const stdinReference = secretReference(rawStdinReference);
  const stdin = resolveSecret(ctx, stdinReference);
  return {
    environment,
    stdin,
    fingerprint: {
      environment: fingerprintEnvironment,
      stdinSha256: createHash('sha256').update(stdin).digest('hex')
    }
  };
}

export function commandEnvironment(args: JsonObject, secretEnvironment: Array<[string, string]> = []): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = { ...process.env };
  for (const name of removedEnvironment(args)) delete environment[name];
  for (const [name, value] of explicitEnvironment(args, secretEnvironment)) environment[name] = value;
  return environment;
}

export function explicitEnvironment(args: JsonObject, secretEnvironment: Array<[string, string]> = []): Array<[string, string]> {
  return [
    ...Object.entries((args.env as Record<string, unknown> | undefined) ?? {})
      .map(([name, value]) => [name, String(value)] as [string, string]),
    ...secretEnvironment
  ];
}

export function removedEnvironment(args: JsonObject): string[] {
  return Array.isArray(args.remove_env) ? args.remove_env.map(String) : [];
}
