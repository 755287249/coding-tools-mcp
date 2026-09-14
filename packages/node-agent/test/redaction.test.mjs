import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  containsSensitivePath, REDACTED, redactSensitiveText, redactToolOutput
} from '../dist/redaction.js';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';

function config(root, dataDir) {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode: 'trusted',
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'test-token-secret' },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 }
  };
}

async function fixture(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-redaction-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-redaction-state-'));
  const ctx = await createToolContext(config(root, dataDir));
  t.after(async () => {
    await ctx.usageStore.flush();
    await rm(root, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  });
  const meta = { 'openai/session': `redaction-${Date.now()}-${Math.random()}` };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  return { root, ctx, meta };
}

function serialized(value) {
  return JSON.stringify(value);
}

test('central redactor removes nested keys and structured secret text', () => {
  const output = redactToolOutput('server_info', {}, {
    ok: true,
    oauth_password: 'alpha-secret-value',
    stdout: '{"bearer_token":"beta-secret-value","safe":"visible"} api_key=gamma-secret-value',
    nested: { shared_secrets: { client_secret: 'delta-secret-value' } },
    token_count: 9
  });
  const text = serialized(output);
  for (const secret of ['alpha-secret-value', 'beta-secret-value', 'gamma-secret-value', 'delta-secret-value']) {
    assert.doesNotMatch(text, new RegExp(secret));
  }
  assert.match(text, /visible/);
  assert.equal(output.token_count, 9);
  assert.equal(output.sensitive_data_redacted, true);
  assert.ok(output.redaction_count >= 3);
  assert.ok(output.warnings.includes('Sensitive values were automatically redacted from the tool result.'));
});

test('text redaction covers bearer, JWT, private keys, known tokens, flags and URL credentials', () => {
  const jwt = 'eyJabcdefgh.eyJijklmnop.eyJqrstuvwx';
  const known = 'ghp_12345678901234567890';
  const privateKey = '-----BEGIN PRIVATE KEY-----\nabc123\n-----END PRIVATE KEY-----';
  const input = [
    `Authorization: Bearer ${jwt}`,
    known,
    privateKey,
    '--client-secret=command-secret',
    'password: assignment-secret',
    'https://demo:url-secret@example.invalid/path'
  ].join('\n');
  const redacted = redactSensitiveText(input);
  assert.ok(redacted.count >= 6);
  for (const secret of [jwt, known, 'abc123', 'command-secret', 'assignment-secret', 'url-secret']) {
    assert.doesNotMatch(redacted.value, new RegExp(secret));
  }
});

test('sensitive path detection and source-scoped fields match Rust', () => {
  assert.equal(containsSensitivePath('C:\\Users\\demo\\profiles.json'), true);
  assert.equal(containsSensitivePath('./.env'), true);
  assert.equal(containsSensitivePath('~/.ssh/id_ed25519'), true);
  assert.equal(containsSensitivePath('./.env.example'), false);
  assert.equal(containsSensitivePath('src/profile.ts'), false);

  const read = redactToolOutput('read_file', { path: '.env' }, { path: '.env', content: 'UNLABELED_READ' });
  assert.equal(read.content, REDACTED);

  const many = redactToolOutput('read_many', {}, {
    results: [
      { path: '.env', content: 'UNLABELED_BATCH' },
      { path: 'src/safe.ts', content: 'visible-source' }
    ]
  });
  assert.equal(many.results[0].content, REDACTED);
  assert.equal(many.results[1].content, 'visible-source');

  const search = redactToolOutput('search_text', {}, {
    matches: [{ path: '.npmrc', match: 'UNLABELED_MATCH', before: ['UNLABELED_BEFORE'] }]
  });
  assert.equal(search.matches[0].match, REDACTED);
  assert.equal(search.matches[0].before, REDACTED);

  const diff = redactToolOutput('git_diff', {}, {
    diff: 'diff --git a/.env b/.env\n+UNLABELED_DIFF', files: ['.env']
  });
  assert.equal(diff.diff, REDACTED);
  assert.doesNotMatch(serialized(diff), /UNLABELED_DIFF/);
});

test('normal metrics and hashes are not false positives', () => {
  const sha256 = '0123456789abcdef'.repeat(4);
  const output = redactToolOutput('server_info', {}, {
    token_count: 42,
    output_bytes: 1024,
    secret_present: true,
    toolset_revision: '8bca4fb80d5d74d9',
    sha256
  });
  assert.equal(output.token_count, 42);
  assert.equal(output.output_bytes, 1024);
  assert.equal(output.secret_present, true);
  assert.equal(output.sha256, sha256);
  assert.equal(output.sensitive_data_redacted, undefined);
});

test('read_file with a protected credential path is withheld before transport', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, '.env'), 'UNLABELED_FILE_VALUE\n');
  const result = await callTool(ctx, 'read_file', { path: '.env' }, meta);
  assert.equal(result.ok, true);
  assert.equal(result.content, REDACTED);
  assert.equal(result.sensitive_data_redacted, true);
  assert.ok(result.redaction_count >= 1);
  assert.doesNotMatch(serialized(result), /UNLABELED_FILE_VALUE/);
});

test('sensitive process sessions redact initial, delta, retained, resolved and listed output', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'profiles.json'), 'BARE_PROCESS_SECRET');
  await writeFile(path.join(root, 'secrets.json'), 'BARE_PROCESS_ERROR');
  const source = "const fs=require('node:fs');process.stdout.write(fs.readFileSync('profiles.json','utf8'));process.stderr.write(fs.readFileSync('secrets.json','utf8'))";
  const started = await callTool(ctx, 'exec_command', {
    program: path.basename(process.execPath),
    args: ['-e', source],
    workdir: '.',
    yield_time_ms: 10_000,
    output_mode: 'tail',
    operation_id: 'redaction-sensitive-process'
  }, meta);
  assert.equal(started.ok, true);
  assert.equal(started.sensitive_data_redacted, true);
  assert.equal(started.stdout, REDACTED);
  assert.equal(started.stderr, REDACTED);
  assert.doesNotMatch(serialized(started), /BARE_PROCESS_(?:SECRET|ERROR)/);

  const waited = await callTool(ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: 0,
    timeout_ms: 30_000,
    until: 'finalized',
    output_mode: 'delta',
    event_detail: 'full'
  }, meta);
  assert.equal(waited.event_detail, 'full');
  assert.equal(waited.sensitive_data_redacted, true);
  assert.ok(waited.events.every(event => !event.data || event.data === REDACTED));
  assert.doesNotMatch(serialized(waited), /BARE_PROCESS_(?:SECRET|ERROR)/);

  const retained = await callTool(ctx, 'read_output', {
    output_ref: `output://${started.session_id}/stdout`,
    offset: 0,
    limit: 4096
  }, meta);
  assert.equal(retained.content, REDACTED);
  assert.doesNotMatch(serialized(retained), /BARE_PROCESS_SECRET/);

  const resolved = await callTool(ctx, 'resolve_operation', {
    operation_id: started.operation_id,
    output_mode: 'tail'
  }, meta);
  assert.equal(resolved.stdout, REDACTED);
  assert.equal(resolved.sensitive_data_redacted, true);

  const listed = await callTool(ctx, 'list_sessions', { include_finalized: true }, meta);
  const summary = listed.sessions.find(session => session.session_id === started.session_id);
  assert.ok(summary);
  assert.equal(summary.sensitive_data_redacted, true);
  assert.doesNotMatch(serialized(summary), /BARE_PROCESS_(?:SECRET|ERROR)/);
});

test('generic token output is redacted across initial and retained process APIs', async t => {
  const { ctx, meta } = await fixture(t);
  const token = 'ghp_12345678901234567890';
  const started = await callTool(ctx, 'exec_command', {
    program: path.basename(process.execPath),
    args: ['-e', `process.stdout.write('${token}')`],
    yield_time_ms: 10_000,
    output_mode: 'tail'
  }, meta);
  assert.equal(started.stdout, REDACTED);
  assert.equal(started.sensitive_data_redacted, true);
  assert.doesNotMatch(serialized(started), new RegExp(token));

  await callTool(ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: started.latest_cursor,
    timeout_ms: 30_000,
    until: 'finalized',
    output_mode: 'none'
  }, meta);

  const retained = await callTool(ctx, 'read_output', {
    output_ref: `output://${started.session_id}/stdout`,
    offset: 0,
    limit: 4096
  }, meta);
  assert.equal(retained.content, REDACTED);
  assert.equal(retained.sensitive_data_redacted, true);
  assert.doesNotMatch(serialized(retained), new RegExp(token));
});
