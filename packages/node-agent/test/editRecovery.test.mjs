import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { EDIT_PROPOSAL_TTL_MS } from '../dist/editRecovery.js';
import { runtimeForFolderId } from '../dist/folderRuntime.js';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import { editBlastRadius, unchangedLineEndingChanges } from '../dist/fileTools.js';

function proposals(ctx) {
  return runtimeForFolderId(ctx, 'repo').editProposals;
}

function config(root, dataDir) {
  return {
    host: '127.0.0.1',
    port: 0,
    dataDir,
    permissionMode: 'trusted',
    management: { enabled: false },
    oauth: {
      clientId: 'chatgpt',
      password: 'edit-recovery-password',
      tokenSecret: 'edit-recovery-token-secret'
    },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: {
      blockingConcurrency: 4,
      processConcurrency: 4,
      activeSessionLimit: 16,
      maxOutputBytes: 1024 * 1024
    }
  };
}

async function fixture(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-edit-recovery-root-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-edit-recovery-data-'));
  const ctx = await createToolContext(config(root, dataDir));
  t.after(async () => {
    await ctx.conversations.flush();
    await ctx.usageStore.flush();
    await rm(root, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 20 });
  });
  const meta = { 'openai/session': `edit-recovery-${Date.now()}-${Math.random()}` };
  const selected = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(selected.ok, true);
  return { root, ctx, meta };
}

async function proposal(ctx, meta, file = 'main.txt', replacement = 'let value = 2;') {
  const result = await callTool(ctx, 'edit_file', {
    path: file,
    edits: [{
      type: 'replace',
      old_text: 'let value = 1;',
      new_text: replacement
    }]
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.status, 'proposal_required');
  return result;
}

test('newline churn detector identifies EOL changes on unchanged content', () => {
  const changes = unchangedLineEndingChanges(
    'one\ntwo\nTHREE\nfour\nfive\n',
    'one\r\ntwo\r\nTHREE-EDITED\r\nfour\r\nfive\r\n'
  );
  assert.deepEqual(changes.map(change => change.before_line), [1, 2, 4, 5]);
  assert.ok(changes.every(change => change.before_eol === '\n'));
  assert.ok(changes.every(change => change.after_eol === '\r\n'));
});

test('newline churn detector allows removing the delimiter before a deleted unterminated final line', () => {
  assert.deepEqual(unchangedLineEndingChanges('alpha\nbeta\r\ngamma', 'alpha\nbeta'), []);
});

test('newline churn detector does not misalign repeated content after a legitimate deletion', () => {
  assert.deepEqual(unchangedLineEndingChanges('alpha\nx\r\nx\nbeta\n', 'alpha\nx\nbeta\n'), []);
});

test('edit blast-radius detector rejects whole-file churn from a narrow contract', () => {
  const original = `${Array.from({ length: 200 }, (_, index) => `line-${index + 1}`).join('\n')}\n`;
  const updated = `${Array.from({ length: 200 }, (_, index) => `rewritten-${index + 1}`).join('\n')}\n`;
  const radius = editBlastRadius(original, updated, [
    { type: 'replace', old_text: 'line-100', new_text: 'LINE-100' }
  ]);
  assert.equal(radius.excessive, true);
  assert.ok(radius.changed_line_count > radius.allowed_changed_line_count);
  assert.ok(radius.change_ratio >= 0.6);
});

test('edit blast-radius detector allows a precise local change', () => {
  const original = `${Array.from({ length: 200 }, (_, index) => `line-${index + 1}`).join('\n')}\n`;
  const updated = original.replace('line-100', 'LINE-100');
  const radius = editBlastRadius(original, updated, [
    { type: 'replace', old_text: 'line-100', new_text: 'LINE-100' }
  ]);
  assert.equal(radius.excessive, false);
  assert.equal(radius.changed_line_count, 2);
});

test('edit blast-radius detector measures disjoint precise edits instead of the span between them', () => {
  const original = `${Array.from({ length: 200 }, (_, index) => `line-${index + 1}`).join('\n')}\n`;
  const updated = original
    .replace('line-10', 'LINE-10')
    .replace('line-190', 'LINE-190');
  const radius = editBlastRadius(original, updated, [
    { type: 'replace', old_text: 'line-10', new_text: 'LINE-10' },
    { type: 'replace', old_text: 'line-190', new_text: 'LINE-190' }
  ]);
  assert.equal(radius.excessive, false);
  assert.equal(radius.changed_line_count, 4);
  assert.equal(radius.change_measure, 'bounded_line_diff');
});

test('ambiguous exact edit returns a bounded proposal without writing', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'main.txt');
  await writeFile(file, 'let  value = 1;\n');

  const result = await proposal(ctx, meta);
  assert.equal(result.applied, false);
  assert.equal(result.proposal_ttl_seconds, 300);
  assert.match(result.proposal_id, /^[0-9a-f]{32}$/);
  assert.equal(result.actual_text, 'let  value = 1;');
  assert.equal(result.requested_old_text, 'let value = 1;');
  assert.equal(result.requested_new_text, 'let value = 2;');
  assert.equal(result.proposed_content, 'let value = 2;\n');
  assert.equal(result.proposed_content_included, true);
  assert.match(result.proposed_content_sha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(result.accepted_formats, ['accept', 'replacement', 'patch']);
  assert.equal(result.preferred_format, 'replacement');
  assert.equal(result.proposal_patch_format, 'unified_diff_single_file_single_hunk');
  assert.equal(result.next_action, 'apply_proposal');
  assert.equal(await readFile(file, 'utf8'), 'let  value = 1;\n');
  assert.equal(proposals(ctx).size, 1);
});

test('precise edit contracts aggregate invalid fields with guarded recovery metadata', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'old\n');

  const invalid = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    edits: [{ type: 'replace', old_text: 'old', anchor: 'unexpected' }]
  }, meta);
  assert.equal(invalid.ok, false, JSON.stringify(invalid));
  assert.equal(invalid.error.code, 'EDIT_CONTRACT_INVALID');
  assert.equal(invalid.error.details.issue_count, 2);
  assert.equal(invalid.error.details.path, 'main.txt');
  assert.match(invalid.error.details.actual_sha256, /^[0-9a-f]{64}$/);
  assert.equal(invalid.error.details.recovery_actions[1].tool, 'edit');
  assert.equal(await readFile(path.join(root, 'main.txt'), 'utf8'), 'old\n');

  const mixed = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    edits: [{ type: 'replace', old_text: 'old', new_text: 'new' }],
    apply_proposal: { proposal_id: '0'.repeat(32) }
  }, meta);
  assert.equal(mixed.ok, false, JSON.stringify(mixed));
  assert.equal(mixed.error.code, 'EDIT_CONTRACT_INVALID');
  assert.equal(mixed.error.details.recovery_actions[0].action, 'choose_edit_mode');
});

test('canonical edit rejects no-op during cheap preflight before harness tracking', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'same\n');

  const result = await callTool(ctx, 'edit', {
    files: [{
      path: 'main.txt',
      edits: [{ type: 'replace', old_text: 'same', new_text: 'same' }]
    }]
  }, meta);

  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.error.code, 'PATCH_FAILED');
  assert.ok(Number(result.phase_durations_ms?.preflight_ms) >= 0, JSON.stringify(result));
  assert.equal(Object.hasOwn(result.phase_durations_ms ?? {}, 'harness_begin_ms'), false);
  assert.equal(await readFile(path.join(root, 'main.txt'), 'utf8'), 'same\n');
});

test('edit_many reports the failed file index before writing any file', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'first.txt'), 'first\n');
  await writeFile(path.join(root, 'second.txt'), 'second\n');

  const result = await callTool(ctx, 'edit_many', {
    files: [
      {
        path: 'first.txt',
        edits: [{ type: 'replace', old_text: 'first', new_text: 'FIRST' }]
      },
      {
        path: 'second.txt',
        edits: [{ type: 'replace', old_text: 'second', anchor: 'unexpected' }]
      }
    ]
  }, meta);
  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.error.code, 'EDIT_CONTRACT_INVALID');
  assert.equal(result.error.details.file_index, 1);
  assert.equal(result.error.details.path, 'second.txt');
  assert.equal(await readFile(path.join(root, 'first.txt'), 'utf8'), 'first\n');
  assert.equal(await readFile(path.join(root, 'second.txt'), 'utf8'), 'second\n');
});

test('multi-edit keeps later guarded line scopes anchored to the original snapshot', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'anchor\none\ntwo\ntarget\nend\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    edits: [
      { type: 'insert_after', anchor: 'anchor\n', text: 'new1\nnew2\n' },
      { type: 'replace', old_text: 'target', new_text: 'TARGET', start_line: 4, end_line: 4 }
    ]
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(path.join(root, 'main.txt'), 'utf8'), 'anchor\nnew1\nnew2\none\ntwo\nTARGET\nend\n');
});

test('multi-edit preserves sequential fallback when a later edit depends on earlier output', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'one\nseed\nthree\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    edits: [
      { type: 'replace', old_text: 'seed', new_text: 'created' },
      { type: 'replace', old_text: 'created', new_text: 'final', start_line: 2, end_line: 2 }
    ]
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(path.join(root, 'main.txt'), 'utf8'), 'one\nfinal\nthree\n');
});

test('delete_lines applies the same expected_text guard as replace_lines', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'alpha\nbeta\ngamma\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    edits: [{
      type: 'delete_lines',
      start_line: 2,
      end_line: 2,
      expected_text: 'different'
    }]
  }, meta);
  assert.equal(result.ok, false, JSON.stringify(result));
  assert.equal(result.error.code, 'EDIT_EXPECTED_TEXT_MISMATCH');
  assert.equal(result.error.details.actual_text, 'beta');
  assert.equal(await readFile(path.join(root, 'main.txt'), 'utf8'), 'alpha\nbeta\ngamma\n');
});

test('replace_lines preserves untouched mixed line endings and follows the local EOL', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'mixed-lines.txt');
  const original = 'alpha\r\nbeta\r\ngamma\ndelta\n';
  await writeFile(file, original);

  const result = await callTool(ctx, 'edit_file', {
    path: 'mixed-lines.txt',
    edits: [{
      type: 'replace_lines',
      start_line: 3,
      end_line: 3,
      expected_text: 'gamma',
      new_text: 'GAMMA\nSECOND'
    }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'alpha\r\nbeta\r\nGAMMA\nSECOND\ndelta\n');
  assert.equal(result.newline_before, 'mixed');
  assert.equal(result.newline_after, 'mixed');
  assert.equal(result.newline_guard, 'passed');
  assert.equal(result.blast_radius_guard, 'passed');
  assert.equal(result.blast_radius.excessive, false);
});

test('text replacement adapts inserted newlines to the target line instead of the whole mixed file', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'mixed-text.txt');
  await writeFile(file, 'first\nsecond\r\nthird\r\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'mixed-text.txt',
    edits: [{ type: 'replace', old_text: 'first', new_text: 'FIRST\nEXTRA' }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'FIRST\nEXTRA\nsecond\r\nthird\r\n');
});

test('delete_lines preserves untouched mixed EOLs when deleting the final unterminated line', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'mixed-delete.txt');
  await writeFile(file, 'alpha\nbeta\r\ngamma');

  const result = await callTool(ctx, 'edit_file', {
    path: 'mixed-delete.txt',
    edits: [{ type: 'delete_lines', start_line: 3, end_line: 3, expected_text: 'gamma' }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'alpha\nbeta');
});

test('replace_lines keeps CRLF-only files CRLF', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'crlf-lines.txt');
  await writeFile(file, 'alpha\r\nbeta\r\ngamma\r\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'crlf-lines.txt',
    edits: [{ type: 'replace_lines', start_line: 2, end_line: 2, new_text: 'BETA\nSECOND' }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'alpha\r\nBETA\r\nSECOND\r\ngamma\r\n');
  assert.equal(result.newline_before, 'crlf');
  assert.equal(result.newline_after, 'crlf');
  assert.equal(result.newline_guard, 'passed');
});

test('replace_lines preserves an unterminated final line', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'replace-eof.txt');
  await writeFile(file, 'alpha\nbeta');

  const result = await callTool(ctx, 'edit_file', {
    path: 'replace-eof.txt',
    edits: [{ type: 'replace_lines', start_line: 2, end_line: 2, new_text: 'BETA' }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'alpha\nBETA');
});

test('delete_lines preserves normal middle-line semantics', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'delete-middle.txt');
  await writeFile(file, 'alpha\nbeta\ngamma\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'delete-middle.txt',
    edits: [{ type: 'delete_lines', start_line: 2, end_line: 2 }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'alpha\ngamma\n');
});

test('delete_lines does not trigger newline churn guard when repeated content shifts', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'delete-duplicate.txt');
  await writeFile(file, 'alpha\nx\r\nx\nbeta\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'delete-duplicate.txt',
    edits: [{ type: 'delete_lines', start_line: 2, end_line: 2 }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.newline_guard, 'passed');
  assert.equal(await readFile(file, 'utf8'), 'alpha\nx\nbeta\n');
});

test('delete_lines keeps the preceding newline when the deleted final line was terminated', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'delete-final-newline.txt');
  await writeFile(file, 'alpha\nbeta\n');

  const result = await callTool(ctx, 'edit_file', {
    path: 'delete-final-newline.txt',
    edits: [{ type: 'delete_lines', start_line: 2, end_line: 2 }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(await readFile(file, 'utf8'), 'alpha\n');
});

test('trailing empty line is addressable without confusing the preceding delimiter', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'trailing-empty-line.txt');
  await writeFile(file, 'alpha\nbeta\n');

  const replaced = await callTool(ctx, 'edit_file', {
    path: 'trailing-empty-line.txt',
    edits: [{ type: 'replace_lines', start_line: 3, end_line: 3, new_text: 'gamma' }]
  }, meta);
  assert.equal(replaced.ok, true, JSON.stringify(replaced));
  assert.equal(await readFile(file, 'utf8'), 'alpha\nbeta\ngamma');

  await writeFile(file, 'alpha\nbeta\n');
  const deleted = await callTool(ctx, 'edit_file', {
    path: 'trailing-empty-line.txt',
    edits: [{ type: 'delete_lines', start_line: 3, end_line: 3 }]
  }, meta);
  assert.equal(deleted.ok, true, JSON.stringify(deleted));
  assert.equal(await readFile(file, 'utf8'), 'alpha\nbeta');
});

test('dry-run edit plans replay once and reject stale reuse', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'old\n');

  const planned = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    dry_run: true,
    reason: 'guarded replay test',
    edits: [{ type: 'replace', old_text: 'old', new_text: 'new' }]
  }, meta);
  assert.equal(planned.ok, true, JSON.stringify(planned));
  assert.equal(planned.applied, false);
  assert.equal(planned.edit_plan.tool, 'edit');
  assert.equal(planned.edit_plan.arguments.dry_run, false);
  assert.equal(planned.edit_plan.arguments.reason, 'guarded replay test');
  assert.equal(planned.edit_plan.arguments.files[0].expected_sha256, planned.before_sha256);
  assert.equal(planned.edit_plan.expected_result.files[0].after_sha256, planned.after_sha256);
  assert.match(planned.edit_plan.plan_sha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(planned.edit_plan.stateful_dependencies, []);

  const replayed = await callTool(ctx, planned.edit_plan.tool, planned.edit_plan.arguments, meta);
  assert.equal(replayed.ok, true, JSON.stringify(replayed));
  assert.equal(replayed.applied, true);
  assert.equal(await readFile(path.join(root, 'main.txt'), 'utf8'), 'new\n');

  const stale = await callTool(ctx, planned.edit_plan.tool, planned.edit_plan.arguments, meta);
  assert.equal(stale.ok, false, JSON.stringify(stale));
  assert.equal(stale.error.code, 'FILE_VERSION_MISMATCH');
});

test('stale single-file edits return a complete explicit retry only after revalidation on current content', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'stale-direct.txt');
  const original = 'target\nkeep\n';
  const current = 'target\nexternal change\n';
  await writeFile(file, original);
  const staleSha = createHash('sha256').update(original).digest('hex');
  await writeFile(file, current);

  const stale = await callTool(ctx, 'edit', {
    files: [{
      path: 'stale-direct.txt',
      expected_sha256: staleSha,
      edits: [{ type: 'replace', old_text: 'target', new_text: 'updated' }]
    }]
  }, meta);
  assert.equal(stale.ok, false, JSON.stringify(stale));
  assert.equal(stale.error.code, 'FILE_VERSION_MISMATCH');
  assert.equal(stale.error.details.direct_recovery_action_count, 1);
  assert.equal(await readFile(file, 'utf8'), current, 'stale request itself must not write');
  const direct = stale.error.details.recovery_actions[0];
  assert.equal(direct.action, 'retry_guarded_edit');
  assert.equal(direct.tool, 'edit');
  assert.deepEqual(direct.required_arguments, []);
  assert.equal(direct.arguments.files[0].expected_sha256, stale.error.details.actual_sha256);

  const retried = await callTool(ctx, direct.tool, direct.arguments, meta);
  assert.equal(retried.ok, true, JSON.stringify(retried));
  assert.equal(await readFile(file, 'utf8'), 'updated\nexternal change\n');

  await writeFile(file, 'target removed\nexternal change\n');
  const unsafe = await callTool(ctx, 'edit', {
    files: [{
      path: 'stale-direct.txt',
      expected_sha256: stale.error.details.actual_sha256,
      edits: [{ type: 'replace', old_text: 'updated', new_text: 'again' }]
    }]
  }, meta);
  assert.equal(unsafe.ok, false, JSON.stringify(unsafe));
  assert.equal(unsafe.error.code, 'FILE_VERSION_MISMATCH');
  assert.equal(unsafe.error.details.direct_recovery_action_count ?? 0, 0);
  assert.equal(unsafe.error.details.recovery_actions[0].action, 'read_current_file');
});

test('single-file edit returns a bounded contextual diff for large files', async t => {
  const { root, ctx, meta } = await fixture(t);
  const lines = Array.from({ length: 10_000 }, (_, index) => `line-${index + 1}`);
  lines[4_999] = 'target-line';
  await writeFile(path.join(root, 'large.txt'), `${lines.join('\n')}\n`);

  const result = await callTool(ctx, 'edit', {
    files: [{ path: 'large.txt', edits: [{ type: 'replace', old_text: 'target-line', new_text: 'updated-line' }] }]
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.diff_truncated, false);
  assert.equal(result.diff_bytes, Buffer.byteLength(result.diff));
  assert.ok(result.diff_bytes < 1_024, `expected compact diff, got ${result.diff_bytes} bytes`);
  assert.match(result.diff, /line-4997/);
  assert.match(result.diff, /-target-line/);
  assert.match(result.diff, /\+updated-line/);
  assert.match(result.diff, /line-5003/);
  assert.doesNotMatch(result.diff, /line-1\n/);

  const huge = 'x'.repeat(40_000);
  const truncated = await callTool(ctx, 'edit', {
    files: [{ path: 'large.txt', edits: [{ type: 'replace', old_text: 'updated-line', new_text: huge }] }]
  }, meta);
  assert.equal(truncated.ok, true, JSON.stringify(truncated));
  assert.equal(truncated.diff_truncated, true);
  assert.ok(truncated.diff_bytes <= 32 * 1024);
});

test('single-file edit diff stays compact for CRLF files', async t => {
  const { root, ctx, meta } = await fixture(t);
  const lines = Array.from({ length: 200 }, (_, index) => `crlf-line-${index + 1}`);
  lines[99] = 'crlf-target';
  const file = path.join(root, 'crlf-diff.txt');
  await writeFile(file, `${lines.join('\r\n')}\r\n`);

  const result = await callTool(ctx, 'edit', {
    files: [{ path: 'crlf-diff.txt', edits: [{ type: 'replace', old_text: 'crlf-target', new_text: 'crlf-updated' }] }]
  }, meta);

  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.newline_before, 'crlf');
  assert.equal(result.newline_after, 'crlf');
  assert.ok(result.diff_bytes < 1_024, `expected compact CRLF diff, got ${result.diff_bytes} bytes`);
  assert.match(result.diff, /-crlf-target/);
  assert.match(result.diff, /\+crlf-updated/);
  assert.doesNotMatch(result.diff, /crlf-line-1\n/);
});

test('canonical edit applies multiple files atomically in one request', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'first.txt'), 'first\n');
  await writeFile(path.join(root, 'second.txt'), 'second\n');

  const result = await callTool(ctx, 'edit', {
    files: [
      { path: 'first.txt', edits: [{ type: 'replace', old_text: 'first', new_text: 'FIRST' }] },
      { path: 'second.txt', edits: [{ type: 'replace', old_text: 'second', new_text: 'SECOND' }] }
    ]
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.atomic, true);
  assert.equal(result.results.length, 2);
  assert.deepEqual(result.results.map(item => item.path), ['first.txt', 'second.txt']);
  assert.ok(result.results.every(item => item.changed === true));
  assert.equal(await readFile(path.join(root, 'first.txt'), 'utf8'), 'FIRST\n');
  assert.equal(await readFile(path.join(root, 'second.txt'), 'utf8'), 'SECOND\n');
});

test('dry-run edit_many plans replay atomically with per-file guards', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'first.txt'), 'first\n');
  await writeFile(path.join(root, 'second.txt'), 'second\n');

  const planned = await callTool(ctx, 'edit_many', {
    dry_run: true,
    files: [
      { path: 'first.txt', edits: [{ type: 'replace', old_text: 'first', new_text: 'FIRST' }] },
      { path: 'second.txt', edits: [{ type: 'replace', old_text: 'second', new_text: 'SECOND' }] }
    ]
  }, meta);
  assert.equal(planned.ok, true, JSON.stringify(planned));
  assert.equal(planned.edit_plan.tool, 'edit');
  assert.equal(planned.edit_plan.arguments.dry_run, false);
  assert.equal(planned.edit_plan.arguments.files.length, 2);
  assert.match(planned.edit_plan.plan_sha256, /^[0-9a-f]{64}$/);

  const replayed = await callTool(ctx, planned.edit_plan.tool, planned.edit_plan.arguments, meta);
  assert.equal(replayed.ok, true, JSON.stringify(replayed));
  assert.equal(await readFile(path.join(root, 'first.txt'), 'utf8'), 'FIRST\n');
  assert.equal(await readFile(path.join(root, 'second.txt'), 'utf8'), 'SECOND\n');
 });

test('edit returns candidate context, supports context disambiguation, and edits explicit multiple matches', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'main.txt');
  const original = 'fn first() {\n  return value;\n}\n\nfn second() {\n  return value;\n}\n';
  await writeFile(file, original);

  const ambiguous = await callTool(ctx, 'edit', {
    files: [{
      path: 'main.txt',
      edits: [{ type: 'replace', old_text: 'return value;', new_text: 'return result;' }]
    }]
  }, meta);
  assert.equal(ambiguous.ok, false, JSON.stringify(ambiguous));
  assert.equal(ambiguous.error.code, 'EDIT_MATCH_COUNT_MISMATCH');
  assert.equal(ambiguous.error.details.actual_occurrences, 2);
  assert.deepEqual(ambiguous.error.details.candidate_lines, [2, 6]);
  assert.equal(ambiguous.error.details.candidate_contexts.length, 2);
  assert.equal(ambiguous.error.details.candidate_contexts_truncated, false);
  assert.equal(ambiguous.error.details.direct_recovery_action_count, 2);
  const directCandidates = ambiguous.error.details.recovery_actions.filter(action => action.action === 'retry_match_candidate');
  assert.deepEqual(directCandidates.map(action => action.candidate_line), [2, 6]);
  assert.ok(directCandidates.every(action => action.required_arguments.length === 0));
  assert.ok(directCandidates.every(action => action.arguments.files[0].expected_sha256 === ambiguous.error.details.actual_sha256));
  const directSecond = await callTool(ctx, directCandidates[1].tool, directCandidates[1].arguments, meta);
  assert.equal(directSecond.ok, true, JSON.stringify(directSecond));
  assert.match(await readFile(file, 'utf8'), /fn second\(\) \{\n  return result;\n\}/);
  await writeFile(file, original);
  assert.equal(await readFile(file, 'utf8'), original);

  const selected = await callTool(ctx, 'edit', {
    files: [{
      path: 'main.txt',
      edits: [{
        type: 'replace',
        old_text: 'return value;',
        before_context: 'fn second() {\n  ',
        after_context: '\n}',
        new_text: 'return result;'
      }]
    }]
  }, meta);
  assert.equal(selected.ok, true, JSON.stringify(selected));
  assert.equal(selected.atomic, true);
  assert.equal(
    await readFile(file, 'utf8'),
    'fn first() {\n  return value;\n}\n\nfn second() {\n  return result;\n}\n'
  );

  await writeFile(file, original);
  const all = await callTool(ctx, 'edit', {
    files: [{
      path: 'main.txt',
      edits: [{
        type: 'replace',
        old_text: 'return value;',
        new_text: 'return result;',
        expected_occurrences: 2
      }]
    }]
  }, meta);
  assert.equal(all.ok, true, JSON.stringify(all));
  assert.equal(all.atomic, true);
  assert.equal(
    await readFile(file, 'utf8'),
    'fn first() {\n  return result;\n}\n\nfn second() {\n  return result;\n}\n'
  );
});

test('proposal accept and replacement modes apply atomically and consume the proposal', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'main.txt');
  await writeFile(file, 'let  value = 1;\n');

  const acceptedProposal = await proposal(ctx, meta);
  const accepted = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: { proposal_id: acceptedProposal.proposal_id }
  }, meta);
  assert.equal(accepted.ok, true, JSON.stringify(accepted));
  assert.equal(accepted.status, 'proposal_applied');
  assert.equal(accepted.proposal_apply_format, 'accept');
  assert.equal(accepted.applied, true);
  assert.match(accepted.diff, /\+let value = 2;/);
  assert.equal(await readFile(file, 'utf8'), 'let value = 2;\n');
  assert.equal(proposals(ctx).has(acceptedProposal.proposal_id), false);

  await writeFile(file, 'let  value = 1;\n');
  const dryRunProposal = await proposal(ctx, meta);
  const dryRun = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    dry_run: true,
    apply_proposal: { proposal_id: dryRunProposal.proposal_id }
  }, meta);
  assert.equal(dryRun.ok, true, JSON.stringify(dryRun));
  assert.equal(dryRun.dry_run, true);
  assert.equal(dryRun.applied, false);
  assert.equal(proposals(ctx).has(dryRunProposal.proposal_id), true);
  assert.equal(await readFile(file, 'utf8'), 'let  value = 1;\n');

  await writeFile(file, 'let  value = 1;\n');
  const replacementProposal = await proposal(ctx, meta);
  const replaced = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: {
      proposal_id: replacementProposal.proposal_id,
      replacement: 'let value = 3;'
    }
  }, meta);
  assert.equal(replaced.ok, true, JSON.stringify(replaced));
  assert.equal(replaced.proposal_apply_format, 'replacement');
  assert.equal(await readFile(file, 'utf8'), 'let value = 3;\n');
});

test('proposal rejects changed files, expired IDs and missing IDs', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'main.txt');
  await writeFile(file, 'let  value = 1;\n');

  const staleProposal = await proposal(ctx, meta);
  await writeFile(file, 'let  value = 9;\n');
  const stale = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: { proposal_id: staleProposal.proposal_id }
  }, meta);
  assert.equal(stale.ok, false);
  assert.equal(stale.error.code, 'EDIT_PROPOSAL_STALE');
  assert.equal(stale.error.category, 'conflict');
  assert.equal(stale.error.retryable, true);
  assert.equal(stale.error.details.reason, 'file_changed');
  assert.equal(await readFile(file, 'utf8'), 'let  value = 9;\n');

  await writeFile(file, 'let  value = 1;\n');
  const candidateProposal = await proposal(ctx, meta);
  proposals(ctx).get(candidateProposal.proposal_id).actualText = 'different-candidate';
  const candidateChanged = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: { proposal_id: candidateProposal.proposal_id }
  }, meta);
  assert.equal(candidateChanged.ok, false);
  assert.equal(candidateChanged.error.code, 'EDIT_PROPOSAL_STALE');
  assert.equal(candidateChanged.error.details.reason, 'candidate_changed');

  await writeFile(file, 'let  value = 1;\n');
  const expiredProposal = await proposal(ctx, meta);
  proposals(ctx).get(expiredProposal.proposal_id).createdAt -= EDIT_PROPOSAL_TTL_MS + 1;
  const expired = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: { proposal_id: expiredProposal.proposal_id }
  }, meta);
  assert.equal(expired.ok, false);
  assert.equal(expired.error.code, 'EDIT_PROPOSAL_NOT_FOUND');
  assert.equal(expired.error.details.reason, 'missing_or_expired');

  const missing = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: { proposal_id: '0'.repeat(32) }
  }, meta);
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'EDIT_PROPOSAL_NOT_FOUND');
});

test('proposal store remains bounded and evicts the oldest proposal', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'main.txt'), 'let  value = 1;\n');
  const ids = [];
  for (let index = 0; index < 201; index += 1) {
    const result = await proposal(ctx, meta, 'main.txt', `let value = ${index + 2};`);
    ids.push(result.proposal_id);
  }
  assert.equal(proposals(ctx).size, 200);
  assert.equal(proposals(ctx).has(ids[0]), false);
  assert.equal(proposals(ctx).has(ids.at(-1)), true);
});

test('large proposals accept an efficient restricted single-hunk patch', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'main.txt');
  await writeFile(file, 'let  value = 1;\n');
  const lines = Array.from({ length: 1200 }, (_, index) => `line-${String(index + 1).padStart(4, '0')}-payload`);
  const replacement = lines.join('\n');
  const result = await proposal(ctx, meta, 'main.txt', replacement);
  assert.equal(result.preferred_format, 'patch');
  assert.equal(result.proposed_content_included, true);

  const patch = [
    '--- a/proposal',
    '+++ b/proposal',
    '@@ -600,1 +600,1 @@',
    `-${lines[599]}`,
    '+LINE-0600-PATCHED',
    ''
  ].join('\n');
  const applied = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: { proposal_id: result.proposal_id, patch }
  }, meta);
  assert.equal(applied.ok, true, JSON.stringify(applied));
  assert.equal(applied.proposal_apply_format, 'patch');
  const content = await readFile(file, 'utf8');
  assert.match(content, /LINE-0600-PATCHED/);
  assert.doesNotMatch(content, new RegExp(lines[599]));
});

test('restricted proposal patches reject multiple hunks and inefficient patches', async t => {
  const { root, ctx, meta } = await fixture(t);
  const file = path.join(root, 'main.txt');
  await writeFile(file, 'let  value = 1;\n');
  const multipleProposal = await proposal(ctx, meta);
  const multiple = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: {
      proposal_id: multipleProposal.proposal_id,
      patch: [
        '--- a/proposal',
        '+++ b/proposal',
        '@@',
        '-let value = 2;',
        '+let value = 3;',
        '@@',
        '-let value = 3;',
        '+let value = 4;',
        ''
      ].join('\n')
    }
  }, meta);
  assert.equal(multiple.ok, false);
  assert.equal(multiple.error.code, 'EDIT_PROPOSAL_PATCH_INVALID');
  assert.equal(multiple.error.details.reason, 'single_file_single_hunk_required');

  const inefficientProposal = await proposal(ctx, meta);
  const inefficient = await callTool(ctx, 'edit_file', {
    path: 'main.txt',
    apply_proposal: {
      proposal_id: inefficientProposal.proposal_id,
      patch: [
        '--- a/proposal',
        '+++ b/proposal',
        '@@',
        '-let value = 2;',
        '+let value = 3;',
        ''
      ].join('\n')
    }
  }, meta);
  assert.equal(inefficient.ok, false);
  assert.equal(inefficient.error.code, 'EDIT_PROPOSAL_PATCH_INEFFICIENT');
  assert.equal(inefficient.error.details.recommended_format, 'replacement');
  assert.equal(inefficient.error.details.recommended_replacement, 'let value = 3;');
  assert.equal(await readFile(file, 'utf8'), 'let  value = 1;\n');
});

test('patch preflight reports ambiguous and multiple hunk failures with recovery actions', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'ambiguous.txt'), 'same\nother\nsame\n');
  const ambiguous = await callTool(ctx, 'patch_check', {
    patch: [
      '--- a/ambiguous.txt',
      '+++ b/ambiguous.txt',
      '@@',
      ' same',
      '+inserted',
      ''
    ].join('\n')
  }, meta);
  assert.equal(ambiguous.ok, false);
  assert.equal(ambiguous.error.code, 'PATCH_CONTEXT_AMBIGUOUS');
  assert.deepEqual(ambiguous.error.details.candidate_lines, [1, 3]);
  const candidateActions = ambiguous.error.details.recovery_actions;
  assert.equal(candidateActions.length, 2);
  assert.deepEqual(candidateActions.map(action => action.candidate_line), [1, 3]);
  assert.equal(candidateActions[0].tool, 'edit');
  assert.deepEqual(candidateActions[0].required_arguments, []);
  assert.deepEqual(candidateActions[0].arguments, {
    files: [{
      path: 'ambiguous.txt',
      edits: [{
        type: 'replace_lines',
        start_line: 1,
        end_line: 1,
        expected_text: 'same',
        new_text: 'same\ninserted'
      }]
    }]
  });

  await writeFile(path.join(root, 'multiple.txt'), 'actual\ncontent\n');
  const multiple = await callTool(ctx, 'patch_check', {
    patch: [
      '--- a/multiple.txt',
      '+++ b/multiple.txt',
      '@@',
      ' missing-one',
      '@@',
      ' missing-two',
      ''
    ].join('\n')
  }, meta);
  assert.equal(multiple.ok, false);
  assert.equal(multiple.error.code, 'PATCH_PREFLIGHT_FAILED');
  assert.equal(multiple.error.details.issue_count, 2);
  assert.equal(multiple.error.details.issues.length, 2);
  assert.equal(multiple.error.details.recovery_actions[0].action, 'switch_to_precise_edits');
});

test('patch expected hashes fail before Git with guarded recovery metadata', async t => {
  const { root, ctx, meta } = await fixture(t);
  await writeFile(path.join(root, 'hash.txt'), 'old\n');
  const result = await callTool(ctx, 'patch_check', {
    patch: [
      '--- a/hash.txt',
      '+++ b/hash.txt',
      '@@ -1,1 +1,1 @@',
      '-old',
      '+new',
      ''
    ].join('\n'),
    expected_sha256: { 'hash.txt': '0'.repeat(64) }
  }, meta);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'FILE_VERSION_MISMATCH');
  assert.match(result.error.details.actual_sha256, /^[0-9a-f]{64}$/);
  assert.equal(result.error.details.recovery_actions[0].tool, 'read_file');
  assert.equal(result.error.details.recovery_actions[1].tool, 'edit');
  assert.equal(await readFile(path.join(root, 'hash.txt'), 'utf8'), 'old\n');

  const absent = path.join(root, 'should-not-exist.txt');
  await assert.rejects(access(absent));
});
