import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function loadModule() {
  const source = await readFile(path.join(root, 'src/lib/connect/cloudflare-route.ts'), 'utf8');
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  });
  return import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
}

test('route hostname accepts bare hosts and URLs', async () => {
  const { routeHostname } = await loadModule();
  assert.equal(routeHostname('755.cc.cd'), '755.cc.cd');
  assert.equal(routeHostname(' https://MCP.Example.com/mcp '), 'mcp.example.com');
  assert.equal(routeHostname('mcp.example.com:443'), 'mcp.example.com');
  assert.equal(routeHostname(''), null);
  assert.equal(routeHostname('localhost'), null);
  assert.equal(routeHostname('127.0.0.1'), null);
  assert.equal(routeHostname('bad_host.example.com'), null);
});

test('zone guess keeps multi-label and free-domain suffixes', async () => {
  const { guessZone, splitForZone, hostnameSplits } = await loadModule();
  assert.equal(guessZone('755.cc.cd'), '755.cc.cd');
  assert.equal(guessZone('mcp.755.cc.cd'), '755.cc.cd');
  assert.equal(guessZone('mcp.example.com'), 'example.com');
  assert.equal(guessZone('example.com'), 'example.com');
  assert.equal(guessZone('mcp.example.co.uk'), 'example.co.uk');
  assert.equal(guessZone('mcp.example.com.cn'), 'example.com.cn');
  assert.equal(guessZone('api.abc.io'), 'abc.io');
  assert.deepEqual(splitForZone('755.cc.cd', '755.cc.cd'), { subdomain: '', zone: '755.cc.cd' });
  assert.deepEqual(splitForZone('a.b.example.com', 'example.com'), { subdomain: 'a.b', zone: 'example.com' });
  assert.deepEqual(
    hostnameSplits('mcp.755.cc.cd').map((split) => split.zone),
    ['cc.cd', '755.cc.cd', 'mcp.755.cc.cd'],
  );
});

test('service URL points at the local listener with an IP literal', async () => {
  const { routeServiceUrl } = await loadModule();
  assert.equal(routeServiceUrl(18790), 'http://127.0.0.1:18790');
  assert.equal(routeServiceUrl(18790, '0.0.0.0'), 'http://127.0.0.1:18790');
  assert.equal(routeServiceUrl(8080, '192.168.1.5'), 'http://192.168.1.5:8080');
  assert.equal(routeServiceUrl(8080, '::'), 'http://[::1]:8080');
  assert.equal(routeServiceUrl(8080, 'fe80::1'), 'http://[fe80::1]:8080');
});
