import test from 'node:test';
import assert from 'node:assert/strict';
import { pluginHttp, pluginRequestAllowed } from '../dist/management/pluginHttp.js';

test('plugin proxy allows only CodeRabbit tRPC and GitHub repository endpoints', () => {
  assert.ok(pluginRequestAllowed('GET', 'https://app.coderabbit.ai/trpc/repositories.getAllRepos?batch=1&input=%7B%7D'));
  assert.ok(pluginRequestAllowed('POST', 'https://app.coderabbit.ai/trpc/codingAgent.enqueueCodingTask?batch=1'));
  assert.ok(pluginRequestAllowed('POST', 'https://github.com/login/device/code'));
  assert.ok(pluginRequestAllowed('PUT', 'https://api.github.com/repos/me/seed-1/contents/src/a.py'));
  for (const [method, url] of [['GET', 'http://127.0.0.1:3789/admin/api/config'], ['POST', 'https://app.coderabbit.ai.evil.test/trpc/x?batch=1'],
    ['DELETE', 'https://api.github.com/user/repos'], ['PUT', 'https://api.github.com/repos/me/r/contents/../x'], ['GET', 'https://github.com/login/device/code']])
    assert.equal(pluginRequestAllowed(method, url), false, url);
});

test('plugin proxy drops unknown headers, never follows redirects and rejects disallowed URLs', async () => {
  let seen;
  const fetcher = async (url, init) => { seen = { url, init }; return new Response('{"ok":1}', { status: 200 }); };
  const result = await pluginHttp({ url: 'https://app.coderabbit.ai/trpc/a.b?batch=1', method: 'POST', body: '{}', headers: { authorization: 'Bearer x', cookie: 'c=1', 'x-coderabbitai-organization': 'org' } }, fetcher);
  assert.deepEqual(result, { status: 200, body: '{"ok":1}' });
  assert.equal(seen.init.redirect, 'manual'); assert.equal(seen.init.headers.cookie, undefined); assert.equal(seen.init.headers.authorization, 'Bearer x');
  await assert.rejects(pluginHttp({ url: 'https://example.com/', method: 'GET' }, fetcher), /not allowed/);
});
