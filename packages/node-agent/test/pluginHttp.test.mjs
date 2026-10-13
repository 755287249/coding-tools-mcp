import test from 'node:test';
import assert from 'node:assert/strict';
import { assertPublicUrl, pluginHttp, privateAddress, validatePluginPackage } from '../dist/management/pluginHttp.js';

const publicResolver = async () => ['1.1.1.1'];

test('plugin proxy accepts public HTTPS and rejects SSRF targets', async () => {
  assert.equal((await assertPublicUrl('https://example.com/path', publicResolver)).hostname, 'example.com');
  for (const url of ['http://example.com/', 'https://localhost/a', 'https://127.0.0.1/a', 'https://service.internal/a'])
    await assert.rejects(assertPublicUrl(url, publicResolver), /HTTPS|not allowed/);
  for (const address of ['127.0.0.1', '10.1.2.3', '172.16.0.1', '192.168.2.1', '169.254.1.1', '::1', 'fd00::1'])
    assert.equal(privateAddress(address), true, address);
  assert.equal(privateAddress('1.1.1.1'), false);
});

test('plugin proxy forwards cookie, returns response headers and never follows redirects', async () => {
  let seen;
  const fetcher = async (url, init) => {
    seen = { url, init };
    return new Response('redirect', { status: 302, headers: [['location', 'https://example.com/next'], ['set-cookie', 'sid=one; Path=/; Secure']] });
  };
  const result = await pluginHttp({ url: 'https://example.com/start', method: 'POST', body: '{}', headers: { authorization: 'Bearer x', cookie: 'old=1', host: 'evil.test' } }, fetcher, publicResolver);
  assert.equal(result.status, 302);
  assert.equal(result.headers.location, 'https://example.com/next');
  assert.deepEqual(result.headers['set-cookie'], ['sid=one; Path=/; Secure']);
  assert.equal(seen.init.redirect, 'manual');
  assert.equal(seen.init.headers.cookie, 'old=1');
  assert.equal(seen.init.headers.host, undefined);
});

test('plugin packages require a safe id, manifest and single-file code', () => {
  const value = validatePluginPackage({ format: 1, manifest: { id: 'hello-plugin', name: 'Hello', version: '1.0.0', permissions: { hosts: ['EXAMPLE.com'] } }, code: 'console.log(1)' });
  assert.deepEqual(value.manifest.permissions.hosts, ['example.com']);
  assert.throws(() => validatePluginPackage({ format: 1, manifest: { id: '../bad', name: 'Bad', version: '1' }, code: 'x' }), /Invalid plugin/);
});
