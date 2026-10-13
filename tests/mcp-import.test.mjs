import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';

const source = await readFile(new URL('../src/lib/plugins/mcp-import.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { detectImport, serverProblems } = await import(`data:text/javascript,${encodeURIComponent(js)}`);

test('detects plugin packages', () => {
  assert.equal(detectImport({ format: 1, manifest: {}, code: '' }).kind, 'plugin');
});

test('detects MCP configs in common shapes', () => {
  for (const value of [
    { mcpServers: { a: { command: 'npx' } } },
    { servers: { a: { url: 'https://x/mcp' } } },
    { mcp: { servers: { a: { command: 'x' } } } },
    { a: { command: 'npx' }, b: { url: 'https://x' } },
  ]) assert.equal(detectImport(value).kind, 'mcp', JSON.stringify(value));
  assert.equal(detectImport({ command: 'npx', args: ['x'] }).kind, 'mcp-single');
  assert.equal(detectImport({ url: 'https://x/mcp', type: 'http' }).kind, 'mcp-single');
});

test('rejects unrelated JSON', () => {
  for (const value of [null, [], 'x', { a: 1 }, { a: { b: 1 } }, {}]) assert.equal(detectImport(value).kind, 'unknown');
});

test('reports invalid servers', () => {
  assert.deepEqual(serverProblems({ ok: { command: 'x' }, 'bad/name': { command: 'x' }, empty: {} }), { invalidNames: ['bad/name'], missingTarget: ['empty'] });
});
