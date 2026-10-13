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

test('host iframe uses the isolated same-origin blob loader and manifest permission gates', async () => {
  const [dock, frame, runner, loader, tauri] = await Promise.all([
    readFile(new URL('../src/lib/components/plugins/PluginDock.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../src/lib/components/plugins/PluginFrame.svelte', import.meta.url), 'utf8'),
    readFile(new URL('../static/plugin-frame.html', import.meta.url), 'utf8'),
    readFile(new URL('../static/plugin-frame.js', import.meta.url), 'utf8'),
    readFile(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
  ]);
  assert.match(dock, /PluginFrame/);
  assert.match(frame, /sandbox="allow-scripts"/);
  assert.match(frame, /plugin-frame\.html/);
  assert.doesNotMatch(frame, /srcdoc/);
  assert.match(runner, /script-src 'self' blob:/);
  assert.match(loader, /createObjectURL\(new Blob/);
  const security=JSON.parse(tauri).app.security;
  assert.match(security.csp['frame-src'],/'self'/);
  assert.match(security.csp['script-src'],/blob:/);
  assert.match(security.devCsp['frame-src'],/'self'/);
  assert.match(security.devCsp['script-src'],/blob:/);
  assert.match(frame, /permissions\.seedLibrary/);
  assert.match(frame, /hostAllowed/);
});

test('CodeRabbit userscript keeps its independent release version', async () => {
  const [registry,catalog]=await Promise.all([
    readFile(new URL('../src/lib/plugins/registry.ts', import.meta.url),'utf8'),
    readFile(new URL('../src/lib/components/workspace/FeatureCatalog.svelte', import.meta.url),'utf8'),
  ]);
  assert.match(registry,/userscriptVersion: '1\.4\.0'/);
  assert.match(catalog,/userscriptVersion \?\? appPlugin\.version/);
});
