import type { JsonObject, SecurityPolicy } from './types.js';

export const REDACTED = '[REDACTED]';
const RESULT_WARNING = 'Sensitive values were automatically redacted from the tool result.';
const PROCESS_WARNING = 'Sensitive process output was withheld because the command referenced a protected credential source.';

interface RedactionState {
  count: number;
}

const privateKeyPattern = /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g;
const jwtPattern = /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/g;
const knownTokenPattern = /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16})\b/g;
const bearerPattern = /(bearer\s+)[A-Za-z0-9._~+/=-]+/gi;
const basicAuthUrlPattern = /(https?:\/\/[^\s:/@]+:)[^\s/@]+(@)/gi;
const jsonQuotedPattern = /("(?:password|passwd|secret|token|api[-_]?key|authorization|credential|private[-_]?key|client[-_]?secret|bearer[-_]?token|oauth[-_]?password|oauth[-_]?token[-_]?secret)"\s*:\s*)"(?:\\.|[^"\\])*"/gi;
const secretFlagPattern = /(--?(?:password|passwd|secret|token|api[-_]?key|authorization|credential|private[-_]?key|client[-_]?secret)(?:\s+|=))(?:(?:"[^"]*")|(?:'[^']*')|[^\s]+)/gi;
const keyValuePattern = /\b((?:password|passwd|secret|token|api[-_]?key|authorization|credential|private[-_]?key|client[-_]?secret|bearer[-_]?token|oauth[-_]?password|oauth[-_]?token[-_]?secret)\s*[:=]\s*)(?:(?:"(?:\\.|[^"\\])*")|(?:'[^']*')|[^\s;,}\]]+)/gi;

function isRecord(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function replaceCount(
  input: string,
  pattern: RegExp,
  replacement: string | ((...values: string[]) => string),
  state: RedactionState
): string {
  const regex = new RegExp(pattern.source, pattern.flags);
  return input.replace(regex, (...args: unknown[]) => {
    state.count += 1;
    if (typeof replacement === 'string') return replacement;
    return replacement(...args.slice(0, -2).map(String));
  });
}

export function containsSensitivePath(value: string): boolean {
  const normalized = value.replaceAll('\\', '/').toLowerCase()
    .replaceAll('.env.example', '')
    .replaceAll('.env.sample', '')
    .replaceAll('.env.template', '');
  const namedSecret = [
    'profiles.json', 'secrets.json', 'secret.json', '.npmrc', '.pypirc', '.netrc',
    '/credentials', 'credentials.json', 'service-account.json', 'service_account.json',
    'id_rsa', 'id_ed25519', '.pem', '.p12', '.pfx', '.key', '/.ssh/',
    '/.aws/credentials', '/keyring'
  ].some(needle => normalized.includes(needle));
  const envFile = normalized.includes('/.env')
    || normalized.includes(' .env')
    || normalized.includes("' .env")
    || normalized.endsWith('.env')
    || normalized.includes('".env"');
  return namedSecret || envFile;
}

function referencesSecretInput(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(referencesSecretInput);
  if (!isRecord(value)) return false;
  if (typeof value.stdin_secret === 'string' && value.stdin_secret.trim()) return true;
  if (isRecord(value.secret_env) && Object.keys(value.secret_env).length > 0) return true;
  return Object.values(value).some(referencesSecretInput);
}

export function argumentsReferenceSensitiveSource(argumentsValue: unknown): boolean {
  if (referencesSecretInput(argumentsValue)) return true;
  try {
    return containsSensitivePath(JSON.stringify(argumentsValue));
  } catch {
    return false;
  }
}

export function isSensitiveKey(key: string): boolean {
  const normalized = key.trim().toLowerCase().replace(/[-. ]/g, '_');
  if (['_count', '_bytes', '_duration', '_duration_ms', '_length', '_present', '_available']
    .some(suffix => normalized.endsWith(suffix))) return false;
  return normalized === 'stdin'
    || normalized === 'chars'
    || normalized === 'authorization'
    || normalized === 'cookie'
    || normalized === 'set_cookie'
    || normalized === 'credential'
    || normalized === 'credentials'
    || normalized === 'api_key'
    || normalized === 'apikey'
    || normalized === 'private_key'
    || normalized === 'client_secret'
    || normalized === 'shared_secrets'
    || normalized === 'workspace_secrets'
    || normalized === 'app_secrets'
    || normalized.includes('password')
    || normalized.includes('passwd')
    || normalized.includes('private_key')
    || normalized.endsWith('_secret')
    || normalized.startsWith('secret_')
    || normalized === 'secret'
    || normalized.endsWith('_token')
    || normalized.startsWith('token_')
    || normalized === 'token'
    || normalized.endsWith('_credential')
    || normalized.endsWith('_credentials');
}

export function redactSensitiveText(value: string): { value: string; count: number } {
  const state: RedactionState = { count: 0 };
  let redacted = replaceCount(value, privateKeyPattern, REDACTED, state);
  redacted = replaceCount(redacted, jwtPattern, REDACTED, state);
  redacted = replaceCount(redacted, knownTokenPattern, REDACTED, state);
  redacted = replaceCount(redacted, bearerPattern, (_match, prefix) => `${prefix}${REDACTED}`, state);
  redacted = replaceCount(redacted, basicAuthUrlPattern, (_match, prefix, suffix) => `${prefix}${REDACTED}${suffix}`, state);
  redacted = replaceCount(redacted, jsonQuotedPattern, (_match, prefix) => `${prefix}"${REDACTED}"`, state);
  redacted = replaceCount(redacted, secretFlagPattern, (_match, prefix) => `${prefix}${REDACTED}`, state);
  redacted = replaceCount(redacted, keyValuePattern, (_match, prefix) => `${prefix}${REDACTED}`, state);
  return { value: redacted, count: state.count };
}

function redactValue(value: unknown, key: string | undefined, state: RedactionState): unknown {
  if (key && isSensitiveKey(key)) {
    if (value !== REDACTED) state.count += 1;
    return REDACTED;
  }
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index += 1) value[index] = redactValue(value[index], undefined, state);
    return value;
  }
  if (isRecord(value)) {
    for (const [childKey, childValue] of Object.entries(value)) value[childKey] = redactValue(childValue, childKey, state);
    return value;
  }
  if (typeof value === 'string') {
    const redacted = redactSensitiveText(value);
    state.count += redacted.count;
    return redacted.value;
  }
  return value;
}

function redactNamedFields(value: unknown, fields: ReadonlySet<string>, state: RedactionState): void {
  if (Array.isArray(value)) {
    for (const child of value) redactNamedFields(child, fields, state);
    return;
  }
  if (!isRecord(value)) return;
  for (const [key, child] of Object.entries(value)) {
    if (fields.has(key)) {
      if (child !== null && child !== REDACTED) {
        value[key] = REDACTED;
        state.count += 1;
      }
    } else {
      redactNamedFields(child, fields, state);
    }
  }
}

function redactPathScopedFields(value: unknown, fields: ReadonlySet<string>, state: RedactionState): void {
  if (Array.isArray(value)) {
    for (const child of value) redactPathScopedFields(child, fields, state);
    return;
  }
  if (!isRecord(value)) return;
  if (typeof value.path === 'string' && containsSensitivePath(value.path)) {
    for (const field of fields) {
      const child = value[field];
      if (child !== undefined && child !== null && child !== REDACTED) {
        value[field] = REDACTED;
        state.count += 1;
      }
    }
  }
  for (const child of Object.values(value)) redactPathScopedFields(child, fields, state);
}

function valueContainsSensitivePath(value: unknown): boolean {
  try {
    return containsSensitivePath(JSON.stringify(value));
  } catch {
    return false;
  }
}

function appendWarning(value: JsonObject, warning: string): void {
  if (value.warnings === undefined) value.warnings = [];
  if (!Array.isArray(value.warnings)) return;
  if (!value.warnings.includes(warning)) value.warnings.push(warning);
}

function redactSensitiveSourceOutput(toolName: string, sensitiveSource: boolean, value: unknown, state: RedactionState): void {
  if ((toolName === 'exec_command' || toolName === 'exec_many') && sensitiveSource) {
    redactNamedFields(value, new Set(['stdout', 'stderr', 'data']), state);
  } else if (toolName === 'read_file' && sensitiveSource) {
    redactNamedFields(value, new Set(['content']), state);
  } else if (toolName === 'read_many') {
    redactPathScopedFields(value, new Set(['content']), state);
  } else if (toolName === 'search_text') {
    redactPathScopedFields(value, new Set(['match', 'preview', 'before', 'after', 'content']), state);
  } else if (toolName === 'git_diff' && (sensitiveSource || valueContainsSensitivePath(value))) {
    redactNamedFields(value, new Set(['diff']), state);
  } else if (toolName === 'git_show' && (sensitiveSource || valueContainsSensitivePath(value))) {
    redactNamedFields(value, new Set(['content']), state);
  } else if (toolName === 'git_blame' && sensitiveSource) {
    redactNamedFields(value, new Set(['content', 'line', 'text']), state);
  }
}

export class OutputRedactionContext {
  readonly toolName: string;
  readonly sensitiveSource: boolean;
  readonly redactValues: boolean;
  readonly withholdSources: boolean;

  constructor(toolName: string, argumentsValue: unknown, policy?: Pick<SecurityPolicy, 'redactSensitiveOutput' | 'withholdSensitiveSourceOutput'>) {
    this.toolName = toolName;
    this.sensitiveSource = argumentsReferenceSensitiveSource(argumentsValue);
    this.redactValues = policy?.redactSensitiveOutput ?? true;
    this.withholdSources = policy?.withholdSensitiveSourceOutput ?? true;
  }

  redact<T>(value: T): T {
    if (!this.redactValues && !this.withholdSources) return value;
    const state: RedactionState = { count: 0 };
    if (this.withholdSources) redactSensitiveSourceOutput(this.toolName, this.sensitiveSource, value, state);
    const redacted = (this.redactValues ? redactValue(value, undefined, state) : value) as T;
    if (state.count > 0 && isRecord(redacted)) {
      const object = redacted as JsonObject;
      object.sensitive_data_redacted = true;
      object.redaction_count = state.count;
      appendWarning(object, RESULT_WARNING);
    }
    return redacted;
  }
}

export function redactToolOutput<T>(toolName: string, argumentsValue: unknown, value: T, policy?: Pick<SecurityPolicy, 'redactSensitiveOutput' | 'withholdSensitiveSourceOutput'>): T {
  return new OutputRedactionContext(toolName, argumentsValue, policy).redact(value);
}

export function processRedactionWarning(): string {
  return PROCESS_WARNING;
}
