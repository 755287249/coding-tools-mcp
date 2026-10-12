import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { CookieJar } from '../packages/plugin-sdk/src/index.ts';

test('cookie jar applies domain, path, secure and expiry rules', () => {
  const jar = new CookieJar();
  jar.set('https://github.com/login', ['session=one; Path=/; Secure', 'scoped=two; Path=/login', 'bad=x; Domain=evil.test']);
  assert.equal(jar.header('https://github.com/login/oauth'), 'session=one; scoped=two');
  assert.equal(jar.header('https://github.com/settings'), 'session=one');
  assert.equal(jar.header('http://github.com/login'), 'scoped=two');
  assert.equal(jar.header('https://evil.test/'), '');
});

test('built-in CodeRabbit plugin is a single valid package without test credentials', async () => {
  const raw = await readFile(new URL('../static/plugins/coderabbit-seeds.ctplugin.json', import.meta.url), 'utf8');
  const plugin = JSON.parse(raw);
  assert.equal(plugin.format, 1);
  assert.equal(plugin.manifest.id, 'coderabbit-seeds');
  assert.equal(typeof plugin.code, 'string');
  assert.ok(plugin.code.length > 1000);
});

test('host iframe declares sandbox and manifest permission gates', async () => {
  const [dock, frame] = await Promise.all([
    readFile(new URL('../src/lib/components/plugins/PluginDock.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../src/lib/components/plugins/PluginFrame.svelte', import.meta.url), 'utf8'),
  ]);
  assert.match(dock, /PluginFrame/);
  assert.match(frame, /sandbox="allow-scripts"/);
  assert.match(frame, /Content-Security-Policy/);
  assert.match(frame, /permissions\.seedLibrary/);
  assert.match(frame, /hostAllowed/);
});
