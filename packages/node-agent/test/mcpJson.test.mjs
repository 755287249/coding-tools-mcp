import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { mergeMcpJson } from '../dist/management/mcpJson.js';

test('mergeMcpJson adds servers and keeps existing entries', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'mcpjson-'));
  try {
    await writeFile(path.join(dir, '.mcp.json'), JSON.stringify({ mcpServers: { old: { command: 'a' } }, other: 1 }));
    const names = await mergeMcpJson(dir, { fresh: { url: 'https://example.com/mcp' } });
    assert.deepEqual(names, ['fresh']);
    const saved = JSON.parse(await readFile(path.join(dir, '.mcp.json'), 'utf8'));
    assert.equal(saved.other, 1);
    assert.equal(saved.mcpServers.old.command, 'a');
    assert.equal(saved.mcpServers.fresh.url, 'https://example.com/mcp');
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('mergeMcpJson rejects bad input without touching the file', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'mcpjson-'));
  try {
    await writeFile(path.join(dir, '.mcp.json'), 'not json');
    await assert.rejects(mergeMcpJson(dir, { x: { command: 'a' } }), /not valid JSON/);
    assert.equal(await readFile(path.join(dir, '.mcp.json'), 'utf8'), 'not json');
    await assert.rejects(mergeMcpJson(dir, { x: { args: [] } }), /command or url/);
    await assert.rejects(mergeMcpJson(dir, { '../x': { command: 'a' } }), /invalid MCP server name/);
    await assert.rejects(mergeMcpJson(dir, {}), /1 to 64/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
