import { createHash, createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import type { IncomingHttpHeaders, ServerResponse } from 'node:http';
import type { AgentConfig } from './types.js';
import { generateAuthorizationPassword } from './secrets.js';

interface PendingCode {
  challenge: string;
  clientId: string;
  redirectUri: string;
  state: string;
  expiresAt: number;
  issuer: string;
}

interface HtmlResponse {
  status: number;
  body: string;
  location?: string;
}

interface TokenResponse {
  status: number;
  body: unknown;
}

const allowedOrigins = new Set(['https://chatgpt.com', 'https://chat.openai.com']);
const codeTtlMs = 5 * 60_000;
export const DEFAULT_OAUTH_TOKEN_TTL_SECONDS = 7 * 24 * 60 * 60;
export const MAX_OAUTH_TOKEN_TTL_SECONDS = 30 * 24 * 60 * 60;
const loginFailureWindowMs = 60_000;
const loginBlockMs = 60_000;
const maxLoginFailures = 5;

function base64url(value: Buffer | string): string {
  return Buffer.from(value).toString('base64url');
}

function constantTimeEqual(left: string, right: string): boolean {
  const a = Buffer.from(left);
  const b = Buffer.from(right);
  return a.length === b.length && timingSafeEqual(a, b);
}

function requireConfiguredSecret(value: string | undefined, label: string): string {
  if (!value || !value.trim()) throw new Error(`${label} is not configured`);
  return value;
}

function firstHeader(headers: IncomingHttpHeaders, name: string): string {
  const value = headers[name];
  return String(Array.isArray(value) ? value[0] ?? '' : value ?? '').split(',')[0].trim();
}

function forwardedHeaderParam(headers: IncomingHttpHeaders, name: string): string {
  const forwarded = firstHeader(headers, 'forwarded');
  for (const part of forwarded.split(';')) {
    const separator = part.indexOf('=');
    if (separator < 0 || part.slice(0, separator).trim().toLowerCase() !== name.toLowerCase()) continue;
    return part.slice(separator + 1).trim().replace(/^"|"$/g, '');
  }
  return '';
}

function safeHost(value: string): string {
  const host = value.trim();
  return !host || /[\r\n/\\]/.test(host) ? '' : host;
}

function loopbackHost(value: string): boolean {
  const host = value.replace(/^\[/, '').replace(/\](:\d+)?$/, '').replace(/:\d+$/, '');
  return host === '127.0.0.1' || host === 'localhost' || host === '::1';
}

export function externalBase(headers: IncomingHttpHeaders, config: AgentConfig): string {
  const configured = config.publicBaseUrl?.trim().replace(/\/$/, '');
  if (configured) return configured;
  const host = safeHost(firstHeader(headers, 'x-forwarded-host'))
    || safeHost(forwardedHeaderParam(headers, 'host'))
    || safeHost(firstHeader(headers, 'host'))
    || `127.0.0.1:${config.port}`;
  const forwardedProto = (firstHeader(headers, 'x-forwarded-proto') || forwardedHeaderParam(headers, 'proto')).toLowerCase();
  const protocol = forwardedProto === 'http' || forwardedProto === 'https'
    ? forwardedProto
    : loopbackHost(host) ? 'http' : 'https';
  return `${protocol}://${host}`;
}

function wellKnownUrl(baseUrl: string, suffix: string, includeMcp: boolean): string {
  const base = baseUrl.trim().replace(/\/$/, '');
  try {
    const url = new URL(base);
    const route = url.pathname.replace(/\/$/, '');
    url.pathname = `/.well-known/${suffix}${route}${includeMcp ? '/mcp' : ''}`;
    url.search = '';
    url.hash = '';
    return url.toString().replace(/\/$/, '');
  } catch {
    return `${base}/.well-known/${suffix}${includeMcp ? '/mcp' : ''}`;
  }
}

export function protectedResourceMetadataUrl(base: string): string {
  return wellKnownUrl(base, 'oauth-protected-resource', true);
}

export function authorizationServerMetadataUrl(base: string): string {
  return wellKnownUrl(base, 'oauth-authorization-server', false);
}

export function authorizationMetadata(base: string, oauth: OAuthRuntime) {
  const issuer = base.replace(/\/$/, '');
  return {
    issuer,
    authorization_endpoint: `${issuer}/oauth/authorize`,
    token_endpoint: `${issuer}/oauth/token`,
    response_types_supported: ['code'],
    grant_types_supported: ['authorization_code'],
    code_challenge_methods_supported: ['S256'],
    token_endpoint_auth_methods_supported: oauth.clientSecret ? ['client_secret_post', 'client_secret_basic'] : ['none']
  };
}

export function resourceMetadata(base: string) {
  const issuer = base.replace(/\/$/, '');
  return {
    resource: `${issuer}/mcp`,
    authorization_servers: [issuer],
    bearer_methods_supported: ['header'],
    scopes_supported: ['mcp']
  };
}

export function redirectUriAllowed(value: string): boolean {
  if (!value || value.trim() !== value) return false;
  try {
    const url = new URL(value);
    if (url.username || url.password || url.hash) return false;
    // Match Desktop's RFC 8252 loopback callbacks, including manual HTTP PKCE.
    if (url.protocol === 'http:') {
      return ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
    }
    return url.protocol === 'https:'
      && (url.port === '' || url.port === '443')
      && allowedOrigins.has(url.origin);
  } catch { return false; }
}

function validVerifier(value: string): boolean {
  return /^[A-Za-z0-9._~-]{43,128}$/.test(value);
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]!);
}

function loginPage(values: {
  clientId: string;
  redirectUri: string;
  challenge: string;
  challengeMethod: string;
  state: string;
  error?: string;
  workspacePath?: string;
}): string {
  const error = values.error ? `<p style="color:red">${escapeHtml(values.error)}</p>` : '';
  const workspace = values.workspacePath ? `<p>Workspace: <code>${escapeHtml(values.workspacePath)}</code></p>` : '';
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Authorize MCP Server</title><style>body{font-family:sans-serif;max-width:380px;margin:4rem auto;padding:1rem}input{width:100%;padding:.5rem;margin:.4rem 0;box-sizing:border-box}button{width:100%;padding:.7rem;background:#0066cc;color:#fff;border:none;cursor:pointer}</style></head><body><h2>Authorize Coding Tools MCP</h2>${workspace}<p>Client: <strong>${escapeHtml(values.clientId)}</strong></p><p>Redirect URI: <code>${escapeHtml(values.redirectUri)}</code></p>${error}<form method="POST" action=""><input type="hidden" name="client_id" value="${escapeHtml(values.clientId)}"><input type="hidden" name="redirect_uri" value="${escapeHtml(values.redirectUri)}"><input type="hidden" name="code_challenge" value="${escapeHtml(values.challenge)}"><input type="hidden" name="code_challenge_method" value="${escapeHtml(values.challengeMethod)}"><input type="hidden" name="state" value="${escapeHtml(values.state)}"><label>Password<input type="password" name="password" autocomplete="current-password" required></label><button type="submit">Authorize</button></form></body></html>`;
}

function htmlError(message: string, status = 400): HtmlResponse {
  return { status, body: `<h2>Error</h2><p>${escapeHtml(message)}</p>` };
}

function signJwt(payload: Record<string, unknown>, secret: string): string {
  const header = base64url(JSON.stringify({ alg: 'HS256', typ: 'JWT' }));
  const body = base64url(JSON.stringify(payload));
  const signature = createHmac('sha256', secret).update(`${header}.${body}`).digest('base64url');
  return `${header}.${body}.${signature}`;
}

function basicCredentials(headers: IncomingHttpHeaders): { id: string; secret: string } | undefined {
  const authorization = String(headers.authorization ?? '');
  if (!authorization.startsWith('Basic ')) return undefined;
  try {
    const decoded = Buffer.from(authorization.slice(6), 'base64').toString('utf8');
    const separator = decoded.indexOf(':');
    if (separator < 0) return undefined;
    return { id: decoded.slice(0, separator), secret: decoded.slice(separator + 1) };
  } catch { return undefined; }
}

function tokenError(error: string, description: string): TokenResponse {
  return { status: 400, body: { error, error_description: description } };
}

export class OAuthRuntime {
  clientId: string;
  clientSecret?: string;
  password: string;
  tokenSecret: string;
  tokenTtlSeconds: number;
  readonly #pending = new Map<string, PendingCode>();
  readonly #now: () => number;
  readonly #persistPassword?: (password: string) => Promise<void>;
  #authorizationQueue: Promise<void> = Promise.resolve();
  #loginFailures = 0;
  #loginWindowStartedAt = 0;
  #loginBlockedUntil = 0;

  constructor(
    config: AgentConfig['oauth'],
    now: () => number = Date.now,
    persistPassword?: (password: string) => Promise<void>
  ) {
    this.clientId = config.clientId.trim();
    if (!this.clientId) throw new Error('OAuth client ID is not configured');
    this.clientSecret = config.clientSecret && config.clientSecret.trim() ? config.clientSecret : undefined;
    this.password = requireConfiguredSecret(config.password, 'OAuth password');
    this.tokenSecret = requireConfiguredSecret(config.tokenSecret, 'OAuth token secret');
    const requestedTokenTtl = Number(config.tokenTtlSeconds ?? DEFAULT_OAUTH_TOKEN_TTL_SECONDS);
    this.tokenTtlSeconds = Number.isInteger(requestedTokenTtl) && requestedTokenTtl > 0
      ? Math.min(requestedTokenTtl, MAX_OAUTH_TOKEN_TTL_SECONDS)
      : DEFAULT_OAUTH_TOKEN_TTL_SECONDS;
    this.#now = now;
    this.#persistPassword = persistPassword;
  }

  update(config: AgentConfig['oauth']): void {
    const replacement = new OAuthRuntime(config, this.#now, this.#persistPassword);
    this.clientId = replacement.clientId;
    this.clientSecret = replacement.clientSecret;
    this.password = replacement.password;
    this.tokenSecret = replacement.tokenSecret;
    this.tokenTtlSeconds = replacement.tokenTtlSeconds;
    this.#pending.clear();
    this.#resetLoginFailures();
  }

  clientIdAllowed(clientId: string): boolean {
    return Boolean(clientId) && constantTimeEqual(clientId, this.clientId);
  }

  authorizePage(url: URL, workspacePath?: string): HtmlResponse {
    const responseType = url.searchParams.get('response_type') ?? '';
    const clientId = url.searchParams.get('client_id') ?? '';
    const redirectUri = url.searchParams.get('redirect_uri') ?? '';
    const challenge = url.searchParams.get('code_challenge') ?? '';
    const challengeMethod = url.searchParams.get('code_challenge_method') ?? '';
    const state = url.searchParams.get('state') ?? '';
    if (responseType !== 'code') return htmlError("response_type must be 'code'");
    if (!this.clientIdAllowed(clientId)) return htmlError('Unknown client_id');
    if (!redirectUriAllowed(redirectUri)) return htmlError('redirect_uri is not allowed');
    if (challengeMethod !== 'S256' || !challenge) {
      return htmlError('code_challenge_method must be S256 and code_challenge is required');
    }
    return { status: 200, body: loginPage({ clientId, redirectUri, challenge, challengeMethod, state, workspacePath }) };
  }

  authorizeSubmit(form: URLSearchParams, base: string): HtmlResponse {
    const clientId = form.get('client_id') ?? '';
    const redirectUri = form.get('redirect_uri') ?? '';
    const challenge = form.get('code_challenge') ?? '';
    const challengeMethod = form.get('code_challenge_method') ?? '';
    const state = form.get('state') ?? '';
    const values = { clientId, redirectUri, challenge, challengeMethod, state };
    if (!redirectUriAllowed(redirectUri)) return htmlError('redirect_uri is not allowed');
    if (!this.clientIdAllowed(clientId)) return { status: 200, body: loginPage({ ...values, error: 'Invalid client' }) };
    if (challengeMethod !== 'S256' || !challenge) return { status: 200, body: loginPage({ ...values, error: 'Invalid PKCE parameters' }) };
    if (this.#loginRateLimited()) {
      return { status: 429, body: loginPage({ ...values, error: 'Too many authorization attempts; try again later' }) };
    }
    if (!constantTimeEqual(form.get('password') ?? '', this.password)) {
      const blocked = this.#recordLoginFailure();
      return {
        status: blocked ? 429 : 401,
        body: loginPage({
          ...values,
          error: blocked ? 'Too many authorization attempts; try again later' : 'Invalid password'
        })
      };
    }
    this.#resetLoginFailures();

    this.#cleanupPending();
    const code = randomBytes(16).toString('hex');
    this.#pending.set(code, {
      challenge,
      clientId,
      redirectUri,
      state,
      expiresAt: this.#now() + codeTtlMs,
      issuer: base.replace(/\/$/, '')
    });
    const target = new URL(redirectUri);
    target.searchParams.append('code', code);
    if (state) target.searchParams.append('state', state);
    return { status: 303, location: target.toString(), body: '' };
  }

  async authorizeSubmitOneTime(form: URLSearchParams, base: string): Promise<HtmlResponse> {
    let release!: () => void;
    const previous = this.#authorizationQueue;
    this.#authorizationQueue = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try {
      const clientId = form.get('client_id') ?? '';
      const redirectUri = form.get('redirect_uri') ?? '';
      const challenge = form.get('code_challenge') ?? '';
      const challengeMethod = form.get('code_challenge_method') ?? '';
      const state = form.get('state') ?? '';
      const submittedPassword = form.get('password') ?? '';
      const values = { clientId, redirectUri, challenge, challengeMethod, state };
      if (!redirectUriAllowed(redirectUri)) return htmlError('redirect_uri is not allowed');
      if (!this.clientIdAllowed(clientId)) return { status: 200, body: loginPage({ ...values, error: 'Invalid client' }) };
      if (challengeMethod !== 'S256' || !challenge) return { status: 200, body: loginPage({ ...values, error: 'Invalid PKCE parameters' }) };
      if (this.#loginRateLimited()) {
        return { status: 429, body: loginPage({ ...values, error: 'Too many authorization attempts; try again later' }) };
      }
      if (!constantTimeEqual(submittedPassword, this.password)) {
        const blocked = this.#recordLoginFailure();
        return {
          status: blocked ? 429 : 401,
          body: loginPage({
            ...values,
            error: blocked ? 'Too many authorization attempts; try again later' : 'Invalid password'
          })
        };
      }

      const nextPassword = generateAuthorizationPassword();
      try {
        await this.#persistPassword?.(nextPassword);
      } catch {
        return {
          status: 503,
          body: loginPage({ ...values, error: 'Authorization password rotation failed; try again later' })
        };
      }
      this.password = nextPassword;
      this.#resetLoginFailures();

      this.#cleanupPending();
      const code = randomBytes(16).toString('hex');
      this.#pending.set(code, {
        challenge,
        clientId,
        redirectUri,
        state,
        expiresAt: this.#now() + codeTtlMs,
        issuer: base.replace(/\/$/, '')
      });
      const target = new URL(redirectUri);
      target.searchParams.append('code', code);
      if (state) target.searchParams.append('state', state);
      return { status: 303, location: target.toString(), body: '' };
    } finally {
      release();
    }
  }

  exchangeToken(form: URLSearchParams, headers: IncomingHttpHeaders, base: string): TokenResponse {
    if (form.get('grant_type') !== 'authorization_code') {
      return tokenError('unsupported_grant_type', 'Only authorization_code is supported');
    }

    let clientId = form.get('client_id') ?? '';
    let clientSecret = form.get('client_secret') ?? '';
    const basic = basicCredentials(headers);
    if (basic) {
      clientId ||= basic.id;
      clientSecret ||= basic.secret;
    }
    if (!this.clientIdAllowed(clientId)) return tokenError('invalid_client', 'Unknown client_id');
    if (this.clientSecret && !constantTimeEqual(clientSecret, this.clientSecret)) {
      return tokenError('invalid_client', 'Invalid client_secret');
    }

    const code = form.get('code') ?? '';
    const redirectUri = form.get('redirect_uri') ?? '';
    const verifier = form.get('code_verifier') ?? '';
    if (!code) return tokenError('invalid_grant', 'code is required');
    if (!validVerifier(verifier)) return tokenError('invalid_grant', 'Invalid code_verifier');
    if (!redirectUriAllowed(redirectUri)) return tokenError('invalid_grant', 'redirect_uri is not allowed');

    const data = this.#pending.get(code);
    this.#pending.delete(code);
    if (!data) return tokenError('invalid_grant', 'Unknown or already-used authorization code');
    if (this.#now() > data.expiresAt) return tokenError('invalid_grant', 'Authorization code expired');
    if (!constantTimeEqual(data.clientId, clientId)) return tokenError('invalid_grant', 'client_id mismatch');
    if (!constantTimeEqual(data.redirectUri, redirectUri)) return tokenError('invalid_grant', 'redirect_uri mismatch');
    const challenge = createHash('sha256').update(verifier).digest('base64url');
    if (!constantTimeEqual(challenge, data.challenge)) return tokenError('invalid_grant', 'PKCE verification failed');

    const issuer = (data.issuer || base).replace(/\/$/, '');
    const issuedAt = Math.floor(this.#now() / 1000);
    return {
      status: 200,
      body: {
        access_token: signJwt({ iss: issuer, aud: `${issuer}/mcp`, iat: issuedAt, exp: issuedAt + this.tokenTtlSeconds, scope: 'mcp' }, this.tokenSecret),
        token_type: 'Bearer',
        expires_in: this.tokenTtlSeconds
      }
    };
  }

  verifyBearer(headers: IncomingHttpHeaders, base: string): boolean {
    const authorization = String(headers.authorization ?? '');
    if (!authorization.startsWith('Bearer ')) return false;
    const parts = authorization.slice(7).trim().split('.');
    if (parts.length !== 3) return false;
    const expected = createHmac('sha256', this.tokenSecret).update(`${parts[0]}.${parts[1]}`).digest('base64url');
    if (!constantTimeEqual(parts[2], expected)) return false;
    try {
      const header = JSON.parse(Buffer.from(parts[0], 'base64url').toString('utf8')) as { alg?: unknown };
      const payload = JSON.parse(Buffer.from(parts[1], 'base64url').toString('utf8')) as {
        iss?: unknown; aud?: unknown; iat?: unknown; exp?: unknown; scope?: unknown;
      };
      const issuer = base.replace(/\/$/, '');
      return header.alg === 'HS256'
        && payload.iss === issuer
        && typeof payload.aud === 'string'
        && (payload.aud === `${issuer}/mcp` || payload.aud === issuer)
        && typeof payload.iat === 'number'
        && typeof payload.exp === 'number'
        && payload.exp >= this.#now() / 1000
        && typeof payload.scope === 'string';
    } catch { return false; }
  }

  dispose(): void {
    this.#pending.clear();
  }

  #loginRateLimited(): boolean {
    const now = this.#now();
    if (this.#loginBlockedUntil > now) return true;
    if (this.#loginBlockedUntil) this.#resetLoginFailures();
    return false;
  }

  #recordLoginFailure(): boolean {
    const now = this.#now();
    if (!this.#loginWindowStartedAt || now - this.#loginWindowStartedAt >= loginFailureWindowMs) {
      this.#loginWindowStartedAt = now;
      this.#loginFailures = 0;
    }
    this.#loginFailures += 1;
    if (this.#loginFailures < maxLoginFailures) return false;
    this.#loginBlockedUntil = now + loginBlockMs;
    return true;
  }

  #resetLoginFailures(): void {
    this.#loginFailures = 0;
    this.#loginWindowStartedAt = 0;
    this.#loginBlockedUntil = 0;
  }

  #cleanupPending(): void {
    const now = this.#now();
    for (const [code, value] of this.#pending) if (value.expiresAt < now) this.#pending.delete(code);
  }
}

export function sendJson(res: ServerResponse, status: number, value: unknown): void {
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' }).end(JSON.stringify(value));
}
