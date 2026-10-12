import test from 'node:test';
import assert from 'node:assert/strict';
import { parseSession, runSeeding, CodeRabbitClient, publicMcpEndpoint, SessionExpiredError, jwtExpiry } from '../plugins/coderabbit/src/engine.ts';
import { createStarterRepository, pollDeviceToken, randomSource } from '../plugins/coderabbit/src/github.ts';

const user = JSON.stringify({ state: { user: { id: 'u1', providerLogin: 'alice', providerUserId: '42' }, currentOrganization: { id: 'org1', organization_name: 'alice', provider_organization_id: '42', workspace_id: 'ws1', type: 'github' } } });
const captured = { accessToken: 'tok-1', user, clerkGitProvider: 'github' };
const trpc = data => JSON.stringify([{ result: { data: { json: data } } }]);
const uuid = n => `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`;

function fakeHttp({ repos = [{ id: 'r-new', name: 'alice/new', defaultBranch: '' }], expireOnce = false } = {}) {
  const calls = []; let tasks = 0, expired = !expireOnce;
  const http = async request => {
    calls.push(request);
    const url = new URL(request.url);
    if (url.pathname.endsWith('repositories.getAllRepos')) return { status: 200, body: trpc({ repos }) };
    if (url.pathname.endsWith('codingAgent.getRepositoryDefaultBranch')) return { status: 200, body: trpc({ defaultBranch: 'develop' }) };
    if (url.pathname.endsWith('codingAgent.enqueueCodingTask')) {
      if (!expired) { expired = true; return { status: 401, body: '' }; }
      return { status: 200, body: trpc({ taskId: uuid(++tasks) }) };
    }
    throw new Error('unexpected ' + request.url);
  };
  return { http, calls };
}
function library() {
  const created = []; let n = 0;
  const call = async args => {
    if (args.action === 'seed_batch') return { batch: Array.from({ length: args.count }, () => ({ seed_id: 'seed-' + (++n), ticket: 'f'.repeat(64), url_key: 'a'.repeat(64), prefix: args.prefix, repo_id: args.repo_id, branch: args.branch })) };
    if (args.action === 'seed_created') { created.push(args); return { ok: true }; }
    throw new Error(args.action);
  };
  return { call, created };
}

test('session parsing reads the web app storage and rejects missing organization', () => {
  const session = parseSession(JSON.stringify(captured));
  assert.equal(session.accountName, 'alice'); assert.equal(session.organization, 'org1'); assert.equal(session.workspace, 'ws1');
  assert.throws(() => parseSession({ accessToken: 'x', user: JSON.stringify({ state: { user: { id: 'u' } } }) }), /organization/);
  assert.throws(() => parseSession('not json'), /JSON/);
  const payload = Buffer.from(JSON.stringify({ exp: 2000000000 })).toString('base64url');
  assert.equal(jwtExpiry(`h.${payload}.s`), 2000000000000);
});

test('seeding defaults to 30 on the latest repository in batches of 10 and registers every task', async () => {
  const { http, calls } = fakeHttp(), lib = library(), progress = [];
  const client = new CodeRabbitClient(http, parseSession(captured));
  const result = await runSeeding({ endpoint: 'https://mcp.example/mcp', folderId: 'folder', client, library: lib.call, delayMs: 0, onProgress: p => progress.push(p) });
  assert.equal(result.done, 30); assert.equal(lib.created.length, 30); assert.equal(result.branch, 'develop');
  const listing = calls.find(c => c.url.includes('getAllRepos'));
  assert.match(decodeURIComponent(listing.url), /"repoSort":"activity"/);
  const enqueue = calls.filter(c => c.url.includes('enqueueCodingTask'));
  assert.equal(enqueue.length, 30); assert.equal(enqueue[0].headers.authorization, 'Bearer tok-1'); assert.equal(enqueue[0].headers['x-coderabbitai-organization'], 'org1');
  const body = JSON.parse(enqueue[0].body)[0];
  assert.equal(body.repoId, 'r-new'); assert.equal(body.branch_name, 'develop');
  assert.match(body.source.instructions, /^https:\/\/mcp\.example\/mcp\/seeds\/folder\/a{64} /);
  assert.equal(progress.at(-1).message, 'Done');
});

test('an expired session logs in again once and continues', async () => {
  const { http } = fakeHttp({ expireOnce: true }), lib = library();
  let relogins = 0;
  const client = new CodeRabbitClient(http, parseSession(captured));
  const result = await runSeeding({ count: 3, endpoint: 'https://mcp.example/mcp', folderId: 'f', client, library: lib.call, delayMs: 0,
    relogin: async () => { relogins++; return parseSession({ ...captured, accessToken: 'tok-2' }); } });
  assert.equal(result.done, 3); assert.equal(relogins, 1); assert.equal(client.session.access, 'tok-2');
});

test('an expired session without relogin stops with SessionExpiredError', async () => {
  const { http } = fakeHttp({ expireOnce: true }), lib = library();
  await assert.rejects(runSeeding({ count: 2, endpoint: 'https://m/mcp', folderId: 'f', client: new CodeRabbitClient(http, parseSession(captured)), library: lib.call, delayMs: 0 }), SessionExpiredError);
});

test('no repository triggers starter repository creation, then seeds it', async () => {
  let created = false;
  const base = fakeHttp({ repos: [] }), lib = library();
  const http = async request => request.url.includes('getAllRepos') && created ? { status: 200, body: trpc({ repos: [{ id: 'r1', name: 'alice/seed', defaultBranch: 'main' }] }) } : base.http(request);
  const result = await runSeeding({ count: 1, endpoint: 'https://m/mcp', folderId: 'f', client: new CodeRabbitClient(http, parseSession(captured)), library: lib.call, delayMs: 0, createRepository: async () => { created = true; } });
  assert.equal(created, true); assert.equal(result.repo, 'alice/seed'); assert.equal(result.done, 1);
});

test('an unknown enqueue result stops without retrying the same seed', async () => {
  const lib = library(); let attempts = 0;
  const http = async request => request.url.includes('enqueue') ? (attempts++, { status: 200, body: trpc({}) }) : fakeHttp().http(request);
  await assert.rejects(runSeeding({ count: 5, endpoint: 'https://m/mcp', folderId: 'f', client: new CodeRabbitClient(http, parseSession(captured)), library: lib.call, delayMs: 0 }), /task ID/);
  assert.equal(attempts, 1); assert.equal(lib.created.length, 0);
});

test('public MCP endpoint must be HTTPS and ends in /mcp', () => {
  assert.equal(publicMcpEndpoint('https://a.trycloudflare.com/'), 'https://a.trycloudflare.com/mcp');
  assert.equal(publicMcpEndpoint('https://x.example/base/mcp'), 'https://x.example/base/mcp');
  assert.throws(() => publicMcpEndpoint(''), /tunnel/); assert.throws(() => publicMcpEndpoint('http://x.example'), /HTTPS/);
});

test('GitHub device flow and starter repository use the REST API with a random code file', async () => {
  const calls = []; let polls = 0;
  const http = async request => {
    calls.push(request);
    if (request.url.endsWith('/access_token')) return { status: 200, body: JSON.stringify(++polls < 2 ? { error: 'authorization_pending' } : { access_token: 'gho_x' }) };
    if (request.url.endsWith('/user/repos')) return { status: 201, body: JSON.stringify({ full_name: 'alice/seed-1' }) };
    if (request.method === 'PUT') return { status: 201, body: '{}' };
    throw new Error(request.url);
  };
  const token = await pollDeviceToken(http, { device_code: 'd', user_code: 'U', verification_uri: '', interval: 5, expires_in: 60 }, undefined, async () => {});
  assert.equal(token, 'gho_x');
  assert.equal(await createStarterRepository(http, token, () => 0.5), 'alice/seed-1');
  const repo = JSON.parse(calls.find(c => c.url.endsWith('/user/repos')).body);
  assert.equal(repo.private, true); assert.equal(repo.auto_init, true);
  const put = calls.find(c => c.method === 'PUT');
  assert.match(put.url, /\/repos\/alice\/seed-1\/contents\/src\/[a-z]+_[a-z]+\.py$/);
  assert.match(Buffer.from(JSON.parse(put.body).content, 'base64').toString(), /def [a-z]+_[a-z]+\(/);
  assert.match(randomSource(() => 0).content, /VALUES = \[0,0,0,0,0,0,0,0\]/);
});
