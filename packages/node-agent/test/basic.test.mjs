import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile as execFileCallback } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { toolNames, tools } from '../dist/catalog.js';
import {
  RESTART_SUPERVISED_FLAG,
  RESTART_SUPERVISOR_ENV,
  RESTART_SUPERVISOR_VALUE,
  restartSupervisedFromArgv
} from '../dist/cliOptions.js';
import { captureGitRestoreSnapshot, restoreGitSnapshot } from '../dist/gitTools.js';
import { cargoRustEditionMetadata, rustfmtCommandArgs } from '../dist/formatterTools.js';
import { createToolContext } from '../dist/server.js';
import { callTool } from '../dist/tools.js';
import { MAX_RETAINED_COMMAND_GRAPHS, pruneRetainedCommandGraphs } from '../dist/processes.js';
import { fastContainingGitRoot, fastWorkspaceGitHead, fastWorkspaceGitIdentity } from '../dist/runtimeRevision.js';
import {
  AGENT_VERSION,
  BUILD_GIT_SHA,
  BUILD_SOURCE_CLEAN,
  BUILD_TOOL_EVOLUTION_PROPOSAL_IDS,
  CLIENT_COMPAT_VERSION
} from '../dist/version.js';

const execFile = promisify(execFileCallback);
const nodeProgram = path.basename(process.execPath);

function config(root, dataDir, permissionMode = 'trusted') {
  return {
    host: '127.0.0.1', port: 0, dataDir, permissionMode,
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'a sufficiently long test token secret' },
    folders: [{ id: 'repo', name: 'Repo', path: root }],
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 }
  };
}

async function context(permissionMode = 'trusted') {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-state-'));
  return { root, ctx: await createToolContext(config(root, dataDir, permissionMode)), meta: { 'openai/session': randomSession() } };
}

function randomSession() { return `session-${Math.random().toString(36).slice(2)}`; }

async function pathExists(value) {
  try { await access(value); return true; } catch { return false; }
}

async function installCustomFormatter(root, source, options = {}) {
  const toolsDir = path.join(root, 'tools');
  const configDir = path.join(root, '.coding-tools');
  await mkdir(toolsDir, { recursive: true });
  await mkdir(configDir, { recursive: true });
  const program = 'tools/custom-formatter.cjs';
  await writeFile(path.join(root, program), source);
  await writeFile(path.join(configDir, 'formatters.json'), `${JSON.stringify({
    formatters: {
      'company-template': {
        program,
        extensions: ['tmpl'],
        args: options.args ?? ['{files}'],
        ...(options.config ? { config: options.config } : {})
      }
    }
  }, null, 2)}\n`);
}

async function select(ctx, meta) {
  const result = await callTool(ctx, 'switch_workspace_folder', { folder_id: 'repo' }, meta);
  assert.equal(result.ok, true);
}

async function git(root, ...args) {
  const result = await execFile('git', args, { cwd: root, encoding: 'utf8' });
  return result.stdout.trim();
}

async function gitContext(permissionMode = 'trusted') {
  const state = await context(permissionMode);
  await git(state.root, 'init');
  await git(state.root, 'config', 'user.name', 'Node Agent Test');
  await git(state.root, 'config', 'user.email', 'node-agent@example.invalid');
  await writeFile(path.join(state.root, 'tracked.txt'), 'initial\n');
  await git(state.root, 'add', '--', 'tracked.txt');
  await git(state.root, 'commit', '-m', 'initial');
  await select(state.ctx, state.meta);
  return state;
}

test('catalog exactly matches the Rust P0 tool names', async () => {
  const registryMetadataPath = fileURLToPath(new URL('../../../src-tauri/src/tools/registry_metadata.rs', import.meta.url));
  const registryMetadata = await readFile(registryMetadataPath, 'utf8');
  const block = registryMetadata.split('pub const P0_TOOLS')[1].split('];')[0];
  const rustNames = [...block.matchAll(/^\s*"([a-z_]+)",\s*$/gm)].map(match => match[1]);
  assert.equal(tools.length, rustNames.length);
  assert.deepEqual(toolNames, rustNames);
});

test('agent plan tools track todos, progress and stable ids', async t => {
  const { root, ctx, meta } = await context();
  t.after(() => rm(root, { recursive: true, force: true }));
  await select(ctx, meta);

  let result = await callTool(ctx, 'set_todos', { todos: [
    { id: 'a', title: 'A', status: 'in_progress' },
    { id: 'b', title: 'B', status: 'in_progress' }
  ] }, meta);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'INVALID_ARGUMENT');

  result = await callTool(ctx, 'set_todos', { goal: 'Ship', todos: [
    { id: 'read', title: 'Read code', status: 'completed' },
    { id: 'build', title: 'Build UI', status: 'in_progress' }
  ] }, meta);
  assert.equal(result.ok, true);
  assert.equal(result.plan.completed, 1);
  assert.equal(result.plan.current.id, 'build');

  result = await callTool(ctx, 'report_progress', { message: 'Styling', percent: 40 }, meta);
  assert.equal(result.ok, true);
  assert.equal(result.progress.todo_id, 'build');
  result = await callTool(ctx, 'report_progress', { message: 'x', todo_id: 'missing' }, meta);
  assert.equal(result.ok, false);

  result = await callTool(ctx, 'update_plan', { explanation: 'UI done', plan: [
    { step: 'Read code', status: 'completed' },
    { step: 'Build UI', status: 'completed' },
    { step: 'Docs', status: 'in_progress' }
  ] }, meta);
  assert.equal(result.ok, true);
  assert.equal(result.plan.goal, 'Ship');
  assert.deepEqual(result.plan.todos.map(todo => todo.id), ['read', 'build', 'todo-1']);
  assert.equal(result.plan.progress.phase, '计划更新');

  result = await callTool(ctx, 'set_todos', { todos: [] }, meta);
  assert.equal(result.plan.cleared, true);
});

test('command execution schemas expose the timeout ceilings and deprecate application heartbeats', () => {
  const exec = tools.find(tool => tool.name === 'exec_command');
  const execMany = tools.find(tool => tool.name === 'exec_many');
  const wait = tools.find(tool => tool.name === 'wait_command');
  assert.ok(exec);
  assert.ok(execMany);
  assert.ok(wait);
  assert.equal(exec.inputSchema.properties.timeout_ms.maximum, 60 * 60_000);
  assert.equal(exec.inputSchema.properties.post_checks.items.properties.timeout_ms.maximum, 60 * 60_000);
  assert.equal(execMany.inputSchema.properties.commands.items.properties.timeout_ms.maximum, 60 * 60_000);
  assert.deepEqual(execMany.inputSchema.properties.commands.items.properties.run_if.enum, ['success', 'failure', 'always']);
  assert.equal(execMany.inputSchema.properties.commands.items.properties.run_if.default, 'success');
  assert.equal(execMany.inputSchema.properties.commands.items.properties.yield_time_ms.maximum, 30_000);
  assert.equal(execMany.inputSchema.properties.yield_time_ms.default, 20_000);
  assert.equal(execMany.inputSchema.properties.yield_time_ms.maximum, 300_000);
  assert.equal(wait.inputSchema.properties.timeout_ms.maximum, 60 * 60_000);
  assert.equal(wait.inputSchema.properties.timeout_ms.default, 20_000);
  assert.match(wait.inputSchema.properties.heartbeat_ms.description, /deprecated.*ignored.*transport/i);
});

test('exec_command exposes a retained focused-to-affected Node test workflow', async t => {
  const { root, ctx, meta } = await context();
  t.after(() => rm(root, { recursive: true, force: true }));
  await select(ctx, meta);
  await writeFile(path.join(root, 'focused.test.mjs'), [
    "import test from 'node:test';",
    "import assert from 'node:assert/strict';",
    "test('focused-case', () => assert.equal(1 + 1, 2));",
    ''
  ].join('\n'));

  let result = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['--test', '--test-name-pattern', 'focused-case', 'focused.test.mjs'],
    yield_time_ms: 30_000,
    timeout_ms: 30_000,
    output_mode: 'summary'
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.test_runner_capability.runner, 'node');
  assert.equal(result.test_workflow.current_stage, 'focused');
  assert.equal(result.test_workflow.next_stage, 'affected');
  assert.equal(result.test_workflow.advance_condition, 'command_ok=true');
  assert.deepEqual(result.test_workflow.next_actions[0].required_arguments, []);
  assert.deepEqual(result.test_workflow.next_actions[0].arguments, {
    program: nodeProgram,
    args: ['--test', 'focused.test.mjs'],
    workdir: '.'
  });

  result = await callTool(ctx, 'wait_command', {
    session_id: result.session_id,
    cursor: result.latest_cursor ?? 0,
    timeout_ms: 30_000,
    until: 'finalized',
    output_mode: 'none'
  }, meta);
  assert.equal(result.command_ok, true, JSON.stringify(result));
  assert.equal(result.test_workflow.current_stage, 'focused');
  assert.deepEqual(result.test_workflow.next_actions[0].arguments.args, ['--test', 'focused.test.mjs']);
});

test('portable restart supervision requires both the launcher flag and supervisor contract', async () => {
  assert.equal(restartSupervisedFromArgv(['node', 'cli.js'], {}), false);
  assert.equal(restartSupervisedFromArgv(['node', 'cli.js', RESTART_SUPERVISED_FLAG], {}), false);
  assert.equal(restartSupervisedFromArgv(['node', 'cli.js'], {
    [RESTART_SUPERVISOR_ENV]: RESTART_SUPERVISOR_VALUE
  }), false);
  assert.equal(restartSupervisedFromArgv(['node', 'cli.js', RESTART_SUPERVISED_FLAG], {
    [RESTART_SUPERVISOR_ENV]: RESTART_SUPERVISOR_VALUE
  }), true);

  const portableScriptPath = fileURLToPath(new URL('../scripts/build-portable.ps1', import.meta.url));
  const portableScript = await readFile(portableScriptPath, 'utf8');
  const portableStartTemplate = portableScript.match(/\$startBatTemplate = @'([\s\S]*?)'@/)?.[1];
  assert.ok(portableStartTemplate, 'portable start-node-agent.bat template should be present');
  assert.match(portableScript, /set "CTMCP_RESTART_SUPERVISOR=active-v1"/);
  assert.match(portableScript, /"%NODE_EXE%" "%AGENT_ENTRY%" --restart-supervised %\*/);
  assert.doesNotMatch(portableStartTemplate, /if not defined CTMCP_PORT set "CTMCP_PORT=3789"/);
  assert.match(portableStartTemplate, /set "CTMCP_LAUNCH_PORT=3789"/);
  assert.match(portableStartTemplate, /if defined CTMCP_PORT set "CTMCP_LAUNCH_PORT=%CTMCP_PORT%"/);
  assert.match(portableStartTemplate, /set "CTMCP_DEFAULT_WORKSPACE=%CTMCP_DATA_DIR%\\workspace"/);
  assert.match(portableStartTemplate, /if not defined CTMCP_WORKSPACES \(/);
  assert.match(portableStartTemplate, /if not exist "%CTMCP_DATA_DIR%\\agent\.json" if not exist "%CTMCP_DATA_DIR%\\workspace-profiles\.json" \(/);
  assert.match(portableStartTemplate, /if not exist "%CTMCP_DEFAULT_WORKSPACE%" mkdir "%CTMCP_DEFAULT_WORKSPACE%"/);
  assert.match(portableStartTemplate, /set "CTMCP_WORKSPACES=%CTMCP_DEFAULT_WORKSPACE%"/);
  assert.doesNotMatch(portableStartTemplate, /if not defined CTMCP_WORKSPACES set "CTMCP_WORKSPACES=%CTMCP_DEFAULT_WORKSPACE%"/);
  assert.match(portableStartTemplate, /Node Agent is already running on port %CTMCP_LAUNCH_PORT%/);
  assert.match(portableStartTemplate, /coding-tools-mcp-node/);
  assert.match(portableStartTemplate, /Port %CTMCP_LAUNCH_PORT% is already in use by another process/);
  assert.match(portableStartTemplate, /\$env:CTMCP_LAUNCH_PORT/);
  assert.match(portableScript, /catch \[System\.UnauthorizedAccessException\]/);
  assert.match(portableScript, /The ZIP is current/);
  assert.match(portableScript, /Get-Command tar\.exe/);
  assert.match(portableScript, /& \$tarExecutable -a -c -f \$zipPath -C \$PackagePath \./);
  assert.doesNotMatch(portableScript, /& \$tarExecutable -a -c -f \$zipPath -C \$packageParent \$packageName/);
  assert.doesNotMatch(portableScript, /Compress-Archive/);
  assert.match(portableScript, /\[switch\]\$AllowUnreleasedBuild/);
  assert.match(portableScript, /if \(-not \$AllowUnreleasedBuild\) \{/);
  assert.match(portableScript, /switch-node-agent-to-latest\.ps1'\) -Destination \(Join-Path \$editionPackage 'update-handoff\.ps1'\)/);
  assert.doesNotMatch(portableStartTemplate, /EXIT_CODE%"=="76/);
  assert.doesNotMatch(portableScript, /apply-verified-update\.ps1/);

  const handoffPath = fileURLToPath(new URL('../../../scripts/switch-node-agent-to-latest.ps1', import.meta.url));
  const handoff = await readFile(handoffPath, 'utf8');
  assert.match(handoff, /\[string\]\$FallbackPackageRoot/);
  assert.match(handoff, /Target failed health validation; rolling back/);
  assert.match(handoff, /Wait-NewAgentHealthy[^\r\n]+fallbackManifest\.nodeAgentVersion/);

  const repoLauncherPath = fileURLToPath(new URL('../../../start-node-agent.bat', import.meta.url));
  const repoLauncher = await readFile(repoLauncherPath, 'utf8');
  assert.match(repoLauncher, /set "CTMCP_RESTART_SUPERVISOR=active-v1"/);
  assert.match(repoLauncher, /dist\\cli\.js" --restart-supervised %\*/);

  const devServerPath = fileURLToPath(new URL('../scripts/dev-server.mjs', import.meta.url));
  const devServer = await readFile(devServerPath, 'utf8');
  assert.match(devServer, /CTMCP_RESTART_SUPERVISOR:\s*'active-v1'/);
});

test('server_info reports the package Agent version', async () => {
  const packagePath = fileURLToPath(new URL('../package.json', import.meta.url));
  const packageMetadata = JSON.parse(await readFile(packagePath, 'utf8'));
  const { ctx, meta } = await context();
  const info = await callTool(ctx, 'server_info', {}, meta);
  assert.equal(info.ok, true);
  assert.equal(AGENT_VERSION, packageMetadata.version);
  assert.equal(info.version, packageMetadata.version);
  assert.equal(info.client_compat_version, CLIENT_COMPAT_VERSION);
  assert.equal(typeof info.runtime_revision.process_started_at_ms, 'number');
  assert.equal(info.runtime_revision.build_git_sha, BUILD_GIT_SHA);
  assert.ok(BUILD_GIT_SHA === 'unknown' || /^[0-9a-f]{40}$/.test(BUILD_GIT_SHA));
  assert.ok(['revision_match_unverified', 'mismatch', 'dirty_build', 'unknown', 'not_applicable'].includes(info.runtime_revision.trust_state));
  assert.equal(info.runtime_revision.source_workspace, false);
  assert.equal(info.runtime_revision.source_clean, BUILD_SOURCE_CLEAN);
  assert.ok(Array.isArray(BUILD_TOOL_EVOLUTION_PROPOSAL_IDS));
  assert.ok(BUILD_TOOL_EVOLUTION_PROPOSAL_IDS.length <= 16);
  assert.ok(BUILD_TOOL_EVOLUTION_PROPOSAL_IDS.every(value => /^[0-9a-f]{64}$/.test(value)));
  if (BUILD_SOURCE_CLEAN !== true) assert.deepEqual(BUILD_TOOL_EVOLUTION_PROPOSAL_IDS, []);
  assert.equal(info.runtime_revision.workspace_clean_verified, false);
  assert.equal(info.runtime_revision.workspace_clean_verification_tool, 'git_status');
  assert.ok(info.runtime_revision.trusted === null || info.runtime_revision.trusted === false);
  assert.ok(Object.hasOwn(info.runtime_revision, 'matches_workspace'));
  assert.ok(Object.hasOwn(info.runtime_revision, 'workspace_git_head'));
  assert.ok(Object.hasOwn(info.runtime_revision, 'runtime_predates_workspace_head'));
  assert.equal(info.limits.commandTimeoutAbsoluteMaxMs, 60 * 60_000);
  assert.ok(Number(info.phase_durations_ms.dispatch_ms) >= 0);
  assert.ok(Number(info.phase_durations_ms.serialization_ms) >= 0);
});

test('fastWorkspaceGitHead reads repo-local refs and rejects external pointers without spawning git', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-git-head-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const gitDir = path.join(root, '.git');
  const refDir = path.join(gitDir, 'refs', 'heads');
  await mkdir(refDir, { recursive: true });
  await writeFile(path.join(gitDir, 'HEAD'), 'ref: refs/heads/main\n');
  await writeFile(path.join(refDir, 'main'), '0123456789abcdef0123456789abcdef01234567\n');
  assert.deepEqual(await fastWorkspaceGitIdentity(root), {
    branch: 'main',
    head: '0123456789abcdef0123456789abcdef01234567'
  });
  assert.equal(await fastWorkspaceGitHead(root), '0123456789abcdef0123456789abcdef01234567');
  const deep = path.join(root, 'src', 'deep');
  await mkdir(deep, { recursive: true });
  assert.equal(await fastContainingGitRoot(root, deep), root);

  const nested = path.join(root, 'packages', 'nested');
  const nestedGit = path.join(nested, '.git');
  const nestedDeep = path.join(nested, 'src', 'deep');
  await mkdir(nestedDeep, { recursive: true });
  await mkdir(nestedGit, { recursive: true });
  await writeFile(path.join(nestedGit, 'HEAD'), '89abcdef0123456789abcdef0123456789abcdef\n');
  assert.equal(await fastContainingGitRoot(root, nestedDeep), nested);

  const repository = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-git-worktree-'));
  t.after(() => rm(repository, { recursive: true, force: true }));
  const common = path.join(repository, '.git');
  const linked = path.join(repository, '.worktrees', 'linked');
  const worktreeGit = path.join(common, 'worktrees', 'linked');
  await mkdir(linked, { recursive: true });
  await mkdir(path.join(common, 'refs', 'heads'), { recursive: true });
  await mkdir(worktreeGit, { recursive: true });
  await writeFile(path.join(linked, '.git'), `gitdir: ${worktreeGit}\n`);
  await writeFile(path.join(worktreeGit, 'HEAD'), 'ref: refs/heads/linked\n');
  await writeFile(path.join(worktreeGit, 'commondir'), '../..\n');
  await writeFile(path.join(common, 'refs', 'heads', 'linked'), 'fedcba9876543210fedcba9876543210fedcba98\n');
  assert.deepEqual(await fastWorkspaceGitIdentity(linked), {
    branch: 'linked',
    head: 'fedcba9876543210fedcba9876543210fedcba98'
  });
  assert.equal(await fastWorkspaceGitHead(linked), 'fedcba9876543210fedcba9876543210fedcba98');
  const linkedDeep = path.join(linked, 'src', 'deep');
  await mkdir(linkedDeep, { recursive: true });
  assert.equal(await fastContainingGitRoot(repository, linkedDeep), linked);

  const malicious = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-git-malicious-'));
  const external = await mkdtemp(path.join(tmpdir(), 'ctmcp-node-git-external-'));
  t.after(() => rm(malicious, { recursive: true, force: true }));
  t.after(() => rm(external, { recursive: true, force: true }));
  await writeFile(path.join(external, 'HEAD'), '0123456789abcdef0123456789abcdef01234567\n');
  await writeFile(path.join(malicious, '.git'), `gitdir: ${external}\n`);
  assert.equal(await fastWorkspaceGitHead(malicious), null);
  assert.equal(await fastContainingGitRoot(malicious, malicious), null);
});

test('workspace selection gates access and read_file returns a bounded slice', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'hello.txt'), 'one\ntwo\nthree\n');
  const denied = await callTool(ctx, 'read_file', { path: 'hello.txt' }, meta);
  assert.equal(denied.ok, false);
  assert.equal(denied.error.code, 'WORKSPACE_FOLDER_NOT_SELECTED');
  await select(ctx, meta);
  const read = await callTool(ctx, 'read_file', { path: 'hello.txt', start_line: 2, end_line: 2 }, meta);
  assert.equal(read.content, 'two\n');
  assert.match(read.sha256, /^[0-9a-f]{64}$/);
});

test('read_many, search_text, list_files and project_map follow the Rust read contracts', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'hello.txt'), 'alpha\nbeta needle\ngamma needle\ndelta\n');
  await writeFile(path.join(root, 'package.json'), JSON.stringify({ scripts: { test: 'node --test', build: 'tsc' } }));
  await import('node:fs/promises').then(({ mkdir }) => mkdir(path.join(root, 'nested')));
  await writeFile(path.join(root, 'nested', 'test.txt'), 'nested needle\n');
  await select(ctx, meta);

  const batch = await callTool(ctx, 'read_many', {
    matches: [
      { path: 'hello.txt', line: 2 },
      { path: 'hello.txt', line: 3 }
    ],
    context_lines: 1,
    line_numbers: true
  }, meta);
  assert.equal(batch.requested_count, 2);
  assert.equal(batch.result_count, 1);
  assert.equal(batch.merged_count, 1);
  assert.deepEqual(batch.results[0].source_indexes, [0, 1]);
  assert.equal(batch.content_mode, 'numbered');
  assert.equal(batch.results[0].content_mode, 'numbered');
  assert.equal(batch.results[0].content, undefined);
  assert.equal(batch.results[0].content_omitted, true);
  assert.match(batch.results[0].numbered_content, /^\s+1 \| alpha/m);

  const bothContent = await callTool(ctx, 'read_many', {
    items: [{ path: 'hello.txt', start_line: 1, end_line: 2 }],
    content_mode: 'both'
  }, meta);
  assert.equal(bothContent.content_mode, 'both');
  assert.equal(bothContent.results[0].content, 'alpha\nbeta needle\n');
  assert.match(bothContent.results[0].numbered_content, /^\s+1 \| alpha/m);

  const largeLines = Array.from({ length: 500 }, (_, index) => `${String(index + 1).padStart(4, '0')}-${'x'.repeat(80)}`);
  await writeFile(path.join(root, 'large-read.txt'), `${largeLines.join('\n')}\n`);
  const numberedOnlyLarge = await callTool(ctx, 'read_many', {
    items: [{ path: 'large-read.txt', start_line: 1, end_line: 500 }],
    line_numbers: true,
    max_total_bytes: 128 * 1024
  }, meta);
  const bothLarge = await callTool(ctx, 'read_many', {
    items: [{ path: 'large-read.txt', start_line: 1, end_line: 500 }],
    content_mode: 'both',
    max_total_bytes: 128 * 1024
  }, meta);
  const numberedResponseBytes = Buffer.byteLength(JSON.stringify(numberedOnlyLarge));
  const bothResponseBytes = Buffer.byteLength(JSON.stringify(bothLarge));
  assert.ok(numberedResponseBytes < bothResponseBytes * 0.7, `${numberedResponseBytes} should be < 70% of ${bothResponseBytes}`);

  const firstPage = await callTool(ctx, 'search_text', {
    query: 'needle',
    path: '.',
    include_globs: ['hello.txt'],
    max_results: 1,
    context_lines: 1
  }, meta);
  assert.equal(firstPage.returned_count, 1);
  assert.equal(firstPage.truncated, true);
  assert.equal(firstPage.next_cursor, 1);
  assert.equal(firstPage.matches[0].line, 2);
  assert.deepEqual(firstPage.matches[0].before, ['alpha']);
  assert.equal(firstPage.calculate_total, false);
  assert.equal(firstPage.scan_completed, false);
  assert.equal(firstPage.total_matches_exact, false);
  assert.equal(firstPage.early_stop_reason, 'result_limit');

  const secondPage = await callTool(ctx, 'search_text', {
    query: 'needle',
    path: '.',
    include_globs: ['hello.txt'],
    max_results: 10,
    cursor: firstPage.next_cursor
  }, meta);
  assert.equal(secondPage.matches[0].line, 3);
  assert.equal(secondPage.next_cursor, null);

  const exactTotal = await callTool(ctx, 'search_text', {
    query: 'needle',
    path: '.',
    max_results: 1,
    calculate_total: true
  }, meta);
  assert.equal(exactTotal.returned_count, 1);
  assert.equal(exactTotal.truncated, true);
  assert.equal(exactTotal.calculate_total, true);
  assert.equal(exactTotal.scan_completed, true);
  assert.equal(exactTotal.total_matches_exact, true);
  assert.equal(exactTotal.total_matches, 3);
  assert.ok(exactTotal.files_considered >= exactTotal.scanned_files);
  assert.equal(exactTotal.early_stop_reason, null);
  assert.equal(exactTotal.exact_total_forced_full_scan, true);
  assert.match(exactTotal.search_recommendation, /full scan/);
  assert.ok(exactTotal.warnings.some(warning => warning.includes('calculate_total')));

  const searchPayloadLines = Array.from({ length: 300 }, (_, index) => `needle ${String(index).padStart(3, '0')} ${'x'.repeat(2000)}`);
  await writeFile(path.join(root, 'large-search.txt'), `${searchPayloadLines.join('\n')}\n`);
  const boundedPayload = await callTool(ctx, 'search_text', {
    query: 'needle',
    path: 'large-search.txt',
    max_results: 1000,
    max_preview_bytes: 4096
  }, meta);
  assert.equal(boundedPayload.response_limit_reached, true);
  assert.equal(boundedPayload.truncated, true);
  assert.equal(boundedPayload.early_stop_reason, 'response_limit');
  assert.equal(boundedPayload.scan_completed, false);
  assert.equal(boundedPayload.total_matches_exact, false);
  assert.ok(boundedPayload.returned_count > 0 && boundedPayload.returned_count < 300);
  assert.ok(boundedPayload.response_bytes_estimate <= boundedPayload.response_bytes_limit);
  assert.equal(boundedPayload.next_cursor, boundedPayload.returned_count);
  assert.ok(boundedPayload.warnings.some(warning => warning.includes('response payload')));

  const boundedPayloadNext = await callTool(ctx, 'search_text', {
    query: 'needle',
    path: 'large-search.txt',
    max_results: 1000,
    max_preview_bytes: 4096,
    cursor: boundedPayload.next_cursor
  }, meta);
  assert.equal(boundedPayloadNext.matches[0].line, boundedPayload.returned_count + 1);

  const exactPayload = await callTool(ctx, 'search_text', {
    query: 'needle',
    path: 'large-search.txt',
    max_results: 1000,
    max_preview_bytes: 4096,
    calculate_total: true
  }, meta);
  assert.equal(exactPayload.response_limit_reached, true);
  assert.equal(exactPayload.truncated, true);
  assert.equal(exactPayload.scan_completed, true);
  assert.equal(exactPayload.total_matches_exact, true);
  assert.equal(exactPayload.total_matches, 300);
  assert.equal(exactPayload.early_stop_reason, null);
  assert.equal(exactPayload.exact_total_forced_full_scan, true);
  assert.ok(exactPayload.returned_count < 300);
  assert.ok(exactPayload.response_bytes_estimate <= exactPayload.response_bytes_limit);

  const filenameOnly = await callTool(ctx, 'search_text', {
    filename_query: 'nested',
    files_only: true
  }, meta);
  assert.deepEqual(filenameOnly.files.map(item => item.path), ['nested/test.txt']);

  const counted = await callTool(ctx, 'search_text', {
    query: 'needle',
    count_only: true,
    include_globs: ['hello.txt', 'nested/test.txt']
  }, meta);
  assert.equal(counted.total_matches, 3);
  assert.equal(counted.calculate_total, true);
  assert.equal(counted.total_matches_exact, true);
  assert.deepEqual(counted.matches, []);

  const listed = await callTool(ctx, 'list_files', { path: '.', recursive: false }, meta);
  assert.ok(listed.entries.every(entry => entry.type !== 'directory'));
  assert.ok(listed.entries.some(entry => entry.path === 'hello.txt' && typeof entry.size_bytes === 'number'));
  const directories = await callTool(ctx, 'list_files', { path: '.', recursive: false, entry_types: ['directory'] }, meta);
  assert.deepEqual(directories.entries.map(entry => entry.path), ['nested']);

  const project = await callTool(ctx, 'project_map', { max_depth: 3 }, meta);
  assert.ok(project.manifests.some(item => item.path === 'package.json' && item.kind === 'npm'));
  assert.equal(project.package_scripts.test, 'node --test');
  assert.ok(project.suggested_commands.some(item => item.command === 'npm run test'));
});

test('validated search canary preserves exact totals while skipping tail match-detail work', async () => {
  const { root, ctx } = await context();
  const runtime = ctx.folderRuntimes.get('repo');
  assert.ok(runtime);
  const lines = Array.from({ length: 400 }, (_, index) => `needle ${String(index).padStart(3, '0')} ${'x'.repeat(200)}`);
  await writeFile(path.join(root, 'canary-search.txt'), `${lines.join('\n')}\n`);
  const strategy = await runtime.knowledgeStore.upsert({
    id: '',
    schemaVersion: 1,
    scope: 'runtime',
    target: 'tool_strategy',
    status: 'validated',
    trigger: { tool: 'search_text', calculate_total: true },
    hypothesis: 'Exact total search should preserve semantics while reducing tail detail work.',
    recommendedAction: { recommendation: 'prefer_bounded_search', calculate_total: false },
    evidence: {
      observations: 20,
      successes: 20,
      failures: 0,
      totalDurationMs: 20_000,
      totalRequestBytes: 2_000,
      totalResponseBytes: 4_000,
      firstSeenAtMs: 1,
      lastSeenAtMs: 20
    },
    confidence: 0.8,
    sourceEventIds: Array.from({ length: 20 }, (_, index) => `seed-${index}`),
    counterexampleEventIds: [],
    createdAtMs: 1,
    updatedAtMs: 20
  });
  await runtime.canaryStrategyEngine.refresh();
  const args = {
    query: 'needle',
    path: 'canary-search.txt',
    max_results: 1,
    calculate_total: true
  };
  const findMeta = async applied => {
    for (let index = 0; index < 500; index += 1) {
      const meta = { 'openai/session': `search-canary-${applied ? 'on' : 'off'}-${index}` };
      const key = ctx.conversations.identity(meta).key;
      const decision = await runtime.canaryStrategyEngine.decide('search_text', args, key);
      if (decision?.applied === applied) return meta;
    }
    throw new Error(`Unable to find deterministic canary cohort applied=${applied}`);
  };
  const controlMeta = await findMeta(false);
  const canaryMeta = await findMeta(true);
  await select(ctx, controlMeta);
  await select(ctx, canaryMeta);

  const control = await callTool(ctx, 'search_text', args, controlMeta);
  const canary = await callTool(ctx, 'search_text', args, canaryMeta);
  assert.equal(control.total_matches, 400);
  assert.equal(canary.total_matches, control.total_matches);
  assert.equal(canary.total_matches_exact, control.total_matches_exact);
  assert.equal(canary.returned_count, control.returned_count);
  assert.equal(canary.next_cursor, control.next_cursor);
  assert.deepEqual(canary.matches, control.matches);
  assert.equal(control.exact_total_forced_full_scan, true);
  assert.equal(canary.exact_total_forced_full_scan, true);
  assert.equal(control.exact_total_fast_tail, false);
  assert.equal(canary.exact_total_fast_tail, true);
  assert.ok(canary.exact_total_fast_tail_skipped_match_details >= 399);
  assert.equal('knowledge_canary_id' in control, false, 'knowledge metadata stays out of normal tool responses');
  assert.equal('knowledge_canary_id' in canary, false, 'knowledge metadata stays out of normal tool responses');

  await runtime.knowledgeIngestor.flush();
  const [impact] = await runtime.canaryImpactStore.list();
  assert.equal(impact.knowledgeId, strategy.id);
  assert.equal(impact.controlCalls, 1);
  assert.equal(impact.interventionCalls, 1);
});

test('format_files plans safely and applies guarded builtin JSON formatting', async () => {
  const { root, ctx, meta } = await context();
  const file = path.join(root, 'config.json');
  await writeFile(file, '{"b":2,"a":1}');
  await select(ctx, meta);

  const planned = await callTool(ctx, 'format_files', { paths: ['config.json'] }, meta);
  assert.equal(planned.status, 'planned');
  assert.equal(planned.mode, 'plan');
  assert.equal(planned.groups[0].adapter_id, 'builtin-json');
  assert.equal(planned.applied, false);

  const checked = await callTool(ctx, 'format_files', {
    paths: ['config.json'],
    mode: 'check'
  }, meta);
  assert.deepEqual(checked.files_changed, ['config.json']);
  assert.equal(await readFile(file, 'utf8'), '{"b":2,"a":1}');

  const before = await readFile(file);
  const expected = createHash('sha256').update(before).digest('hex');
  const applied = await callTool(ctx, 'format_files', {
    paths: ['config.json'],
    mode: 'apply',
    expected_sha256: { 'config.json': expected }
  }, meta);
  assert.equal(applied.status, 'applied');
  assert.equal(applied.applied, true);
  assert.equal(await readFile(file, 'utf8'), '{\n  "b": 2,\n  "a": 1\n}\n');

  await writeFile(file, '{"stale":true}');
  const conflict = await callTool(ctx, 'format_files', {
    paths: ['config.json'],
    mode: 'apply',
    expected_sha256: { 'config.json': expected }
  }, meta);
  assert.equal(conflict.ok, false);
  assert.equal(conflict.error.code, 'FILE_VERSION_MISMATCH');

  const projectApply = await callTool(ctx, 'format_files', {
    scope: 'project',
    mode: 'apply'
  }, meta);
  assert.equal(projectApply.ok, false);
  assert.equal(projectApply.error.code, 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
});

test('format_files derives Cargo editions and keeps rustfmt groups edition-safe', async () => {
  const { root, ctx, meta } = await context();
  await mkdir(path.join(root, 'crate-2021', 'src'), { recursive: true });
  await mkdir(path.join(root, 'crate-2024', 'src'), { recursive: true });
  await mkdir(path.join(root, 'member', 'src'), { recursive: true });
  await writeFile(path.join(root, 'Cargo.toml'), '[workspace]\nmembers = ["member"]\n[workspace.package]\nedition = "2024"\n');
  await writeFile(path.join(root, 'crate-2021', 'Cargo.toml'), '[package]\nname = "a"\nversion = "0.1.0"\nedition = "2021"\n');
  await writeFile(path.join(root, 'crate-2024', 'Cargo.toml'), '[package]\nname = "b"\nversion = "0.1.0"\nedition = "2024"\n');
  await writeFile(path.join(root, 'member', 'Cargo.toml'), '[package]\nname = "member"\nversion = "0.1.0"\nedition.workspace = true\n');
  await writeFile(path.join(root, 'crate-2021', 'src', 'lib.rs'), 'pub async fn a() {}\n');
  await writeFile(path.join(root, 'crate-2024', 'src', 'lib.rs'), 'pub async fn b() {}\n');
  await writeFile(path.join(root, 'member', 'src', 'lib.rs'), 'pub async fn inherited() {}\n');
  await select(ctx, meta);

  const planned = await callTool(ctx, 'format_files', {
    paths: ['crate-2021/src/lib.rs', 'crate-2024/src/lib.rs', 'member/src/lib.rs'],
    mode: 'plan',
    formatter: 'rustfmt'
  }, meta);
  assert.equal(planned.ok, true, JSON.stringify(planned));
  assert.deepEqual(planned.groups.map(group => group.rust_edition).sort(), ['2021', '2024', '2024']);
  assert.deepEqual(planned.selection.map(file => file.rust_edition).sort(), ['2021', '2024', '2024']);
  assert.deepEqual(rustfmtCommandArgs(['src/lib.rs'], '2024'), ['--edition', '2024', 'src/lib.rs']);

  assert.deepEqual(cargoRustEditionMetadata('[package]\nedition = "2024"\n'), {
    package_present: true,
    package_edition: '2024',
    package_inherits_workspace: false
  });
  assert.deepEqual(cargoRustEditionMetadata('[package]\nedition.workspace = true\n[workspace.package]\nedition = "2021"\n'), {
    package_present: true,
    package_inherits_workspace: true,
    workspace_edition: '2021'
  });
});

test('format_files executes rustfmt with the Cargo edition when rustfmt is available', async t => {
  try {
    await execFile('rustfmt', ['--version']);
  } catch {
    t.skip('rustfmt is unavailable');
    return;
  }
  const { root, ctx, meta } = await context();
  await mkdir(path.join(root, 'src'), { recursive: true });
  await writeFile(path.join(root, 'Cargo.toml'), '[package]\nname = "edition-smoke"\nversion = "0.1.0"\nedition = "2024"\n');
  await writeFile(path.join(root, 'src', 'lib.rs'), 'pub async fn smoke(){println!("ok");}\n');
  await select(ctx, meta);

  const checked = await callTool(ctx, 'format_files', {
    paths: ['src/lib.rs'],
    mode: 'check',
    formatter: 'rustfmt',
    strict: true
  }, meta);
  assert.equal(checked.ok, true, JSON.stringify(checked));
  assert.equal(checked.groups[0].rust_edition, '2024');
  assert.deepEqual(checked.unavailable_adapters, []);
});

test('custom formatter uses workspace configuration and executes only after confirmation', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'page.tmpl'), 'hello\n');
  await installCustomFormatter(root, `
const fs = require('node:fs');
for (const file of process.argv.slice(2)) {
  const value = fs.readFileSync(file, 'utf8');
  fs.writeFileSync(file, value.replaceAll('hello', 'HELLO'));
}
`);
  await select(ctx, meta);

  const planned = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'plan',
    formatter: 'company-template'
  }, meta);
  assert.equal(planned.ok, true);
  assert.equal(planned.custom_formatter_group_count, 1);
  assert.equal(planned.groups[0].adapter_id, 'company-template');
  assert.equal(planned.groups[0].custom, true);
  assert.equal(planned.selection[0].selection_source, 'workspace_config');

  const denied = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'check',
    formatter: 'company-template'
  }, meta);
  assert.equal(denied.ok, false);
  assert.equal(denied.error.code, 'CUSTOM_FORMATTER_REQUIRES_CONFIRMATION');

  const checked = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'check',
    formatter: 'company-template',
    confirm: true
  }, meta);
  assert.equal(checked.ok, true);
  assert.deepEqual(checked.files_changed, ['page.tmpl']);
  assert.equal(checked.applied, false);
  assert.equal(await readFile(path.join(root, 'page.tmpl'), 'utf8'), 'hello\n');

  const applied = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'apply',
    formatter: 'company-template',
    confirm: true
  }, meta);
  assert.equal(applied.ok, true);
  assert.equal(applied.applied, true);
  assert.equal(await readFile(path.join(root, 'page.tmpl'), 'utf8'), 'HELLO\n');
  assert.equal(await pathExists(path.join(root, '.coding-tools-format')), false);
});

test('formatter preserves a pre-existing mirror root directory', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'page.tmpl'), 'hello\n');
  await installCustomFormatter(root, `
const fs = require('node:fs');
for (const file of process.argv.slice(2)) fs.writeFileSync(file, 'HELLO\\n');
`);
  const mirrorParent = path.join(root, '.coding-tools-format');
  await mkdir(mirrorParent);
  await select(ctx, meta);
  const result = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'check',
    formatter: 'company-template',
    confirm: true
  }, meta);
  assert.equal(result.ok, true);
  assert.equal(result.mirror_location, 'workspace');
  assert.equal(await pathExists(mirrorParent), true);
  assert.deepEqual(await import('node:fs/promises').then(fs => fs.readdir(mirrorParent)), []);
});

test('formatter falls back to system temp when the workspace mirror is not writable', { skip: process.platform === 'win32' }, async t => {
  const { root, ctx, meta } = await context();
  const file = path.join(root, 'config.json');
  await writeFile(file, '{"b":2,"a":1}');
  const mirrorParent = path.join(root, '.coding-tools-format');
  await mkdir(mirrorParent);
  await chmod(mirrorParent, 0o555);
  t.after(async () => chmod(mirrorParent, 0o755).catch(() => undefined));
  await select(ctx, meta);

  const result = await callTool(ctx, 'format_files', {
    paths: ['config.json'],
    mode: 'check'
  }, meta);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.mirror_location, 'system_temp');
  assert.deepEqual(result.files_changed, ['config.json']);
  assert.ok(result.warnings.some(warning => warning.includes('system temporary directory')));
  assert.equal(await readFile(file, 'utf8'), '{"b":2,"a":1}');
});

test('custom formatter paths must stay inside the workspace', async () => {
  const { root, ctx, meta } = await context();
  await mkdir(path.join(root, '.coding-tools'), { recursive: true });
  await writeFile(path.join(root, '.coding-tools', 'formatters.json'), JSON.stringify({
    formatters: {
      unsafe: { program: '../formatter.cjs', extensions: ['tmpl'], args: ['{files}'] }
    }
  }));
  await writeFile(path.join(root, 'page.tmpl'), 'hello\n');
  await select(ctx, meta);
  const result = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'plan',
    formatter: 'unsafe'
  }, meta);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'FORMATTER_CONFIG_INVALID');
});

test('unexpected formatter changes abort without touching the workspace', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'page.tmpl'), 'hello\n');
  await installCustomFormatter(root, `
const fs = require('node:fs');
for (const file of process.argv.slice(2)) {
  fs.writeFileSync(file, 'FORMATTED\\n');
}
fs.writeFileSync('unexpected.txt', 'surprise\\n');
`);
  await select(ctx, meta);
  const result = await callTool(ctx, 'format_files', {
    paths: ['page.tmpl'],
    mode: 'apply',
    formatter: 'company-template',
    confirm: true
  }, meta);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'FORMAT_UNEXPECTED_CHANGES');
  assert.deepEqual(result.error.details.unexpected_changes, ['unexpected.txt']);
  assert.equal(await readFile(path.join(root, 'page.tmpl'), 'utf8'), 'hello\n');
  assert.equal(await pathExists(path.join(root, 'unexpected.txt')), false);
  assert.equal(await pathExists(path.join(root, '.coding-tools-format')), false);
});

test('file_ops performs transactional create, copy, move, delete and mkdir operations', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'source.txt'), 'source\n');
  await writeFile(path.join(root, 'move.txt'), 'move\n');
  await writeFile(path.join(root, 'delete.txt'), 'delete\n');
  await select(ctx, meta);

  const sourceHash = createHash('sha256').update('source\n').digest('hex');
  const dryRun = await callTool(ctx, 'file_ops', {
    dry_run: true,
    operations: [{ type: 'create', path: 'dry-run.txt', content: 'planned\n' }]
  }, meta);
  assert.equal(dryRun.ok, true);
  assert.equal(dryRun.preflight, true);
  assert.equal(dryRun.atomic, true);
  assert.equal(dryRun.applied, false);
  assert.equal(await pathExists(path.join(root, 'dry-run.txt')), false);

  const result = await callTool(ctx, 'file_ops', {
    operations: [
      { type: 'create', path: 'created.txt', content: 'created\n' },
      { type: 'copy', path: 'source.txt', destination: 'nested/copied.txt', expected_sha256: sourceHash },
      { type: 'move', path: 'move.txt', destination: 'moved.txt' },
      { type: 'delete', path: 'delete.txt' },
      { type: 'mkdir', path: 'empty/directory' }
    ]
  }, meta);
  assert.equal(result.ok, true);
  assert.equal(result.atomic, true);
  assert.equal(result.applied, true);
  assert.equal(typeof result.change_id, 'string');
  assert.equal(await readFile(path.join(root, 'created.txt'), 'utf8'), 'created\n');
  assert.equal(await readFile(path.join(root, 'nested', 'copied.txt'), 'utf8'), 'source\n');
  assert.equal(await readFile(path.join(root, 'moved.txt'), 'utf8'), 'move\n');
  assert.equal(await pathExists(path.join(root, 'move.txt')), false);
  assert.equal(await pathExists(path.join(root, 'delete.txt')), false);
  assert.equal(await pathExists(path.join(root, 'empty', 'directory')), true);
});

test('file_ops rolls back staged files when a later directory operation fails', async () => {
  const { root, ctx, meta } = await context();
  await select(ctx, meta);
  const result = await callTool(ctx, 'file_ops', {
    operations: [
      { type: 'create', path: 'nested/collision', content: 'temporary\n' },
      { type: 'mkdir', path: 'nested/collision' }
    ]
  }, meta);
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'FILE_OPS_APPLY_FAILED');
  assert.deepEqual(result.error.details.rollback_failures, []);
  assert.equal(await pathExists(path.join(root, 'nested', 'collision')), false);
  assert.equal(await pathExists(path.join(root, 'nested')), false);
});

test('file_ops deletes directories and requires confirm=true for non-empty trees', async () => {
  const { root, ctx, meta } = await context();
  await mkdir(path.join(root, 'empty'), { recursive: true });
  await mkdir(path.join(root, 'tree', 'nested'), { recursive: true });
  await writeFile(path.join(root, 'tree', 'a.txt'), 'a');
  await writeFile(path.join(root, 'tree', 'nested', 'b.txt'), 'b');
  await mkdir(path.join(root, 'vendor', 'lib', '.git'), { recursive: true });
  await select(ctx, meta);

  const empty = await callTool(ctx, 'file_ops', { operations: [{ type: 'delete', path: 'empty' }] }, meta);
  assert.equal(empty.ok, true);
  assert.deepEqual(empty.directories_deleted, ['empty']);
  assert.equal(await pathExists(path.join(root, 'empty')), false);

  const refused = await callTool(ctx, 'file_ops', { operations: [{ type: 'delete', path: 'tree' }] }, meta);
  assert.equal(refused.ok, false);
  assert.equal(refused.error.code, 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
  assert.equal(await pathExists(path.join(root, 'tree', 'nested', 'b.txt')), true);

  const overlap = await callTool(ctx, 'file_ops', {
    confirm: true,
    operations: [
      { type: 'delete', path: 'tree' },
      { type: 'create', path: 'tree/new.txt', content: 'x' }
    ]
  }, meta);
  assert.equal(overlap.ok, false);
  assert.equal(overlap.error.code, 'INVALID_ARGUMENT');

  const protectedTree = await callTool(ctx, 'file_ops', { confirm: true, operations: [{ type: 'delete', path: 'vendor' }] }, meta);
  assert.equal(protectedTree.ok, false);
  assert.equal(protectedTree.error.code, 'PROTECTED_PATH');
  assert.equal(await pathExists(path.join(root, 'vendor', 'lib', '.git')), true);

  const deleted = await callTool(ctx, 'file_ops', { confirm: true, operations: [{ type: 'delete', path: 'tree' }] }, meta);
  assert.equal(deleted.ok, true);
  assert.equal(deleted.affected_files[0].operation, 'delete_directory');
  assert.equal(deleted.affected_files[0].entries, 3);
  assert.deepEqual(deleted.directories_deleted, ['tree']);
  assert.equal(await pathExists(path.join(root, 'tree')), false);
});

test('file_ops enforces hashes, overwrite confirmation and protected paths', async () => {
  const { root, ctx, meta } = await context();
  await writeFile(path.join(root, 'package.json'), '{}\n');
  await writeFile(path.join(root, 'existing.txt'), 'before\n');
  await select(ctx, meta);

  const stale = await callTool(ctx, 'file_ops', {
    operations: [{
      type: 'delete',
      path: 'existing.txt',
      expected_sha256: '0'.repeat(64)
    }]
  }, meta);
  assert.equal(stale.ok, false);
  assert.equal(stale.error.code, 'FILE_VERSION_MISMATCH');
  assert.equal(await readFile(path.join(root, 'existing.txt'), 'utf8'), 'before\n');

  const overwrite = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: 'existing.txt', content: 'after\n', overwrite: true }]
  }, meta);
  assert.equal(overwrite.ok, false);
  assert.equal(overwrite.error.code, 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');

  const critical = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'delete', path: 'package.json' }]
  }, meta);
  assert.equal(critical.ok, false);
  assert.equal(critical.error.code, 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');

  const protectedResult = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: '.git/config', content: 'blocked' }]
  }, meta);
  assert.equal(protectedResult.ok, false);
  assert.equal(protectedResult.error.code, 'PROTECTED_PATH');

  const normalizedProtected = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: 'safe/../.git/config', content: 'blocked' }]
  }, meta);
  assert.equal(normalizedProtected.ok, false);
  assert.equal(normalizedProtected.error.code, 'PATH_OUTSIDE_WORKSPACE');

  const protectedGithub = await callTool(ctx, 'file_ops', {
    operations: [{ type: 'create', path: '.github/workflows/blocked.yml', content: 'blocked' }]
  }, meta);
  assert.equal(protectedGithub.ok, false);
  assert.equal(protectedGithub.error.code, 'PROTECTED_PATH');
});

test('Git branch and stage support dry-run, expected HEAD and structured status', async () => {
  const { root, ctx, meta } = await gitContext();
  const head = await git(root, 'rev-parse', 'HEAD');
  await writeFile(path.join(root, 'tracked.txt'), 'modified\n');

  const dryStage = await callTool(ctx, 'git_stage', {
    paths: ['tracked.txt'],
    expected_head: head,
    dry_run: true
  }, meta);
  assert.equal(dryStage.ok, true);
  assert.equal(dryStage.applied, false);
  assert.deepEqual(dryStage.command, ['git', 'add', '--', 'tracked.txt']);
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), '');

  const stale = await callTool(ctx, 'git_stage', {
    paths: ['tracked.txt'],
    expected_head: '0'.repeat(40)
  }, meta);
  assert.equal(stale.ok, false);
  assert.equal(stale.error.code, 'EXPECTED_HEAD_MISMATCH');

  const dryBranch = await callTool(ctx, 'git_branch', {
    action: 'create',
    name: 'feature/dry-run',
    switch: false,
    dry_run: true,
    expected_head: head
  }, meta);
  assert.equal(dryBranch.ok, true);
  assert.equal(dryBranch.applied, false);
  assert.equal(await git(root, 'branch', '--list', 'feature/dry-run'), '');

  const current = await git(root, 'branch', '--show-current');
  const created = await callTool(ctx, 'git_branch', {
    action: 'create',
    name: 'feature/no-switch',
    switch: false,
    expected_head: head
  }, meta);
  assert.equal(created.ok, true);
  assert.equal(created.applied, true);
  assert.equal(await git(root, 'branch', '--show-current'), current);
  assert.match(await git(root, 'branch', '--list', 'feature/no-switch'), /feature\/no-switch/);

  const denied = await callTool(ctx, 'git_branch', {
    action: 'delete',
    name: 'feature/no-switch',
    expected_head: head
  }, meta);
  assert.equal(denied.ok, false);
  assert.equal(denied.error.code, 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');
  const deleted = await callTool(ctx, 'git_branch', {
    action: 'delete',
    name: 'feature/no-switch',
    confirm: true,
    expected_head: head
  }, meta);
  assert.equal(deleted.ok, true);
  assert.equal(await git(root, 'branch', '--list', 'feature/no-switch'), '');
});

test('git_worktree manages linked worktree lifecycle and resets removed cwd', async () => {
  const { root, ctx, meta } = await gitContext();
  const worktreePath = '.worktrees/managed';
  const branch = 'feature/managed-worktree';

  const listed = await callTool(ctx, 'git_worktree', { action: 'list' }, meta);
  assert.equal(listed.ok, true);
  assert.equal(listed.worktrees.length, 1);

  const created = await callTool(ctx, 'git_worktree', {
    action: 'create',
    path: worktreePath,
    branch,
    start_point: 'HEAD'
  }, meta);
  assert.equal(created.ok, true, JSON.stringify(created));
  assert.equal(created.applied, true);
  assert.equal(await pathExists(path.join(root, worktreePath)), true);
  assert.ok(created.worktrees.some(entry => entry.path === worktreePath && entry.branch === branch));

  const selectedCwd = await callTool(ctx, 'set_default_cwd', { path: worktreePath }, meta);
  assert.equal(selectedCwd.ok, true);
  assert.equal(selectedCwd.default_cwd.replaceAll('\\', '/'), worktreePath);

  const denied = await callTool(ctx, 'git_worktree', {
    action: 'remove',
    path: worktreePath,
    delete_branch: true
  }, meta);
  assert.equal(denied.ok, false);
  assert.equal(denied.error.code, 'DANGEROUS_OPERATION_REQUIRES_CONFIRMATION');

  const removed = await callTool(ctx, 'git_worktree', {
    action: 'remove',
    path: worktreePath,
    delete_branch: true,
    confirm: true
  }, meta);
  assert.equal(removed.ok, true, JSON.stringify(removed));
  assert.equal(removed.applied, true);
  assert.equal(await pathExists(path.join(root, worktreePath)), false);
  assert.equal(await git(root, 'branch', '--list', branch), '');

  const workspace = await callTool(ctx, 'list_workspace_folders', {}, meta);
  assert.equal(workspace.default_cwd, '.');
});

test('git_worktree reuses an existing branch and derives a managed default path', async () => {
  const { root, ctx, meta } = await gitContext();
  const branch = 'feature/existing-worktree';
  await git(root, 'branch', branch);

  const dryRun = await callTool(ctx, 'git_worktree', {
    action: 'create', branch, dry_run: true
  }, meta);
  assert.equal(dryRun.ok, true, JSON.stringify(dryRun));
  assert.equal(dryRun.applied, false);
  assert.equal(dryRun.branch_mode, 'existing');
  assert.equal(dryRun.path, '.worktrees/feature-existing-worktree');

  const created = await callTool(ctx, 'git_worktree', { action: 'create', branch }, meta);
  assert.equal(created.ok, true, JSON.stringify(created));
  assert.equal(created.applied, true);
  assert.equal(created.branch_mode, 'existing');
  assert.equal(await git(path.join(root, created.path), 'branch', '--show-current'), branch);

  const removed = await callTool(ctx, 'git_worktree', {
    action: 'remove', path: created.path, confirm: true
  }, meta);
  assert.equal(removed.ok, true, JSON.stringify(removed));
});

test('Git read tools follow the active linked worktree cwd', async () => {
  const { root, ctx, meta } = await gitContext();
  const worktreePath = '.worktrees/read-tools';
  const linked = path.join(root, worktreePath);
  const branch = 'feature/read-tools-worktree';
  const created = await callTool(ctx, 'git_worktree', {
    action: 'create', path: worktreePath, branch
  }, meta);
  assert.equal(created.ok, true, JSON.stringify(created));
  await callTool(ctx, 'set_default_cwd', { path: worktreePath }, meta);
  await writeFile(path.join(linked, 'tracked.txt'), 'linked-read-tools\n');
  await git(linked, 'add', '--', 'tracked.txt');
  await git(linked, 'commit', '-m', 'linked read tools');
  const linkedHead = await git(linked, 'rev-parse', 'HEAD');

  const log = await callTool(ctx, 'git_log', { max_count: 1 }, meta);
  assert.equal(log.ok, true, JSON.stringify(log));
  assert.equal(log.commits[0].hash, linkedHead);
  assert.equal(log.commits[0].subject, 'linked read tools');

  const shown = await callTool(ctx, 'git_show', { rev: 'HEAD', include_diff: false }, meta);
  assert.equal(shown.ok, true, JSON.stringify(shown));
  assert.match(shown.content, /linked read tools/);

  const blame = await callTool(ctx, 'git_blame', { path: 'tracked.txt', start_line: 1, end_line: 1 }, meta);
  assert.equal(blame.ok, true, JSON.stringify(blame));
  assert.equal(blame.lines[0].commit, linkedHead);
  assert.equal(blame.lines[0].content, 'linked-read-tools');

  await callTool(ctx, 'set_default_cwd', { path: '.' }, meta);
  const rootLog = await callTool(ctx, 'git_log', { max_count: 1 }, meta);
  assert.notEqual(rootLog.commits[0].hash, linkedHead);

  await callTool(ctx, 'git_worktree', {
    action: 'remove', path: worktreePath, delete_branch: true, confirm: true
  }, meta);
});

test('git_worktree remove dry-run is non-destructive and does not require confirmation', async () => {
  const { root, ctx, meta } = await gitContext();
  const worktreePath = '.worktrees/dry-remove';
  const created = await callTool(ctx, 'git_worktree', {
    action: 'create', path: worktreePath, branch: 'feature/dry-remove'
  }, meta);
  assert.equal(created.ok, true, JSON.stringify(created));

  const dryRun = await callTool(ctx, 'git_worktree', {
    action: 'remove', path: worktreePath, dry_run: true
  }, meta);
  assert.equal(dryRun.ok, true, JSON.stringify(dryRun));
  assert.equal(dryRun.applied, false);
  assert.equal(await pathExists(path.join(root, worktreePath)), true);

  await callTool(ctx, 'git_worktree', {
    action: 'remove', path: worktreePath, delete_branch: true, confirm: true
  }, meta);
});

test('git_worktree list reports workspace management and removability metadata', async () => {
  const { ctx, meta } = await gitContext();
  const worktreePath = '.worktrees/metadata';
  await callTool(ctx, 'git_worktree', {
    action: 'create', path: worktreePath, branch: 'feature/metadata-worktree'
  }, meta);

  const listed = await callTool(ctx, 'git_worktree', { action: 'list' }, meta);
  const primary = listed.worktrees.find(entry => entry.path === '.');
  const managed = listed.worktrees.find(entry => entry.path === worktreePath);
  assert.equal(primary.primary, true);
  assert.equal(primary.inside_workspace, true);
  assert.equal(primary.removable, false);
  assert.equal(managed.primary, false);
  assert.equal(managed.inside_workspace, true);
  assert.equal(managed.managed, true);
  assert.equal(managed.removable, true);
  assert.equal(typeof managed.dirty, 'boolean');
  assert.equal(typeof managed.branch_merged, 'boolean');

  await callTool(ctx, 'git_worktree', {
    action: 'remove', path: worktreePath, delete_branch: true, confirm: true
  }, meta);
});

test('Git mutators route to a selected nested repository and guard its fingerprint', async () => {
  const { root, ctx, meta } = await gitContext();
  const nested = path.join(root, 'nested-repo');
  await mkdir(nested);
  await git(nested, 'init');
  await git(nested, 'config', 'user.email', 'test@example.com');
  await git(nested, 'config', 'user.name', 'Test User');
  await writeFile(path.join(nested, 'tracked.txt'), 'before\n');
  await git(nested, 'add', 'tracked.txt');
  await git(nested, 'commit', '-m', 'initial');
  await writeFile(path.join(nested, 'tracked.txt'), 'after\n');

  const status = await callTool(ctx, 'git_status', { path: 'nested-repo' }, meta);
  assert.equal(status.ok, true);
  assert.match(status.repo_fingerprint, /^[0-9a-f]{64}$/);
  assert.equal(status.repo.repo_path, 'nested-repo');

  const stale = await callTool(ctx, 'git_stage', {
    repo_path: 'nested-repo',
    paths: ['tracked.txt'],
    expected_repo_fingerprint: '0'.repeat(64)
  }, meta);
  assert.equal(stale.ok, false);
  assert.equal(stale.error.code, 'GIT_REPO_TARGET_MISMATCH');

  const staged = await callTool(ctx, 'git_stage', {
    repo_path: 'nested-repo',
    paths: ['tracked.txt'],
    expected_repo_fingerprint: status.repo_fingerprint
  }, meta);
  assert.equal(staged.ok, true);
  assert.equal(staged.repo.repo_path, 'nested-repo');
  assert.equal(await git(nested, 'diff', '--cached', '--name-only'), 'tracked.txt');
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), '');
});

test('Git mutators keep explicit repo_path workspace-relative and canonicalize pathspecs with default cwd', async () => {
  const { root, ctx, meta } = await gitContext();
  const nested = path.join(root, 'nested-repo');
  await mkdir(nested);
  await git(nested, 'init');
  await git(nested, 'config', 'user.email', 'test@example.com');
  await git(nested, 'config', 'user.name', 'Test User');
  await writeFile(path.join(nested, 'tracked.txt'), 'before\n');
  await git(nested, 'add', 'tracked.txt');
  await git(nested, 'commit', '-m', 'initial');
  await writeFile(path.join(nested, 'tracked.txt'), 'after\n');

  const selected = await callTool(ctx, 'set_default_cwd', { path: 'nested-repo' }, meta);
  assert.equal(selected.ok, true, JSON.stringify(selected));

  const repoRelative = await callTool(ctx, 'git_stage', {
    repo_path: 'nested-repo',
    paths: ['tracked.txt'],
    dry_run: true
  }, meta);
  assert.equal(repoRelative.ok, true, JSON.stringify(repoRelative));
  assert.equal(repoRelative.repo.repo_path, 'nested-repo');
  assert.deepEqual(repoRelative.paths, ['tracked.txt']);

  const workspaceRelative = await callTool(ctx, 'git_stage', {
    repo_path: 'nested-repo',
    paths: ['nested-repo/tracked.txt'],
    dry_run: true
  }, meta);
  assert.equal(workspaceRelative.ok, true, JSON.stringify(workspaceRelative));
  assert.equal(workspaceRelative.repo.repo_path, 'nested-repo');
  assert.deepEqual(workspaceRelative.paths, ['tracked.txt']);

  const implicitRepo = await callTool(ctx, 'git_stage', {
    paths: ['tracked.txt'],
    dry_run: true
  }, meta);
  assert.equal(implicitRepo.ok, true, JSON.stringify(implicitRepo));
  assert.equal(implicitRepo.repo.repo_path, 'nested-repo');
  assert.deepEqual(implicitRepo.paths, ['tracked.txt']);

  const diffRepoRelative = await callTool(ctx, 'git_diff', {
    repo_path: 'nested-repo',
    paths: ['tracked.txt']
  }, meta);
  assert.equal(diffRepoRelative.ok, true, JSON.stringify(diffRepoRelative));
  assert.equal(diffRepoRelative.repo.repo_path, 'nested-repo');
  assert.deepEqual(diffRepoRelative.paths, ['tracked.txt']);
  assert.match(diffRepoRelative.diff, /-before/);
  assert.match(diffRepoRelative.diff, /\+after/);

  const diffWorkspaceRelative = await callTool(ctx, 'git_diff', {
    repo_path: 'nested-repo',
    paths: ['nested-repo/tracked.txt']
  }, meta);
  assert.equal(diffWorkspaceRelative.ok, true, JSON.stringify(diffWorkspaceRelative));
  assert.equal(diffWorkspaceRelative.repo.repo_path, 'nested-repo');
  assert.deepEqual(diffWorkspaceRelative.paths, ['tracked.txt']);
  assert.equal(diffWorkspaceRelative.diff, diffRepoRelative.diff);

  const implicitDiff = await callTool(ctx, 'git_diff', { paths: ['tracked.txt'] }, meta);
  assert.equal(implicitDiff.ok, true, JSON.stringify(implicitDiff));
  assert.equal(implicitDiff.repo.repo_path, 'nested-repo');
  assert.deepEqual(implicitDiff.paths, ['tracked.txt']);

  const shown = await callTool(ctx, 'git_show', {
    repo_path: 'nested-repo',
    rev: 'HEAD',
    include_diff: false
  }, meta);
  assert.equal(shown.ok, true, JSON.stringify(shown));
  assert.equal(shown.repo.repo_path, 'nested-repo');
  assert.match(shown.content, /initial/);

  const implicitShow = await callTool(ctx, 'git_show', { rev: 'HEAD', include_diff: false }, meta);
  assert.equal(implicitShow.ok, true, JSON.stringify(implicitShow));
  assert.equal(implicitShow.repo.repo_path, 'nested-repo');
  assert.equal(implicitShow.content, shown.content);
});

test('git_commit enforces a clean index and returns previous/new HEAD metadata', async () => {
  const { root, ctx, meta } = await gitContext();
  const oldHead = await git(root, 'rev-parse', 'HEAD');
  await writeFile(path.join(root, 'tracked.txt'), 'committed\n');

  const dryRun = await callTool(ctx, 'git_commit', {
    message: 'dry run',
    paths: ['tracked.txt'],
    expected_head: oldHead,
    dry_run: true
  }, meta);
  assert.equal(dryRun.ok, true);
  assert.equal(dryRun.applied, false);
  assert.equal(dryRun.index_clean, true);
  assert.equal(dryRun.index_clean_before, true);
  assert.equal(dryRun.staged_path_count_before, 0);
  assert.equal(dryRun.selected_path_count, 1);
  assert.equal(dryRun.staged_by_tool, true);
  assert.equal(dryRun.expected_head, oldHead);
  assert.equal(dryRun.actual_head_before, oldHead);
  assert.equal(dryRun.transaction_stage, 'preflight');
  assert.equal(dryRun.would_stage, true);
  assert.equal(dryRun.would_commit, true);
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), '');

  const committed = await callTool(ctx, 'git_commit', {
    message: 'commit tracked change',
    paths: ['tracked.txt'],
    expected_head: oldHead
  }, meta);
  assert.equal(committed.ok, true);
  assert.equal(committed.applied, true);
  assert.equal(committed.previous_head, oldHead);
  assert.equal(committed.expected_head, oldHead);
  assert.equal(committed.actual_head_before, oldHead);
  assert.equal(committed.actual_head_at_commit, oldHead);
  assert.equal(committed.actual_head_after, committed.commit);
  assert.equal(committed.transaction_stage, 'committed');
  assert.equal(committed.selected_path_count, 1);
  assert.equal(committed.staged_path_count_before, 0);
  assert.equal(committed.staged_path_count, 1);
  assert.equal(committed.index_clean_before, true);
  assert.equal(committed.staged_by_tool, true);
  assert.equal(committed.index_restored, false);
  assert.notEqual(committed.commit, oldHead);
  assert.equal(await git(root, 'rev-parse', 'HEAD'), committed.commit);

  await writeFile(path.join(root, 'pre-staged.txt'), 'staged\n');
  await git(root, 'add', '--', 'pre-staged.txt');
  await writeFile(path.join(root, 'tracked.txt'), 'next\n');
  const blocked = await callTool(ctx, 'git_commit', {
    message: 'must not mix staged changes',
    paths: ['tracked.txt'],
    expected_head: committed.commit
  }, meta);
  assert.equal(blocked.ok, false);
  assert.equal(blocked.error.code, 'GIT_INDEX_NOT_CLEAN');
  assert.equal(blocked.error.details.transaction_stage, 'preflight_index');
  assert.equal(blocked.error.details.selected_path_count, 1);
  assert.equal(blocked.error.details.staged_path_count_before, 1);
  assert.equal(blocked.error.details.index_clean_before, false);
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), 'pre-staged.txt');
});

test('git_commit restores a clean index after a commit hook failure', async () => {
  const { root, ctx, meta } = await gitContext();
  const head = await git(root, 'rev-parse', 'HEAD');
  await writeFile(path.join(root, 'tracked.txt'), 'hook failure\n');
  const hook = path.join(root, '.git', 'hooks', 'pre-commit');
  await writeFile(hook, '#!/bin/sh\nexit 1\n');
  await chmod(hook, 0o755);

  const failed = await callTool(ctx, 'git_commit', {
    message: 'rejected by hook',
    paths: ['tracked.txt'],
    expected_head: head
  }, meta);
  assert.equal(failed.ok, false);
  assert.equal(failed.error.code, 'GIT_COMMIT_FAILED');
  assert.equal(failed.error.details.staged_by_tool, true);
  assert.equal(failed.error.details.index_restored, true);
  assert.equal(failed.error.details.transaction_stage, 'commit');
  assert.equal(failed.error.details.actual_head_before, head);
  assert.equal(failed.error.details.actual_head_at_commit, head);
  assert.equal(failed.error.details.selected_path_count, 1);
  assert.equal(failed.error.details.staged_path_count_before, 0);
  assert.equal(failed.error.details.staged_path_count, 1);
  assert.equal(failed.error.details.index_clean_before, true);
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), '');
  assert.equal(await git(root, 'diff', '--name-only'), 'tracked.txt');
  assert.equal(await readFile(path.join(root, 'tracked.txt'), 'utf8'), 'hook failure\n');
});

test('git_push uses guarded network approval and pushes to a local bare remote without local baseline work', async t => {
  const { root, ctx, meta } = await gitContext('guarded');
  const remote = await mkdtemp(path.join(tmpdir(), 'ctmcp-push-remote-'));
  t.after(() => rm(remote, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 }));
  await git(remote, 'init', '--bare');
  await git(root, 'remote', 'add', 'origin', remote);
  const head = await git(root, 'rev-parse', 'HEAD');
  const status = await callTool(ctx, 'git_status', {}, meta);

  const pending = await callTool(ctx, 'git_push', {
    remote: 'origin',
    branch: status.branch,
    expected_head: head,
    expected_repo_fingerprint: status.repo_fingerprint,
    reason: 'local guarded push regression'
  }, meta);
  assert.equal(pending.ok, false);
  assert.equal(pending.error.code, 'PERMISSION_REQUIRED');
  assert.equal(pending.error.details.permission_request.permission, 'network');
  assert.equal(pending.error.details.permission_request.tool_name, 'git_push');

  const resumed = await callTool(ctx, 'request_permissions', {
    resume_id: pending.error.details.permission_request.resume_id,
    approve: true,
    confirm: true,
    scope: 'once'
  }, meta);
  assert.equal(resumed.ok, true, JSON.stringify(resumed));
  assert.equal(resumed.resumed, true);
  assert.equal(resumed.applied, true);
  assert.equal(await git(remote, 'rev-parse', `refs/heads/${status.branch}`), head);

  const operations = await callTool(ctx, 'operation_log', {
    order: 'desc',
    tool: 'git_push',
    limit: 10
  }, meta);
  assert.equal(operations.ok, true);
  assert.equal(operations.order, 'desc');
  assert.equal(operations.operations.length, 2);
  assert.equal(operations.operations[0].kind, 'completed');
  assert.equal(operations.operations[1].kind, 'started');
});

test('git_restore keeps staged and worktree restoration modes separate', async () => {
  const { root, ctx, meta } = await gitContext();
  const head = await git(root, 'rev-parse', 'HEAD');
  await writeFile(path.join(root, 'tracked.txt'), 'changed\n');
  await git(root, 'add', '--', 'tracked.txt');

  const dryRun = await callTool(ctx, 'git_restore', {
    paths: ['tracked.txt'],
    staged: true,
    confirm: true,
    dry_run: true,
    expected_head: head
  }, meta);
  assert.equal(dryRun.ok, true);
  assert.equal(dryRun.staged, true);
  assert.equal(dryRun.worktree, false);
  assert.equal(dryRun.rollback_protected, true);
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), 'tracked.txt');

  const unstaged = await callTool(ctx, 'git_restore', {
    paths: ['tracked.txt'],
    staged: true,
    confirm: true,
    expected_head: head
  }, meta);
  assert.equal(unstaged.ok, true);
  assert.equal(unstaged.worktree, false);
  assert.equal(unstaged.rollback_protected, true);
  assert.ok(unstaged.snapshot_bytes > 0);
  assert.equal(await git(root, 'diff', '--cached', '--name-only'), '');
  assert.equal(await readFile(path.join(root, 'tracked.txt'), 'utf8'), 'changed\n');

  const restored = await callTool(ctx, 'git_restore', {
    paths: ['tracked.txt'],
    worktree: true,
    confirm: true,
    expected_head: head
  }, meta);
  assert.equal(restored.ok, true);
  assert.equal(restored.rollback_protected, true);
  assert.ok(restored.snapshot_bytes > 0);
  assert.equal((await readFile(path.join(root, 'tracked.txt'), 'utf8')).replaceAll('\r\n', '\n'), 'initial\n');
});

test('git_restore rolls back path-scoped staged and worktree state after Git rejects a mixed path set', async () => {
  const { root, ctx, meta } = await gitContext();
  const key = meta['openai/session'];
  await writeFile(path.join(root, 'deleted.txt'), 'delete-base\n');
  await git(root, 'add', '--', 'deleted.txt');
  await git(root, 'commit', '-m', 'add restore fixture');
  const head = await git(root, 'rev-parse', 'HEAD');

  await writeFile(path.join(root, 'tracked.txt'), 'staged\n');
  await git(root, 'add', '--', 'tracked.txt');
  await writeFile(path.join(root, 'tracked.txt'), 'worktree\n');
  await writeFile(path.join(root, 'added.txt'), 'added\n');
  await git(root, 'add', '--', 'added.txt');
  await git(root, 'rm', '--quiet', '--', 'deleted.txt');
  await writeFile(path.join(root, 'untracked.txt'), 'untracked\n');

  const paths = ['tracked.txt', 'added.txt', 'deleted.txt', 'untracked.txt'];
  const snapshot = await captureGitRestoreSnapshot(ctx, key, paths);
  assert.deepEqual(snapshot.changedPaths, ['added.txt', 'deleted.txt', 'tracked.txt']);
  assert.ok(snapshot.stagedBytes > 0);
  assert.ok(snapshot.worktreeBytes > 0);

  await git(root, 'restore', `--source=${snapshot.head}`, '--staged', '--worktree', '--', ...snapshot.changedPaths);
  const rebuilt = await restoreGitSnapshot(ctx, key, snapshot);
  assert.equal(rebuilt.ok, true);
  assert.deepEqual(rebuilt.steps.map(step => step.step), ['baseline', 'staged_patch', 'worktree_patch']);

  const beforeStatus = await git(root, 'status', '--porcelain=v1');
  const failed = await callTool(ctx, 'git_restore', {
    paths,
    staged: true,
    worktree: true,
    confirm: true,
    expected_head: head
  }, meta);
  assert.equal(failed.ok, false);
  assert.equal(failed.error.code, 'GIT_RESTORE_FAILED');
  assert.equal(failed.error.details.rollback_protected, true);
  assert.equal(failed.error.details.rollback_ok, true);
  assert.ok(failed.error.details.snapshot_bytes > 0);
  assert.equal(await git(root, 'status', '--porcelain=v1'), beforeStatus);
  assert.equal((await readFile(path.join(root, 'tracked.txt'), 'utf8')).replaceAll('\r\n', '\n'), 'worktree\n');
  assert.equal((await readFile(path.join(root, 'added.txt'), 'utf8')).replaceAll('\r\n', '\n'), 'added\n');
  assert.equal(await pathExists(path.join(root, 'deleted.txt')), false);
  assert.equal((await readFile(path.join(root, 'untracked.txt'), 'utf8')).replaceAll('\r\n', '\n'), 'untracked\n');
});

test('guarded mode creates a permission request and resumes the edit', async () => {
  const { root, ctx, meta } = await context('guarded');
  await writeFile(path.join(root, 'hello.txt'), 'before\n');
  await select(ctx, meta);
  const pending = await callTool(ctx, 'edit_file', { path: 'hello.txt', edits: [{ type: 'replace', old_text: 'before', new_text: 'after' }] }, meta);
  assert.equal(pending.ok, false);
  assert.equal(pending.error.code, 'PERMISSION_REQUIRED');
  const resumeId = pending.error.details.permission_request.resume_id;
  const declined = await callTool(ctx, 'request_permissions', { resume_id: resumeId }, meta);
  assert.equal(declined.ok, false);
  assert.equal(declined.error.code, 'PERMISSION_NOT_APPROVED');
  const resumed = await callTool(ctx, 'request_permissions', {
    resume_id: resumeId,
    approve: true,
    confirm: true,
    scope: 'once'
  }, meta);
  assert.equal(resumed.ok, true);
  assert.equal(resumed.resumed, true);
  assert.equal(resumed.permission_grant.status, 'granted_and_resumed');
  assert.equal(await readFile(path.join(root, 'hello.txt'), 'utf8'), 'after\n');
  await ctx.usageStore.flush();
  const telemetry = await ctx.usageStore.query({ scope: 'current_runtime', exclude_tools: [], include_records: true });
  assert.equal(telemetry.records.filter(record => record.tool === 'edit').length, 1, 'internal resume must not be counted as a second MCP request');
});

test('request_permissions requires a resume ID except in dangerous mode', async () => {
  const trusted = await context('trusted');
  await select(trusted.ctx, trusted.meta);
  const unsupported = await callTool(trusted.ctx, 'request_permissions', {
    tool_name: 'exec_command',
    permission: 'inline_script',
    reason: 'test direct grant'
  }, trusted.meta);
  assert.equal(unsupported.ok, false);
  assert.equal(unsupported.status, 'unsupported');
  assert.equal(unsupported.error.code, 'RESUME_ID_REQUIRED');

  const dangerous = await context('dangerous');
  await select(dangerous.ctx, dangerous.meta);
  const granted = await callTool(dangerous.ctx, 'request_permissions', {
    tool_name: 'exec_command',
    permission: 'inline_script',
    reason: 'test dangerous grant'
  }, dangerous.meta);
  assert.equal(granted.ok, true);
  assert.equal(granted.status, 'granted');
  assert.equal(granted.grant_id, 'dangerously-skip-all-permissions');
});

test('exec_many runs a dependency DAG and retains output sessions', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const graph = await callTool(ctx, 'exec_many', {
    mode: 'dag', max_parallel: 2,
    commands: [
      { id: 'first', program: nodeProgram, args: ['-e', 'process.stdout.write("first")'] },
      { id: 'second', depends_on: ['first'], program: nodeProgram, args: ['-e', 'process.stdout.write("second")'] }
    ]
  }, meta);
  assert.equal(graph.ok, true);
  assert.equal(graph.graph_completed, true);
  assert.equal(graph.detached, false);
  assert.equal(graph.failed_command_count, 0);
  assert.equal(graph.results[0].stdout, 'first');
  assert.equal(graph.results[1].stdout, 'second');
  const sessions = await callTool(ctx, 'list_sessions', {}, meta);
  assert.equal(sessions.count, 2);
});

test('exec_many run_if keeps generic finalizers running after dependency failure', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const graph = await callTool(ctx, 'exec_many', {
    mode: 'dag',
    stop_on_error: true,
    commands: [
      { id: 'fail', program: nodeProgram, args: ['-e', 'process.exit(7)'] },
      { id: 'cleanup', depends_on: ['fail'], run_if: 'always', program: nodeProgram, args: ['-e', 'process.stdout.write("cleanup")'] },
      { id: 'on-failure', depends_on: ['fail'], run_if: 'failure', program: nodeProgram, args: ['-e', 'process.stdout.write("failure-handler")'] },
      { id: 'success-only', depends_on: ['fail'], program: nodeProgram, args: ['-e', 'process.stdout.write("must-not-run")'] }
    ]
  }, meta);
  assert.equal(graph.ok, true, JSON.stringify(graph));
  assert.equal(graph.command_ok, false, JSON.stringify(graph));
  assert.deepEqual(graph.failed_command_ids, ['fail']);
  assert.equal(graph.results.find(result => result.id === 'cleanup').stdout, 'cleanup');
  assert.equal(graph.results.find(result => result.id === 'on-failure').stdout, 'failure-handler');
  assert.equal(graph.results.find(result => result.id === 'success-only').skipped, true);
  assert.equal(graph.results.find(result => result.id === 'success-only').skip_reason, 'dependency_failed');
  assert.equal(graph.results.find(result => result.id === 'cleanup').run_if, 'always');
  assert.equal(graph.results.find(result => result.id === 'on-failure').run_if, 'failure');
});

test('exec_many run_if always finalizer runs after a command timeout', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const graph = await callTool(ctx, 'exec_many', {
    mode: 'dag',
    stop_on_error: true,
    commands: [
      { id: 'timeout', program: nodeProgram, args: ['-e', 'setTimeout(() => process.stdout.write("late"), 5000)'], timeout_ms: 150 },
      { id: 'cleanup', depends_on: ['timeout'], run_if: 'always', program: nodeProgram, args: ['-e', 'process.stdout.write("cleanup-after-timeout")'] }
    ]
  }, meta);
  assert.equal(graph.ok, true, JSON.stringify(graph));
  assert.equal(graph.command_ok, false, JSON.stringify(graph));
  assert.deepEqual(graph.failed_command_ids, ['timeout']);
  assert.equal(graph.results.find(result => result.id === 'cleanup').stdout, 'cleanup-after-timeout');
  assert.equal(graph.results.find(result => result.id === 'cleanup').run_if, 'always');
  assert.equal(graph.first_failure?.termination_reason, 'process_timeout');
});

test('exec_many failure-only branches are neutral skips when dependencies succeed', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const graph = await callTool(ctx, 'exec_many', {
    mode: 'dag',
    commands: [
      { id: 'pass', program: nodeProgram, args: ['-e', 'process.stdout.write("pass")'] },
      { id: 'cleanup', depends_on: ['pass'], run_if: 'always', program: nodeProgram, args: ['-e', 'process.stdout.write("cleanup")'] },
      { id: 'on-failure', depends_on: ['pass'], run_if: 'failure', program: nodeProgram, args: ['-e', 'process.stdout.write("must-not-run")'] }
    ]
  }, meta);
  assert.equal(graph.ok, true, JSON.stringify(graph));
  assert.equal(graph.command_ok, true, JSON.stringify(graph));
  assert.equal(graph.graph_execution_ok, true, JSON.stringify(graph));
  assert.equal(graph.results.find(result => result.id === 'cleanup').stdout, 'cleanup');
  assert.equal(graph.results.find(result => result.id === 'on-failure').skipped, true);
  assert.equal(graph.results.find(result => result.id === 'on-failure').skip_reason, 'run_condition_not_met');
  assert.deepEqual(graph.conditional_skipped_command_ids, ['on-failure']);
});

test('exec_many reports failed and skipped command ids with bounded recovery guidance', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const graph = await callTool(ctx, 'exec_many', {
    mode: 'dag',
    commands: [
      { id: 'fail', program: nodeProgram, args: ['-e', 'process.exit(7)'] },
      { id: 'blocked', depends_on: ['fail'], program: nodeProgram, args: ['-e', 'process.stdout.write("never")'] }
    ]
  }, meta);
  assert.equal(graph.ok, true);
  assert.equal(graph.command_ok, false);
  assert.equal(graph.graph_execution_ok, false);
  assert.deepEqual(graph.failed_command_ids, ['fail']);
  assert.deepEqual(graph.skipped_command_ids, ['blocked']);
  assert.equal(graph.first_failure.id, 'fail');
  assert.equal(graph.first_failure.exit_code, 7);
  assert.equal(graph.recovery_actions[0].action, 'retry_failed_commands');
  assert.deepEqual(graph.recovery_actions[0].command_ids, ['fail']);
  assert.equal(graph.recovery_actions[1].action, 'retry_affected_subgraph');
  assert.deepEqual(graph.recovery_actions[1].command_ids, ['blocked']);
  assert.deepEqual(graph.recovery_actions[1].failed_command_ids, ['fail']);
});

test('exec_many only graph-deduplicates explicitly idempotent children', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const idempotent = {
    mode: 'sequential',
    commands: [{
      id: 'safe',
      operation_id: 'safe-graph-child',
      program: nodeProgram,
      args: ['-e', 'process.stdout.write("safe")']
    }]
  };
  const first = await callTool(ctx, 'exec_many', idempotent, meta);
  const second = await callTool(ctx, 'exec_many', idempotent, meta);
  assert.equal(first.graph_execution_ok, true);
  assert.equal(second.graph_operation_id, first.graph_operation_id);
  assert.equal(second.reattached, true);
  assert.equal(second.graph_deduplicated, true);
  assert.equal(second.retained_graph_count, first.retained_graph_count);

  const ordinary = {
    mode: 'sequential',
    commands: [{ id: 'ordinary', program: nodeProgram, args: ['-e', 'process.stdout.write("ordinary")'] }]
  };
  const ordinaryFirst = await callTool(ctx, 'exec_many', ordinary, meta);
  const ordinarySecond = await callTool(ctx, 'exec_many', ordinary, meta);
  assert.notEqual(ordinarySecond.graph_operation_id, ordinaryFirst.graph_operation_id);
  assert.equal(ordinarySecond.graph_deduplicated, undefined);
  assert.equal(ordinaryFirst.results[0].stdout, 'ordinary');
  assert.equal(ordinarySecond.results[0].stdout, 'ordinary');
});

test('exec_many cancellation preserves a pre-existing deduplicated child session', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const child = {
    operation_id: 'shared-preexisting-child',
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.exit(0), 30_000)'],
    timeout_ms: 60_000,
    yield_time_ms: 0,
    output_mode: 'none'
  };
  const original = await callTool(ctx, 'exec_command', child, meta);
  assert.equal(original.process_still_running, true);

  const graph = await callTool(ctx, 'exec_many', {
    operation_id: 'shared-session-graph',
    yield_time_ms: 5,
    mode: 'sequential',
    commands: [{ id: 'shared', ...child }]
  }, meta);
  assert.equal(graph.graph_completed, false);

  const cancelled = await callTool(ctx, 'exec_many', {
    operation_id: 'shared-session-graph',
    action: 'cancel',
    yield_time_ms: 2_000
  }, meta);
  assert.equal(cancelled.graph_status, 'cancelled');
  assert.equal(cancelled.cancelled_session_count, 0);
  assert.equal(cancelled.recovery_actions.length, 0);
  assert.equal(cancelled.results[0].shared_session_preserved, true);
  assert.equal(cancelled.results[0].session_id, original.session_id);

  const stillShared = await callTool(ctx, 'resolve_operation', {
    operation_id: 'shared-preexisting-child',
    output_mode: 'none'
  }, meta);
  assert.equal(stillShared.session_id, original.session_id);
  assert.equal(stillShared.termination_reason, 'running');
  assert.equal(stillShared.process_still_running, true);

  const cleaned = await callTool(ctx, 'kill_session', {
    session_id: original.session_id,
    signal: 'KILL',
    wait_ms: 5_000
  }, meta);
  assert.equal(cleaned.ok, true, JSON.stringify(cleaned));
  assert.equal(cleaned.killed, true, JSON.stringify(cleaned));
  assert.equal(cleaned.process_still_running, false, JSON.stringify(cleaned));
});

test('exec_many keeps command setup failures inside the graph result', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const graph = await callTool(ctx, 'exec_many', {
    mode: 'sequential',
    commands: [{
      id: 'missing-workdir',
      program: nodeProgram,
      args: ['-e', 'process.stdout.write("never")'],
      workdir: 'missing-exec-many-workdir'
    }]
  }, meta);
  assert.equal(graph.ok, true);
  assert.equal(graph.command_ok, false);
  assert.deepEqual(graph.failed_command_ids, ['missing-workdir']);
  assert.equal(graph.results[0].command_ok, false);
  assert.equal(typeof graph.results[0].error.code, 'string');
  assert.equal(graph.first_failure.id, 'missing-workdir');
  assert.equal(graph.first_failure.error_code, graph.results[0].error.code);
});

test('exec_many detaches long graphs and reattaches without starting duplicate children', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const operationId = 'retained-long-graph';
  const started = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    yield_time_ms: 10,
    mode: 'sequential',
    commands: [{
      id: 'slow',
      program: nodeProgram,
      args: ['-e', 'setTimeout(() => process.stdout.write("slow-done"), 180)'],
      timeout_ms: 5_000
    }]
  }, meta);
  assert.equal(started.ok, true);
  assert.equal(started.graph_operation_id, operationId);
  assert.equal(started.graph_completed, false);
  assert.equal(started.graph_status, 'running');
  assert.equal(started.detached, true);
  assert.equal(started.reattached, false);
  assert.equal(started.result_mode, 'summary');
  assert.equal(started.result_output_included, false);
  assert.equal(started.results[0].stdout, undefined);
  assert.equal(started.graph_yield_ms, 10);
  assert.equal(started.graph_requested_yield_ms, 10);
  assert.equal(started.next_actions[0].tool, 'exec_many');
  assert.deepEqual(started.next_actions[0].arguments, { operation_id: operationId, yield_time_ms: 20000, result_mode: 'summary' });

  const verboseProgress = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    yield_time_ms: 0,
    result_mode: 'full'
  }, meta);
  assert.equal(verboseProgress.graph_completed, false);
  assert.equal(verboseProgress.result_mode, 'full');
  assert.equal(verboseProgress.result_output_included, true);

  const finalized = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    yield_time_ms: 300_000,
    result_mode: 'full'
  }, meta);
  assert.equal(finalized.ok, true);
  assert.equal(finalized.graph_requested_yield_ms, 300_000);
  assert.equal(finalized.graph_yield_ms, 20_000);
  assert.equal(finalized.graph_completed, true);
  assert.equal(finalized.terminal, true);
  assert.equal(finalized.detached, false);
  assert.equal(finalized.reattached, true);
  assert.equal(finalized.commands_executed, 1);
  assert.equal(finalized.result_mode, 'full');
  assert.equal(finalized.results[0].stdout, 'slow-done');

  const sessionsAfterFinal = await callTool(ctx, 'list_sessions', {}, meta);
  assert.equal(sessionsAfterFinal.count, 1);
  const attachedAgain = await callTool(ctx, 'exec_many', { operation_id: operationId, yield_time_ms: 0 }, meta);
  const sessionsAfterReattach = await callTool(ctx, 'list_sessions', {}, meta);
  assert.equal(attachedAgain.graph_completed, true);
  assert.equal(attachedAgain.results[0].stdout, 'slow-done');
  assert.equal(sessionsAfterReattach.count, sessionsAfterFinal.count);

  await ctx.usageStore.flush();
  const usage = await ctx.usageStore.query({
    scope: 'current_runtime', tools: ['exec_many'], exclude_tools: [], include_records: true
  });
  const detachedRecord = usage.records.find(record => record.graph_operation_id === operationId && record.detached === true);
  assert.equal(detachedRecord.graph_status, 'running');
  assert.equal(detachedRecord.graph_completed, false);
  assert.ok(Number(detachedRecord.running_command_id_count ?? 0) + Number(detachedRecord.pending_command_id_count ?? 0) >= 1);
  const completedRecord = usage.records.find(record => record.graph_operation_id === operationId && record.graph_completed === true);
  assert.equal(completedRecord.reattached, true);
  assert.equal(completedRecord.completed_command_id_count, 1);
});

test('detached exec_many preserves failed and skipped recovery after reattachment', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const started = await callTool(ctx, 'exec_many', {
    operation_id: 'retained-failure-graph',
    yield_time_ms: 5,
    mode: 'dag',
    commands: [
      { id: 'fail', program: nodeProgram, args: ['-e', 'setTimeout(() => process.exit(7), 120)'], timeout_ms: 5_000 },
      { id: 'blocked', depends_on: ['fail'], program: nodeProgram, args: ['-e', 'process.stdout.write("never")'] }
    ]
  }, meta);
  assert.equal(started.graph_completed, false);
  assert.equal(started.detached, true);

  const finalized = await callTool(ctx, 'exec_many', {
    operation_id: 'retained-failure-graph',
    yield_time_ms: 2_000
  }, meta);
  assert.equal(finalized.graph_completed, true);
  assert.equal(finalized.ok, true);
  assert.equal(finalized.command_ok, false);
  assert.deepEqual(finalized.failed_command_ids, ['fail']);
  assert.deepEqual(finalized.skipped_command_ids, ['blocked']);
  assert.equal(finalized.first_failure.id, 'fail');
  assert.equal(finalized.first_failure.exit_code, 7);
  assert.equal(finalized.recovery_actions[0].action, 'retry_failed_commands');
  assert.deepEqual(finalized.recovery_actions[0].command_ids, ['fail']);
  assert.equal(finalized.commands_executed, 1);
});

test('exec_many command failures remain successful tool calls and status preserves graph execution failure', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const operationId = 'retained-failed-status';
  const failedRun = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    commands: [{ id: 'fail', program: nodeProgram, args: ['-e', 'process.exit(9)'] }]
  }, meta);
  assert.equal(failedRun.ok, true);
  assert.equal(failedRun.command_ok, false);
  assert.equal(failedRun.graph_execution_ok, false);

  const status = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'status'
  }, meta);
  assert.equal(status.ok, true);
  assert.equal(status.control_ok, true);
  assert.equal(status.graph_execution_ok, false);
  assert.deepEqual(status.failed_command_ids, ['fail']);

  await ctx.usageStore.flush();
  const usage = await ctx.usageStore.query({
    scope: 'current_runtime', tools: ['exec_many'], exclude_tools: [], include_records: true
  });
  const runRecord = usage.records.find(record => record.graph_operation_id === operationId && record.graph_action === 'run');
  const statusRecord = usage.records.find(record => record.graph_operation_id === operationId && record.graph_action === 'status');
  assert.equal(runRecord.outcome, 'success');
  assert.equal(statusRecord.outcome, 'success');
  assert.equal(statusRecord.graph_execution_ok, false);
  assert.equal(statusRecord.control_ok, true);
});

test('exec_many retained graph operation ids reject conflicts and unknown reattachments', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const operationId = 'retained-conflict-graph';
  const command = {
    id: 'slow',
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.stdout.write("original"), 120)'],
    timeout_ms: 5_000
  };
  const started = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    yield_time_ms: 0,
    commands: [command]
  }, meta);
  assert.equal(started.graph_operation_id, operationId);

  const conflict = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    yield_time_ms: 0,
    commands: [{ ...command, args: ['-e', 'process.stdout.write("different")'] }]
  }, meta);
  assert.equal(conflict.ok, false);
  assert.equal(conflict.error.code, 'OPERATION_ID_CONFLICT');

  const missing = await callTool(ctx, 'exec_many', {
    operation_id: 'missing-retained-graph',
    yield_time_ms: 0
  }, meta);
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'COMMAND_GRAPH_OPERATION_NOT_FOUND');

  const finalized = await callTool(ctx, 'exec_many', { operation_id: operationId, yield_time_ms: 2_000 }, meta);
  assert.equal(finalized.ok, true);
  assert.equal(finalized.results[0].stdout, 'original');
});

test('exec_many retained graph control actions report status, terminate children, and forget completed graphs', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const operationId = 'retained-control-graph';
  const started = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    yield_time_ms: 5,
    mode: 'sequential',
    commands: [
      {
        id: 'long',
        program: nodeProgram,
        args: ['-e', 'setTimeout(() => process.stdout.write("should-not-complete"), 5_000)'],
        timeout_ms: 10_000
      },
      {
        id: 'pending',
        program: nodeProgram,
        args: ['-e', 'process.stdout.write("never-started")'],
        timeout_ms: 5_000
      }
    ]
  }, meta);
  assert.equal(started.graph_completed, false);

  let status = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'status'
  }, meta);
  for (let attempt = 0; attempt < 20 && !status.results.some(result => result.session_id); attempt += 1) {
    await new Promise(resolve => setTimeout(resolve, 10));
    status = await callTool(ctx, 'exec_many', { operation_id: operationId, action: 'status' }, meta);
  }
  assert.equal(status.graph_action, 'status');
  assert.equal(status.graph_status, 'running');
  assert.equal(status.graph_wait_ms, 0);
  assert.equal(status.graph_yield_ms, 0);
  assert.equal(status.cancel_requested, false);
  assert.ok(status.results.some(result => result.session_id), 'test must observe a retained child before cancellation');

  const cancelled = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'cancel',
    reason: 'test cancellation',
    yield_time_ms: 2_000
  }, meta);
  assert.equal(cancelled.graph_action, 'cancel');
  assert.equal(cancelled.ok, true);
  assert.equal(cancelled.control_ok, true);
  assert.equal(cancelled.graph_execution_ok, false);
  assert.equal(cancelled.cancel_accepted, true);
  assert.equal(cancelled.cancel_requested, true);
  assert.equal(cancelled.cancel_reason, 'test cancellation');
  assert.equal(cancelled.graph_completed, true);
  assert.equal(cancelled.graph_status, 'cancelled');
  assert.equal(cancelled.terminal, true);
  assert.ok(cancelled.cancelled_session_count >= 1);
  assert.ok(cancelled.skipped_command_ids.includes('pending'));
  const longResult = cancelled.results.find(result => result.id === 'long');
  assert.equal(longResult.command_ok, false);
  assert.equal(longResult.termination_reason, 'graph_cancelled');

  const sessions = await callTool(ctx, 'list_sessions', {}, meta);
  const graphSessions = sessions.sessions.filter(session => session.termination_reason === 'graph_cancelled');
  assert.ok(graphSessions.length >= 1);
  assert.ok(graphSessions.every(session => session.process_still_running === false));

  const cancelledAgain = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'cancel',
    yield_time_ms: 0
  }, meta);
  assert.equal(cancelledAgain.cancel_accepted, false);
  assert.equal(cancelledAgain.graph_status, 'cancelled');

  const forgotten = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'forget'
  }, meta);
  assert.equal(forgotten.ok, true);
  assert.equal(forgotten.forgotten, true);
  assert.equal(forgotten.graph_status, 'forgotten');

  const afterForget = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'status'
  }, meta);
  assert.equal(afterForget.ok, false);
  assert.equal(afterForget.error.code, 'COMMAND_GRAPH_OPERATION_NOT_FOUND');
});

test('exec_many retained graph control actions require an operation id and reject commands', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const missingId = await callTool(ctx, 'exec_many', { action: 'status' }, meta);
  assert.equal(missingId.ok, false);
  assert.equal(missingId.error.code, 'INVALID_ARGUMENT');

  const withCommands = await callTool(ctx, 'exec_many', {
    action: 'cancel',
    operation_id: 'no-such-graph',
    commands: [{ id: 'unexpected', program: nodeProgram, args: ['-e', 'process.exit(0)'] }]
  }, meta);
  assert.equal(withCommands.ok, false);
  assert.equal(withCommands.error.code, 'INVALID_ARGUMENT');
});

test('command graph retention is owned by a dedicated module behind the processes facade', async () => {
  const commandGraph = await import('../dist/processes/commandGraph.js');
  assert.equal(commandGraph.MAX_RETAINED_COMMAND_GRAPHS, MAX_RETAINED_COMMAND_GRAPHS);
  assert.equal(commandGraph.pruneRetainedCommandGraphs, pruneRetainedCommandGraphs);
});

test('command graph runtime orchestration is owned by the dedicated module', async () => {
  const commandGraph = await import('../dist/processes/commandGraph.js');
  assert.equal(typeof commandGraph.runCommandGraph, 'function');
  assert.equal(typeof commandGraph.abortRetainedCommandGraphs, 'function');
});

test('retained exec_many capacity eviction removes oldest completed graphs but never active graphs', () => {
  const now = Date.now();
  const graphs = new Map();
  for (let index = 0; index < MAX_RETAINED_COMMAND_GRAPHS - 1; index += 1) {
    const id = `active-${index}`;
    graphs.set(id, { id, createdAt: now - index });
  }
  graphs.set('completed-old', { id: 'completed-old', createdAt: now - 1_000, completedAt: now - 500 });
  graphs.set('completed-new', { id: 'completed-new', createdAt: now - 500, completedAt: now - 100 });

  const evicted = pruneRetainedCommandGraphs(graphs, 1);
  assert.equal(evicted, 2);
  assert.equal(graphs.size, MAX_RETAINED_COMMAND_GRAPHS - 1);
  assert.equal(graphs.has('completed-old'), false);
  assert.equal(graphs.has('completed-new'), false);
  assert.equal([...graphs.keys()].filter(id => id.startsWith('active-')).length, MAX_RETAINED_COMMAND_GRAPHS - 1);
});

test('exec_many status defaults to compact results and full detail remains opt-in', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const operationId = 'retained-compact-status';
  const completed = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    commands: [{
      id: 'large-output',
      program: nodeProgram,
      args: ['-e', 'process.stdout.write("x".repeat(32768))'],
      max_output_bytes: 65_536,
      timeout_ms: 5_000
    }]
  }, meta);
  assert.equal(completed.graph_completed, true);
  assert.equal(completed.result_mode, 'summary');
  assert.equal(completed.result_mode_reason, 'large_output');
  assert.equal(completed.auto_compacted_output_bytes, 32768);
  assert.equal(completed.result_output_included, false);
  assert.equal(completed.results[0].stdout, undefined);
  assert.equal(completed.results[0].stdout_bytes, 32768);
  assert.equal(completed.results[0].output_refs.stdout.startsWith('output://'), true);

  const summary = await callTool(ctx, 'exec_many', { operation_id: operationId, action: 'status' }, meta);
  assert.equal(summary.result_mode, 'summary');
  assert.equal(summary.ok, true);
  assert.equal(summary.control_ok, true);
  assert.equal(summary.graph_execution_ok, true);
  assert.equal(summary.results_included, true);
  assert.equal(summary.result_output_included, false);
  assert.equal(summary.results[0].stdout, undefined);
  assert.equal(summary.results[0].stdout_bytes, 32768);
  assert.ok(summary.retention_expires_ts_ms > summary.graph_completed_ts_ms);
  assert.ok(summary.retention_remaining_ms > 0);

  const full = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'status',
    result_mode: 'full'
  }, meta);
  assert.equal(full.result_mode, 'full');
  assert.equal(full.result_output_included, true);
  assert.equal(full.results[0].stdout.length, 32768);
  assert.ok(JSON.stringify(summary).length < JSON.stringify(full).length / 4);

  const none = await callTool(ctx, 'exec_many', {
    operation_id: operationId,
    action: 'status',
    result_mode: 'none'
  }, meta);
  assert.equal(none.results_included, false);
  assert.equal(none.results.length, 0);
  assert.equal(none.results_omitted_count, 1);
});

test('identical in-flight idempotent exec calls coalesce without duplicating harness operations', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const command = {
    operation_id: 'coalesce-inflight-operation',
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.stdout.write("coalesced"), 200)'],
    yield_time_ms: 100,
    timeout_ms: 5_000,
    output_mode: 'none'
  };
  const [first, second] = await Promise.all([
    callTool(ctx, 'exec_command', command, meta),
    callTool(ctx, 'exec_command', command, meta)
  ]);
  assert.equal(first.session_id, second.session_id);
  const responses = [first, second];
  assert.equal(responses.filter(result => result.coalesced_inflight === true).length, 1);
  const follower = responses.find(result => result.coalesced_inflight === true);
  assert.ok(follower.coalesced_wait_ms >= 0);
  assert.equal(follower.admission_queue_wait_ms, 0);
  assert.equal(follower.workspace_lock_wait_ms, 0);

  const operations = await callTool(ctx, 'operation_log', { cursor: 0, limit: 100 }, meta);
  const execStarts = operations.operations.filter(row => row.tool === 'exec_command' && row.kind === 'started');
  assert.equal(execStarts.length, 1);

  await ctx.usageStore.flush();
  const usage = await ctx.usageStore.query({
    scope: 'current_runtime', tools: ['exec_command'], exclude_tools: [], include_records: true
  });
  assert.equal(usage.records.length, 2);
  assert.equal(usage.records.filter(record => record.coalesced_inflight === true).length, 1);
});

test('ordinary task mutations are never coalesced', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const [first, second] = await Promise.all([
    callTool(ctx, 'start_task', { objective: 'Concurrent task mutation' }, meta),
    callTool(ctx, 'start_task', { objective: 'Concurrent task mutation' }, meta)
  ]);
  assert.equal([first, second].filter(result => result.ok === true).length, 1);
  const failed = [first, second].find(result => result.ok === false);
  assert.equal(failed.error.code, 'TASK_ALREADY_ACTIVE');
  assert.equal(first.coalesced_inflight, undefined);
  assert.equal(second.coalesced_inflight, undefined);
});

test('retained commands support operation reattachment, post-checks, environment removal and absolute output offsets', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  process.env.NODE_AGENT_REMOVE_ME = 'secret';
  try {
    const command = {
      operation_id: 'stable-operation',
      program: nodeProgram,
      args: ['-e', 'process.stdout.write(process.env.NODE_AGENT_REMOVE_ME ?? "removed")'],
      remove_env: ['NODE_AGENT_REMOVE_ME'],
      yield_time_ms: 30000,
      output_mode: 'all',
      post_checks: [
        {
          name: 'verify-node',
          program: nodeProgram,
          args: ['-e', 'process.stdout.write("verified")'],
          expected_exit_code: 0
        }
      ]
    };
    const started = await callTool(ctx, 'exec_command', command, meta);
    const finalized = await callTool(ctx, 'wait_command', {
      session_id: started.session_id,
      cursor: started.latest_cursor,
      until: 'finalized',
      timeout_ms: 30000,
      output_mode: 'all'
    }, meta);
    assert.equal(finalized.status, 'exited');
    assert.equal(finalized.command_ok, true);
    assert.equal(finalized.stdout, 'removed');
    assert.equal(finalized.verification_ok, true);
    assert.equal(finalized.post_checks[0].ok, true);
    assert.equal(finalized.post_checks[0].stdout, 'verified');
    assert.equal(finalized.process_tree_contained, process.platform !== 'win32');
    assert.equal(finalized.process_tree_control, process.platform === 'win32' ? 'taskkill_tree' : 'process_group');

    const attached = await callTool(ctx, 'exec_command', command, meta);
    assert.equal(attached.session_id, started.session_id);
    const resolved = await callTool(ctx, 'resolve_operation', {
      operation_id: 'stable-operation',
      output_mode: 'none'
    }, meta);
    assert.equal(resolved.session_id, started.session_id);

    const conflict = await callTool(ctx, 'exec_command', {
      ...command,
      args: ['-e', 'process.stdout.write("different")']
    }, meta);
    assert.equal(conflict.ok, false);
    assert.equal(conflict.error.code, 'OPERATION_ID_CONFLICT');

    const exited = await callTool(ctx, 'list_sessions', { status: 'exited' }, meta);
    assert.ok(exited.sessions.some(session => session.session_id === started.session_id));
    const unfinished = await callTool(ctx, 'list_sessions', { include_finalized: false }, meta);
    assert.ok(unfinished.sessions.every(session => session.finalized_ts_ms === null));
  } finally {
    delete process.env.NODE_AGENT_REMOVE_ME;
  }

  ctx.config.limits.maxOutputBytes = 8;
  const truncated = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'process.stdout.write("0123456789abcdef")'],
    yield_time_ms: 30000,
    output_mode: 'all'
  }, meta);
  const complete = await callTool(ctx, 'wait_command', {
    session_id: truncated.session_id,
    until: 'finalized',
    timeout_ms: 30000,
    output_mode: 'all'
  }, meta);
  assert.equal(complete.stdout, '89abcdef');
  assert.equal(complete.stdout_retained_from, 8);
  const output = await callTool(ctx, 'read_output', {
    output_ref: complete.output_refs.stdout,
    offset: 0,
    limit: 32
  }, meta);
  assert.equal(output.cursor_expired, true);
  assert.equal(output.offset, 8);
  assert.equal(output.content, '89abcdef');
  assert.equal(output.total_bytes, 16);
});

test('wait_command separates wait timeout from process completion', async () => {
  const { ctx, meta } = await context();
  await select(ctx, meta);
  const started = await callTool(ctx, 'exec_command', {
    program: nodeProgram,
    args: ['-e', 'setTimeout(() => process.exit(0), 500)'],
    yield_time_ms: 0,
    timeout_ms: 5_000,
    output_mode: 'none'
  }, meta);
  const waited = await callTool(ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: started.latest_cursor,
    until: 'finalized',
    timeout_ms: 20,
    heartbeat_ms: 0,
    output_mode: 'none'
  }, meta);
  assert.equal(waited.ok, true);
  assert.equal(waited.request_timed_out, true);
  assert.equal(waited.wait_timed_out, true);
  assert.equal(waited.wait_completed, true);
  assert.equal(waited.process_completed, false);
  assert.equal(waited.terminal, false);
  assert.equal(waited.progress_since_last_wait, false);
  assert.equal(waited.next_wait_ms, 20);
  assert.equal(waited.process_timed_out, false);
  assert.equal(waited.process_still_running, true);
  assert.match(waited.suggestion, /still running/i);

  const finalized = await callTool(ctx, 'wait_command', {
    session_id: started.session_id,
    cursor: waited.latest_cursor,
    until: 'finalized',
    timeout_ms: 5_000,
    output_mode: 'none'
  }, meta);
  assert.equal(finalized.process_completed, true);
  assert.equal(finalized.terminal, true);
  assert.equal(finalized.next_wait_ms, null);
  assert.equal(finalized.request_timed_out, false);
});

test('production WSS E2E runner refuses to run without explicit credentials', async () => {
  const script = fileURLToPath(new URL('../scripts/test-production-wss.mjs', import.meta.url));
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('CTMCP_E2E_')));
  await assert.rejects(
    execFile(process.execPath, [script], { cwd: path.dirname(script), env, encoding: 'utf8' }),
    error => {
      assert.match(String(error.stderr), /CTMCP_E2E_BUILTIN_PUBLIC_URL is required/);
      return true;
    }
  );
});

test('history and durable task state survive tool calls', async () => {
  const { root, ctx, meta } = await context();
  await select(ctx, meta);
  const bootstrap = await callTool(ctx, 'history_session_bootstrap', { session_key: 'task-test' }, meta);
  assert.equal(bootstrap.ok, true);
  const started = await callTool(ctx, 'start_task', { objective: 'Node agent task' }, meta);
  assert.equal(started.task.status, 'active');
  const taskId = started.task.id;
  const updated = await callTool(ctx, 'update_task', {
    task_id: taskId,
    completed_steps: ['Implement'],
    pending_steps: ['Verify']
  }, meta);
  assert.deepEqual(updated.task.completed_steps, ['Implement']);
  const paused = await callTool(ctx, 'pause_task', { task_id: taskId }, meta);
  assert.equal(paused.task.status, 'paused');
  const resumed = await callTool(ctx, 'resume_task', { task_id: taskId }, meta);
  assert.equal(resumed.task.status, 'active');
  const finished = await callTool(ctx, 'finish_task', { task_id: taskId, allow_unverified: true, summary: 'Done' }, meta);
  assert.equal(finished.task.status, 'completed_unverified');
  const taskContext = await callTool(ctx, 'task_context', { task_id: taskId }, meta);
  assert.equal(taskContext.task.id, taskId);
  const events = await callTool(ctx, 'list_task_events', { task_id: taskId, cursor: 0, limit: 20 }, meta);
  assert.ok(events.events.length >= 5);
  const checkpoint = await callTool(ctx, 'history_session_checkpoint', { session_key: 'task-test', expected_path: bootstrap.current_path, tests: ['node test'] }, meta);
  assert.equal(checkpoint.ok, true);
  assert.equal(await readFile(path.join(root, bootstrap.current_path), 'utf8').then(value => value.includes('node test')), true);
});
