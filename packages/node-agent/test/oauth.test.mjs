import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash, createHmac } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  authorizationMetadata, externalBase, OAuthRuntime, redirectUriAllowed, resourceMetadata
} from '../dist/oauth.js';
import { createAgentRuntime } from '../dist/server.js';

const verifier = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~';
const redirectUri = 'https://chatgpt.com/connector_platform_oauth_redirect';

function oauthConfig(overrides = {}) {
  return {
    clientId: 'chatgpt',
    password: 'oauth-test-password',
    tokenSecret: 'oauth-test-token-secret-that-is-long-enough',
    ...overrides
  };
}

function agentConfig(root, dataDir, publicBaseUrl = 'https://public.example/builtin/clients/oauth-test') {
  return {
    host: '127.0.0.1',
    port: 0,
    publicBaseUrl,
    dataDir,
    permissionMode: 'trusted',
    management: { enabled: false },
    oauth: oauthConfig(),
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 }
  };
}

function authorizationForm(state = 'state-1') {
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  return new URLSearchParams({
    client_id: 'chatgpt',
    redirect_uri: redirectUri,
    code_challenge: challenge,
    code_challenge_method: 'S256',
    state,
    password: 'oauth-test-password'
  });
}

function tokenForm(code) {
  return new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    redirect_uri: redirectUri,
    code_verifier: verifier,
    client_id: 'chatgpt'
  });
}

function signToken(payload, secret) {
  const header = Buffer.from(JSON.stringify({ alg: 'HS256', typ: 'JWT' })).toString('base64url');
  const body = Buffer.from(JSON.stringify(payload)).toString('base64url');
  const signature = createHmac('sha256', secret).update(`${header}.${body}`).digest('base64url');
  return `${header}.${body}.${signature}`;
}

test('OAuth metadata, redirect allowlist and Forwarded base resolution match Rust', () => {
  const runtime = new OAuthRuntime(oauthConfig());
  const config = agentConfig('C:\\workspace', 'C:\\state', undefined);
  delete config.publicBaseUrl;
  config.port = 3789;
  assert.equal(externalBase({ forwarded: 'for=192.0.2.1;proto=https;host="mcp.example"' }, config), 'https://mcp.example');
  assert.equal(externalBase({ 'x-forwarded-proto': 'https', 'x-forwarded-host': 'proxy.example' }, config), 'https://proxy.example');

  assert.equal(redirectUriAllowed('https://chatgpt.com/connector/oauth/test'), true);
  assert.equal(redirectUriAllowed('https://chat.openai.com/aip/test/oauth/callback?source=connector'), true);
  assert.equal(redirectUriAllowed('https://chatgpt.com:443/connector/oauth/test'), true);
  for (const value of [
    'http://chatgpt.com/connector/oauth/test',
    'https://attacker.example/callback',
    'https://chatgpt.com.attacker.example/callback',
    'https://chatgpt.com@attacker.example/callback',
    'https://chatgpt.com:444/connector/oauth/test',
    'https://chatgpt.com/connector/oauth/test#fragment',
    ' https://chatgpt.com/connector/oauth/test'
  ]) assert.equal(redirectUriAllowed(value), false, value);

  assert.deepEqual(authorizationMetadata('https://mcp.example/base', runtime).grant_types_supported, ['authorization_code', 'refresh_token']);
  assert.deepEqual(authorizationMetadata('https://mcp.example/base', runtime).token_endpoint_auth_methods_supported, ['none']);
  assert.deepEqual(resourceMetadata('https://mcp.example/base').authorization_servers, ['https://mcp.example/base']);

  const page = runtime.authorizePage(new URL(`https://local/oauth/authorize?${new URLSearchParams({
    response_type: 'code', client_id: 'chatgpt', redirect_uri: redirectUri,
    code_challenge: 'challenge', code_challenge_method: 'S256', state: 'state'
  })}`));
  assert.equal(page.status, 200);
  assert.match(page.body, /method="POST" action=""/);
});

test('OAuthRuntime rejects missing credentials and ignores blank optional client secrets', () => {
  assert.throws(() => new OAuthRuntime(oauthConfig({ clientId: ' ' })), /OAuth client ID is not configured/);
  assert.throws(() => new OAuthRuntime(oauthConfig({ password: ' ' })), /OAuth password is not configured/);
  assert.throws(() => new OAuthRuntime(oauthConfig({ tokenSecret: ' ' })), /OAuth token secret is not configured/);
  assert.equal(new OAuthRuntime(oauthConfig({ clientSecret: ' ' })).clientSecret, undefined);
});

test('OAuthRuntime rate limits repeated password failures and recovers after the block window', () => {
  let now = 1;
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig(), () => now);
  const invalid = authorizationForm('rate-limit');
  invalid.set('password', 'wrong-password');
  for (let attempt = 0; attempt < 4; attempt += 1) {
    assert.equal(runtime.authorizeSubmit(invalid, base).status, 401);
  }
  assert.equal(runtime.authorizeSubmit(invalid, base).status, 429);
  assert.equal(runtime.authorizeSubmit(authorizationForm('blocked-correct'), base).status, 429);
  now += 60_001;
  assert.equal(runtime.authorizeSubmit(authorizationForm('unblocked'), base).status, 303);
});

test('OAuthRuntime keeps the authorization password reusable by default (keepalive)', async () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const persisted = [];
  const runtime = new OAuthRuntime(oauthConfig(), Date.now, async password => { persisted.push(password); });
  const password = runtime.password;
  const first = await runtime.authorizeSubmitOneTime(authorizationForm('reuse-1'), base);
  const second = await runtime.authorizeSubmitOneTime(authorizationForm('reuse-2'), base);
  assert.equal(first.status, 303);
  assert.equal(second.status, 303);
  assert.equal(runtime.password, password);
  assert.deepEqual(persisted, []);
  const exchanged = runtime.exchangeToken(tokenForm(new URL(first.location).searchParams.get('code')), {}, base);
  assert.equal(exchanged.body.expires_in, 3650 * 24 * 60 * 60);
});

test('OAuthRuntime consumes authorization passwords once, persists rotation, and rejects concurrent reuse', async () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const persisted = [];
  const runtime = new OAuthRuntime(oauthConfig({ rotatePassword: true }), Date.now, async password => {
    await new Promise(resolve => setTimeout(resolve, 5));
    persisted.push(password);
  });

  const oldPassword = runtime.password;
  const firstForm = authorizationForm('one-time-first');
  const reusedForm = authorizationForm('one-time-reused');
  const [first, reused] = await Promise.all([
    runtime.authorizeSubmitOneTime(firstForm, base),
    runtime.authorizeSubmitOneTime(reusedForm, base)
  ]);

  assert.equal(first.status, 303);
  assert.equal(reused.status, 401);
  assert.equal(persisted.length, 1);
  assert.notEqual(runtime.password, oldPassword);
  assert.equal(persisted[0], runtime.password);

  const firstCode = new URL(first.location).searchParams.get('code');
  assert.ok(firstCode);
  const exchanged = runtime.exchangeToken(tokenForm(firstCode), {}, base);
  assert.equal(exchanged.status, 200);
  assert.equal(exchanged.body.expires_in, 3650 * 24 * 60 * 60);

  const nextPassword = runtime.password;
  const nextForm = authorizationForm('one-time-next');
  nextForm.set('password', nextPassword);
  const next = await runtime.authorizeSubmitOneTime(nextForm, base);
  assert.equal(next.status, 303);
  assert.equal(persisted.length, 2);
  assert.notEqual(runtime.password, nextPassword);
});

test('OAuthRuntime refresh_token renews access without the one-time password', async () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig());
  const authorized = await runtime.authorizeSubmitOneTime(authorizationForm('refresh-flow'), base);
  assert.equal(authorized.status, 303);
  const code = new URL(authorized.location).searchParams.get('code');
  const exchanged = runtime.exchangeToken(tokenForm(code), {}, base);
  assert.equal(exchanged.status, 200);
  const { access_token: access, refresh_token: refresh } = exchanged.body;
  assert.equal(typeof refresh, 'string');
  assert.equal(runtime.verifyBearer({ authorization: `Bearer ${access}` }, base), true);
  assert.equal(runtime.verifyBearer({ authorization: `Bearer ${refresh}` }, base), false, 'refresh token is not a bearer token');

  const refreshForm = (token, clientId = 'chatgpt') => new URLSearchParams({ grant_type: 'refresh_token', refresh_token: token, client_id: clientId });
  const renewed = runtime.exchangeToken(refreshForm(refresh), {}, base);
  assert.equal(renewed.status, 200);
  assert.equal(runtime.verifyBearer({ authorization: `Bearer ${renewed.body.access_token}` }, base), true);
  assert.equal(typeof renewed.body.refresh_token, 'string');

  assert.equal(runtime.exchangeToken(refreshForm(access), {}, base).status, 400, 'access token cannot refresh');
  assert.equal(runtime.exchangeToken(refreshForm('garbage'), {}, base).status, 400);
  assert.equal(runtime.exchangeToken(refreshForm(''), {}, base).body.error, 'invalid_request');
  const other = new OAuthRuntime(oauthConfig({ tokenSecret: 'another-token-secret-that-is-long-enough' }));
  assert.equal(other.exchangeToken(refreshForm(refresh), {}, base).status, 400, 'rotated token secret revokes refresh tokens');
});

test('OAuthRuntime applies configurable access-token TTL with a 30-day cap', async () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig({ tokenTtlSeconds: 60 * 60 }));
  const authorized = await runtime.authorizeSubmitOneTime(authorizationForm('custom-ttl'), base);
  assert.equal(authorized.status, 303);
  const code = new URL(authorized.location).searchParams.get('code');
  assert.ok(code);
  const exchanged = runtime.exchangeToken(tokenForm(code), {}, base);
  assert.equal(exchanged.status, 200);
  assert.equal(exchanged.body.expires_in, 60 * 60);
  const [, payloadPart] = exchanged.body.access_token.split('.');
  const payload = JSON.parse(Buffer.from(payloadPart, 'base64url').toString('utf8'));
  assert.equal(payload.exp - payload.iat, 60 * 60);

  const capped = new OAuthRuntime(oauthConfig({ tokenTtlSeconds: 100 * 365 * 24 * 60 * 60 }));
  assert.equal(capped.tokenTtlSeconds, 3650 * 24 * 60 * 60);
});

test('OAuthRuntime does not consume a valid password when persistence fails', async () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig({ rotatePassword: true }), Date.now, async () => {
    throw new Error('simulated persistence failure');
  });
  const originalPassword = runtime.password;
  const failed = await runtime.authorizeSubmitOneTime(authorizationForm('persist-failure'), base);
  assert.equal(failed.status, 503);
  assert.equal(runtime.password, originalPassword);
});

test('OAuthRuntime updates credentials and clears pending authorization codes', () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig({ clientSecret: 'old-client-secret' }));
  const authorized = runtime.authorizeSubmit(authorizationForm('before-rotation'), base);
  const code = new URL(authorized.location).searchParams.get('code');
  assert.ok(code);

  runtime.update(oauthConfig({
    password: 'rotated-password',
    clientSecret: 'rotated-client-secret'
  }));
  assert.equal(runtime.password, 'rotated-password');
  assert.equal(runtime.clientSecret, 'rotated-client-secret');

  const form = tokenForm(code);
  form.set('client_secret', 'rotated-client-secret');
  assert.deepEqual(runtime.exchangeToken(form, {}, base).body, {
    error: 'invalid_grant',
    error_description: 'Unknown or already-used authorization code'
  });
});

test('issuing a new code removes expired pending codes like Rust', () => {
  let now = 0;
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig(), () => now);
  const oldCode = new URL(runtime.authorizeSubmit(authorizationForm('old'), base).location).searchParams.get('code');
  assert.ok(oldCode);
  now = 5 * 60_000 + 1;
  runtime.authorizeSubmit(authorizationForm('new'), base);
  assert.deepEqual(runtime.exchangeToken(tokenForm(oldCode), {}, base).body, {
    error: 'invalid_grant',
    error_description: 'Unknown or already-used authorization code'
  });
});

test('authorization codes are isolated per OAuthRuntime and single-use', () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const first = new OAuthRuntime(oauthConfig());
  const second = new OAuthRuntime(oauthConfig());
  const authorized = first.authorizeSubmit(authorizationForm('state-isolated'), base);
  assert.equal(authorized.status, 303);
  const callback = new URL(authorized.location);
  assert.equal(callback.searchParams.get('state'), 'state-isolated');
  const code = callback.searchParams.get('code');
  assert.ok(code);

  const rejected = second.exchangeToken(tokenForm(code), {}, base);
  assert.deepEqual(rejected.body, {
    error: 'invalid_grant',
    error_description: 'Unknown or already-used authorization code'
  });

  const exchanged = first.exchangeToken(tokenForm(code), {}, base);
  assert.equal(exchanged.status, 200);
  assert.equal(exchanged.body.expires_in, 3650 * 24 * 60 * 60);
  const accessToken = exchanged.body.access_token;
  assert.equal(first.verifyBearer({ authorization: `Bearer ${accessToken}` }, base), true);
  assert.deepEqual(first.exchangeToken(tokenForm(code), {}, base).body, {
    error: 'invalid_grant',
    error_description: 'Unknown or already-used authorization code'
  });
});

test('optional client secret supports post and basic authentication without consuming codes on client errors', () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig({ clientSecret: 'oauth-client-secret' }));
  assert.deepEqual(authorizationMetadata(base, runtime).token_endpoint_auth_methods_supported, [
    'client_secret_post', 'client_secret_basic'
  ]);

  const firstCode = new URL(runtime.authorizeSubmit(authorizationForm('secret-basic'), base).location).searchParams.get('code');
  assert.ok(firstCode);
  assert.deepEqual(runtime.exchangeToken(tokenForm(firstCode), {}, base).body, {
    error: 'invalid_client',
    error_description: 'Invalid client_secret'
  });
  const basicForm = tokenForm(firstCode);
  basicForm.delete('client_id');
  const basic = Buffer.from('chatgpt:oauth-client-secret').toString('base64');
  assert.equal(runtime.exchangeToken(basicForm, { authorization: `Basic ${basic}` }, base).status, 200);

  const secondCode = new URL(runtime.authorizeSubmit(authorizationForm('secret-post'), base).location).searchParams.get('code');
  assert.ok(secondCode);
  const postForm = tokenForm(secondCode);
  postForm.set('client_secret', 'oauth-client-secret');
  assert.equal(runtime.exchangeToken(postForm, {}, base).status, 200);
});

test('bearer verification follows Rust string audience and required claim types', () => {
  const base = 'https://public.example/builtin/clients/oauth-test';
  const runtime = new OAuthRuntime(oauthConfig());
  const now = Math.floor(Date.now() / 1000);
  const common = { iss: base, iat: now, exp: now + 300, scope: 'any-string' };
  const issuerAudience = signToken({ ...common, aud: base }, runtime.tokenSecret);
  assert.equal(runtime.verifyBearer({ authorization: `Bearer ${issuerAudience}` }, base), true);
  const arrayAudience = signToken({ ...common, aud: [`${base}/mcp`] }, runtime.tokenSecret);
  assert.equal(runtime.verifyBearer({ authorization: `Bearer ${arrayAudience}` }, base), false);
  const missingScope = signToken({ iss: base, aud: `${base}/mcp`, iat: now, exp: now + 300 }, runtime.tokenSecret);
  assert.equal(runtime.verifyBearer({ authorization: `Bearer ${missingScope}` }, base), false);
});

test('closing an Agent runtime clears its pending authorization codes', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-oauth-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-oauth-state-'));
  t.after(async () => {
    await rm(root, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  });
  const config = agentConfig(root, dataDir);
  const runtime = await createAgentRuntime(config);
  await new Promise(resolve => runtime.server.listen(0, '127.0.0.1', resolve));
  const address = runtime.server.address();
  assert.ok(address && typeof address === 'object');
  const response = await fetch(`http://127.0.0.1:${address.port}/builtin/clients/oauth-test/oauth/authorize`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: authorizationForm('state-close'),
    redirect: 'manual'
  });
  assert.equal(response.status, 303);
  const code = new URL(response.headers.get('location')).searchParams.get('code');
  assert.ok(code);
  await runtime.close();

  assert.deepEqual(runtime.oauth.exchangeToken(tokenForm(code), {}, config.publicBaseUrl).body, {
    error: 'invalid_grant',
    error_description: 'Unknown or already-used authorization code'
  });
});


test('manual loopback PKCE validates callback and reuses access tokens after restart', async () => {
  const callback = 'http://127.0.0.1:8765/callback';
  const base = 'https://mcp.example';
  for (const value of [callback, 'http://localhost:8765/callback', 'http://[::1]:8765/callback']) assert.equal(redirectUriAllowed(value), true, value);
  for (const value of ['http://127.0.0.1.attacker.example/callback', 'http://attacker.example/callback', 'http://user@127.0.0.1/callback', callback + '#secret']) assert.equal(redirectUriAllowed(value), false, value);
  const runtime = new OAuthRuntime(oauthConfig());
  const form = authorizationForm('manual-state');
  form.set('redirect_uri', callback);
  const authorized = await runtime.authorizeSubmitOneTime(form, base);
  assert.equal(authorized.status, 303);
  const location = new URL(authorized.location);
  assert.equal(location.origin + location.pathname, callback);
  assert.equal(location.searchParams.get('state'), 'manual-state');
  const exchange = tokenForm(location.searchParams.get('code'));
  exchange.set('redirect_uri', callback);
  const token = runtime.exchangeToken(exchange, {}, base);
  assert.equal(token.status, 200);
  const headers = { authorization: `Bearer ${token.body.access_token}` };
  runtime.dispose();
  const restarted = new OAuthRuntime(oauthConfig({ password: runtime.password }));
  assert.equal(restarted.verifyBearer(headers, base), true);
  assert.equal(restarted.verifyBearer(headers, 'https://changed.example'), false);
  assert.equal(new OAuthRuntime(oauthConfig({ tokenSecret: 'different-signing-secret' })).verifyBearer(headers, base), false);
  const renewedForm = authorizationForm('wrong-callback');
  renewedForm.set('password', runtime.password);
  renewedForm.set('redirect_uri', callback);
  const next = await restarted.authorizeSubmitOneTime(renewedForm, base);
  const mismatch = tokenForm(new URL(next.location).searchParams.get('code'));
  mismatch.set('redirect_uri', 'http://127.0.0.1:8766/callback');
  assert.equal(restarted.exchangeToken(mismatch, {}, base).body.error_description, 'redirect_uri mismatch');
});

test('HTTP reconnect resumes the same chat after service restart without reauthorization', async t => {
  const { chatUi } = await import('../dist/chat/store.js');
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-resume-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-resume-state-'));
  const config = agentConfig(root, dataDir, 'https://mcp.example');
  let runtime;
  t.after(async () => {
    await runtime?.close();
    await rm(root, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  });
  async function start() {
    runtime = await createAgentRuntime(config, { persistOAuthPassword: async password => { config.oauth.password = password; } });
    await new Promise(resolve => runtime.server.listen(0, '127.0.0.1', resolve));
    return `http://127.0.0.1:${runtime.server.address().port}`;
  }
  let local = await start();
  const form = authorizationForm('restart-chat');
  form.set('redirect_uri', 'http://127.0.0.1:8765/callback');
  const authorized = await fetch(local + '/oauth/authorize', {
    method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded', 'User-Agent': 'MCP-Reconnect-Test/1.0' }, body: form, redirect: 'manual'
  });
  assert.equal(authorized.status, 303);
  const location = new URL(authorized.headers.get('location'));
  assert.equal(location.searchParams.get('state'), 'restart-chat');
  const exchange = tokenForm(location.searchParams.get('code'));
  exchange.set('redirect_uri', form.get('redirect_uri'));
  const tokenResponse = await fetch(local + '/oauth/token', { method: 'POST', body: exchange, headers: { 'User-Agent': 'MCP-Reconnect-Test/1.0' } });
  assert.equal(tokenResponse.status, 200);
  const { access_token } = await tokenResponse.json();
  let rpcId = 0;
  async function rpc(method, params) {
    const response = await fetch(local + '/mcp', {
      method: 'POST', headers: { 'content-type': 'application/json', 'User-Agent': 'MCP-Reconnect-Test/1.0', authorization: `Bearer ${access_token}` },
      body: JSON.stringify({ jsonrpc: '2.0', id: ++rpcId, method, params })
    });
    assert.equal(response.status, 200);
    const result = await response.json();
    assert.equal(result.error, undefined);
    assert.notEqual(result.result?.isError, true, JSON.stringify(result.result));
    return result.result;
  }
  async function initialize() {
    await rpc('initialize', { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'resume-test', version: '1' } });
    assert.ok((await rpc('tools/list', {})).tools.some(tool => tool.name === 'chat_open'));
  }
  const call = async (name, args = {}) => (await rpc('tools/call', { name, arguments: { workspace_folder_id: 'repo', ...args } })).structuredContent;
  await initialize();
  await rpc('tools/call', { name: 'list_workspace_folders', arguments: {} });
  const session = chatUi(root, { action: 'create' }).session;
  const opened = await call('chat_open', { chat_id: session.id });
  const args = { chat_id: session.id, attachment_id: opened.attachment_id };
  chatUi(root, { action: 'send', chat_id: session.id, message_id: 'before-restart', text: 'Resume me' });
  assert.equal((await call('chat_wait', { ...args, timeout_ms: 0 })).message.id, 'before-restart');
  await runtime.close();
  runtime = undefined;
  local = await start();
  await initialize();
  await rpc('tools/call', { name: 'list_workspace_folders', arguments: {} });
  assert.equal((await call('chat_open', args)).attachment_id, args.attachment_id);
  assert.equal((await call('chat_wait', { ...args, timeout_ms: 0 })).message.id, 'before-restart');
  assert.equal((await call('chat_reply', { ...args, message_id: 'after-restart', reply_to: 'before-restart', text: 'Resumed', final: true })).persisted, true);
  assert.equal((await call('chat_wait', { ...args, timeout_ms: 0 })).status, 'idle');
  assert.equal(chatUi(root, { action: 'read', chat_id: session.id }).session.messages.length, 2);
});
