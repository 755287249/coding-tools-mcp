import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { PNG } from 'pngjs';
import { createAgentRuntime } from '../dist/server.js';
import { toolsForProfile } from '../dist/catalog.js';
import { knowledgeRecordId } from '../dist/knowledge/store.js';
import { selectedSkillAttribution } from '../dist/knowledge/skillAttribution.js';
import { discoverExtensions } from '../dist/extensions/discovery.js';
import { CLIENT_COMPAT_VERSION } from '../dist/version.js';

async function rpc(endpoint, token, request) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: JSON.stringify(request)
  });
  assert.equal(response.status, 200);
  return response.json();
}

async function rpcResponse(endpoint, token, request, extraHeaders = {}) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json', ...extraHeaders },
    body: JSON.stringify(request)
  });
  assert.equal(response.status, 200);
  return { response, body: await response.json() };
}

test('scoped OAuth PKCE and MCP workspace flow', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-http-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-http-state-'));
  await writeFile(path.join(root, 'hello.txt'), 'hello node agent');
  await mkdir(path.join(root, 'skills', 'release-helper'), { recursive: true });
  await writeFile(path.join(root, 'skills', 'release-helper', 'SKILL.md'), [
    '---',
    'name: release-helper',
    'description: Prepare a safe project release.',
    '---',
    '',
    '# Release helper',
    '',
    'Run project release checks before packaging.'
  ].join('\n'));
  const largePayload = `HTTP_LARGE_MARKER:${'x'.repeat(32 * 1024)}`;
  const httpSecret = 'sk-http-contract-secret-abcdefghijklmnopqrstuvwxyz';
  await writeFile(path.join(root, 'large.txt'), largePayload);
  await writeFile(path.join(root, '.env'), httpSecret);
  const image = new PNG({ width: 2, height: 1 });
  image.data = Buffer.from([255, 0, 0, 255, 0, 0, 255, 255]);
  await writeFile(path.join(root, 'pixel.bin'), PNG.sync.write(image));
  const externalMcpFixture = fileURLToPath(new URL('./fixtures/mcp-extension-fixture.mjs', import.meta.url));
  const externalHookFixture = fileURLToPath(new URL('./fixtures/hook-extension-fixture.mjs', import.meta.url));
  await writeFile(path.join(root, '.mcp.json'), JSON.stringify({
    mcpServers: {
      fixture: { type: 'stdio', command: process.execPath, args: [externalMcpFixture] }
    }
  }));
  await mkdir(path.join(root, '.claude'), { recursive: true });
  await writeFile(path.join(root, '.claude', 'settings.json'), JSON.stringify({
    hooks: {
      PreToolUse: [{
        matcher: 'set_default_cwd',
        hooks: [{ type: 'command', command: process.execPath, args: [externalHookFixture, 'block'] }]
      }]
    }
  }));
  const prefix = '/builtin/clients/node-test';
  const publicBaseUrl = `https://public.example${prefix}`;
  const folders = [{ id: 'repo', name: 'Repo', path: root }];
  const extensionDiscovery = await discoverExtensions({ folders, homeDir: null });
  const externalServer = extensionDiscovery.mcpServers.find(server => server.name === 'fixture');
  assert.ok(externalServer);
  const externalHook = extensionDiscovery.hooks.find(hook => hook.provider === 'claude' && hook.event === 'PreToolUse');
  assert.ok(externalHook);
  const config = {
    host: '127.0.0.1', port: 0, publicBaseUrl, dataDir, permissionMode: 'trusted',
    oauth: { clientId: 'chatgpt', password: 'test-password', tokenSecret: 'a sufficiently long test token secret' },
    folders,
    extensions: { hooks: { enabled: [externalHook.key] }, mcp: { enabled: [externalServer.key] } },
    limits: { blockingConcurrency: 4, processConcurrency: 4, activeSessionLimit: 16, maxOutputBytes: 1024 * 1024 }
  };
  const runtime = await createAgentRuntime(config);
  const server = runtime.server;
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => runtime.close());
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  const localBase = `http://127.0.0.1:${address.port}`;

  const health = await fetch(`${localBase}${prefix}/health`).then(response => response.json());
  assert.equal(health.clientCompatVersion, CLIENT_COMPAT_VERSION);
  const info = await fetch(`${localBase}${prefix}/mcp/info`).then(response => response.json());
  assert.equal(info.clientCompatVersion, CLIENT_COMPAT_VERSION);

  const metadata = await fetch(`${localBase}/.well-known/oauth-authorization-server${prefix}`).then(response => response.json());
  assert.equal(metadata.issuer, publicBaseUrl);
  assert.equal(metadata.authorization_endpoint, `${publicBaseUrl}/oauth/authorize`);
  const resource = await fetch(`${localBase}/.well-known/oauth-protected-resource${prefix}/mcp`).then(response => response.json());
  assert.equal(resource.resource, `${publicBaseUrl}/mcp`);

  const verifier = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~';
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const redirectUri = 'https://chatgpt.com/connector_platform_oauth_redirect';
  const form = new URLSearchParams({ client_id: 'chatgpt', redirect_uri: redirectUri, code_challenge: challenge, code_challenge_method: 'S256', state: 'state-1', password: 'test-password' });
  const authorized = await fetch(`${localBase}${prefix}/oauth/authorize`, { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: form, redirect: 'manual' });
  assert.equal(authorized.status, 303);
  const callback = new URL(authorized.headers.get('location'));
  assert.equal(callback.searchParams.get('state'), 'state-1');
  const code = callback.searchParams.get('code');
  assert.ok(code);

  const tokenResponse = await fetch(`${localBase}${prefix}/oauth/token`, {
    method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ grant_type: 'authorization_code', code, redirect_uri: redirectUri, code_verifier: verifier, client_id: 'chatgpt' })
  });
  assert.equal(tokenResponse.status, 200);
  const { access_token: token } = await tokenResponse.json();
  assert.ok(token);

  const endpoint = `${localBase}${prefix}/mcp`;
  const initialized = await rpc(endpoint, token, { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } });
  assert.equal(initialized.result.protocolVersion, '2025-11-25');
  assert.match(initialized.result.instructions, /conversation_bootstrap/);
  assert.match(initialized.result.instructions, /exec_many\(mode=auto\)/);
  assert.equal(initialized.result.capabilities.prompts.listChanged, false);
  assert.equal(initialized.result.capabilities.resources.subscribe, false);
  assert.match(initialized.result.instructions, /Workspace and enabled Codex\/Claude user-level Skills/);
  const promptList = await rpc(endpoint, token, { jsonrpc: '2.0', id: 30, method: 'prompts/list', params: {} });
  const releasePrompt = promptList.result.prompts.find(prompt => prompt.name === 'project-skill/repo/release-helper');
  assert.ok(releasePrompt);
  assert.equal(releasePrompt.description, 'Prepare a safe project release.');
  const skillMeta = { 'openai/session': 'skill-attribution-http' };
  const loadedPrompt = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 31, method: 'prompts/get',
    params: { name: 'project-skill/repo/release-helper', _meta: skillMeta }
  });
  assert.match(loadedPrompt.result.messages[0].content.text, /Run project release checks before packaging\./);
  assert.match(loadedPrompt.result.messages[0].content.text, /never grant permissions|does not grant permissions/i);
  const folderRuntime = runtime.context.folderRuntimes.get('repo');
  assert.ok(folderRuntime);
  const attributedSkill = selectedSkillAttribution(
    runtime.context.conversations, runtime.context.conversations.identity(skillMeta).key, 'repo'
  );
  assert.equal(attributedSkill.name, 'release-helper');
  assert.equal(attributedSkill.source, 'project');
  assert.match(attributedSkill.contentSha256, /^[a-f0-9]{64}$/);
  const contextIds = ['resource-http-a', 'resource-http-b'].map(value =>
    createHash('sha256').update(`conversation\0${value}`).digest('hex')
  );
  const evolutionKnowledge = {
    id: '',
    schemaVersion: 1,
    scope: 'runtime',
    target: 'tool_evolution',
    status: 'candidate',
    trigger: { tool: 'search_text', variant: 'http-resource' },
    hypothesis: 'Expose pending MCP tool evolution work through opt-in resources.',
    recommendedAction: { proposal: 'http-resource-fast-path' },
    evidence: {
      observations: 5, successes: 5, failures: 0,
      totalDurationMs: 500, totalRequestBytes: 50, totalResponseBytes: 100,
      firstSeenAtMs: 100, lastSeenAtMs: 104,
      conversationContextIds: contextIds
    },
    confidence: 0.5,
    sourceEventIds: ['http-resource-1', 'http-resource-2', 'http-resource-3', 'http-resource-4', 'http-resource-5'],
    counterexampleEventIds: [],
    metadata: { proposalType: 'performance' },
    createdAtMs: 100,
    updatedAtMs: 104
  };
  evolutionKnowledge.id = knowledgeRecordId(evolutionKnowledge);
  const storedKnowledge = await folderRuntime.knowledgeStore.upsert(evolutionKnowledge);
  const evolutionProposal = await folderRuntime.toolEvolutionProposalStore.plan(storedKnowledge, 'a'.repeat(40), 105);
  const evolutionUri = `tool-evolution://coding-tools/repo/${evolutionProposal.proposalId}`;
  const resourceList = await rpc(endpoint, token, { jsonrpc: '2.0', id: 32, method: 'resources/list', params: {} });
  const releaseResource = resourceList.result.resources.find(resource => resource.uri === 'skill://coding-tools/repo/release-helper');
  assert.ok(releaseResource);
  const evolutionResource = resourceList.result.resources.find(resource => resource.uri === evolutionUri);
  assert.ok(evolutionResource);
  assert.equal(evolutionResource.mimeType, 'application/json');
  assert.equal(evolutionResource._meta['coding-tools/resource-kind'], 'tool-evolution-proposal');
  const loadedResource = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 33, method: 'resources/read', params: { uri: 'skill://coding-tools/repo/release-helper' }
  });
  assert.match(loadedResource.result.contents[0].text, /^---\nname: release-helper/m);
  const loadedEvolution = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 35, method: 'resources/read', params: { uri: evolutionUri }
  });
  const evolutionDocument = JSON.parse(loadedEvolution.result.contents[0].text);
  assert.equal(evolutionDocument.kind, 'tool_evolution_proposal');
  assert.equal(evolutionDocument.proposal.tool, 'search_text');
  assert.equal(
    evolutionDocument.implementation_contract.required_commit_trailer,
    `Tool-Evolution-Proposal: ${evolutionProposal.proposalId}`
  );
  assert.equal(JSON.stringify(evolutionDocument).includes('http-resource-1'), false);
  const listedResponse = await rpcResponse(endpoint, token, { jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} });
  const listedTools = listedResponse.body;
  assert.equal(listedTools.result.tools.length, toolsForProfile('advanced').length + 1);
  const externalTool = listedTools.result.tools.find(tool => tool.name.includes('__fixture__echo'));
  assert.ok(externalTool);
  const externalCall = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 34, method: 'tools/call',
    params: {
      name: externalTool.name,
      arguments: { message: 'server-proxy' },
      _meta: { 'coding-tools/toolset-revision': listedTools.result.toolsetRevision }
    }
  });
  assert.equal(externalCall.result.content[0].text, 'fixture:server-proxy');
  assert.deepEqual(externalCall.result.structuredContent, { echoed: 'server-proxy' });
  assert.equal(listedResponse.response.headers.get('x-coding-tools-toolset-revision'), listedTools.result.toolsetRevision);
  assert.equal(listedResponse.response.headers.get('x-coding-tools-agent-version'), initialized.result.serverInfo.version);
  assert.ok(Number(listedResponse.response.headers.get('x-coding-tools-runtime-started-at')) > 0);

  const staleCatalog = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 22, method: 'tools/call',
    params: {
      name: 'list_workspace_folders',
      arguments: {},
      _meta: { 'coding-tools/toolset-revision': 'stale-revision' }
    }
  });
  assert.equal(staleCatalog.result, undefined);
  assert.equal(staleCatalog.error.code, -32602);
  assert.equal(staleCatalog.error.data.error_code, 'TOOLSET_REVISION_MISMATCH');
  assert.equal(staleCatalog.error.data.reason, 'stale_tool_catalog');
  assert.equal(staleCatalog.error.data.toolset_revision, listedTools.result.toolsetRevision);

  const missingMetaList = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 20, method: 'tools/call',
    params: { name: 'list_workspace_folders', arguments: {} }
  });
  assert.equal(missingMetaList.result.structuredContent.selected_folder_id, null);
  assert.equal(missingMetaList.result.structuredContent.selection_scope, 'unselected');
  assert.equal(missingMetaList.result.structuredContent.conversation_source, 'missing_mcp_conversation');
  const missingMetaSwitch = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 21, method: 'tools/call',
    params: { name: 'switch_workspace_folder', arguments: { folder_id: 'repo' } }
  });
  assert.equal(missingMetaSwitch.result.isError, true);
  assert.equal(missingMetaSwitch.result.structuredContent.error.code, 'WORKSPACE_FOLDER_NOT_SELECTED');

  const meta = { 'openai/session': 'integration' };
  const bootstrapped = await rpc(endpoint, token, { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'conversation_bootstrap', arguments: {}, _meta: meta } });
  assert.equal(bootstrapped.result.structuredContent.ok, true);
  assert.equal(bootstrapped.result.structuredContent.selected_folder_id, 'repo');
  assert.equal(bootstrapped.result.structuredContent.response_mode, 'compact');
  assert.equal(bootstrapped.result.structuredContent.needs_folder_selection, false);
  const releaseSkill = bootstrapped.result.structuredContent.project_skills.skills.find(skill => skill.name === 'release-helper');
  assert.ok(releaseSkill);
  assert.equal(releaseSkill.source, 'project');
  assert.equal(releaseSkill.scope, 'workspace');
  assert.ok(bootstrapped.result.structuredContent.project_skills.skillset_revision);
  assert.equal(bootstrapped.result.structuredContent.startup_flow, 'workspace_and_history_bootstrapped');
  assert.ok(bootstrapped.result.structuredContent.current_path);

  const hookBlocked = await rpc(endpoint, token, {
    jsonrpc: '2.0', id: 35, method: 'tools/call',
    params: {
      name: 'set_default_cwd',
      arguments: { path: '.' },
      _meta: { ...meta, 'coding-tools/toolset-revision': listedTools.result.toolsetRevision }
    }
  });
  assert.equal(hookBlocked.error, undefined, JSON.stringify(hookBlocked));
  assert.equal(hookBlocked.result.isError, true);
  assert.equal(hookBlocked.result.structuredContent.error.code, 'HOOK_BLOCKED');
  assert.match(hookBlocked.result.structuredContent.error.message, /blocked-by-extension-fixture/);

  const read = await rpc(endpoint, token, { jsonrpc: '2.0', id: 5, method: 'tools/call', params: { name: 'read_file', arguments: { path: 'hello.txt' }, _meta: meta } });
  assert.equal(read.result.content.length, 1);
  assert.equal(read.result.content[0].type, 'text');
  assert.ok(Buffer.byteLength(read.result.content[0].text) < 128);
  assert.doesNotMatch(read.result.content[0].text, /hello node agent/);
  assert.equal(read.result.structuredContent.content, 'hello node agent');

  const viewed = await rpc(endpoint, token, { jsonrpc: '2.0', id: 6, method: 'tools/call', params: { name: 'view_image', arguments: { path: 'pixel.bin' }, _meta: meta } });
  assert.equal(viewed.result.content.length, 1);
  assert.equal(viewed.result.content[0].type, 'image');
  assert.equal(viewed.result.content[0].mimeType, 'image/png');
  assert.ok(viewed.result.content[0].data.length > 0);
  assert.equal(viewed.result.structuredContent.mime_type, 'image/png');
  assert.deepEqual([viewed.result.structuredContent.width, viewed.result.structuredContent.height], [2, 1]);
  assert.equal(viewed.result.structuredContent.content, undefined);
  assert.equal(viewed.result.structuredContent.base64, undefined);
  assert.equal(viewed.result.structuredContent.data_url, undefined);

  const largeRead = await rpc(endpoint, token, { jsonrpc: '2.0', id: 7, method: 'tools/call', params: { name: 'read_file', arguments: { path: 'large.txt' }, _meta: meta } });
  assert.equal(largeRead.result.structuredContent.content, largePayload);
  assert.ok(Buffer.byteLength(largeRead.result.content[0].text) < 128);
  assert.doesNotMatch(largeRead.result.content[0].text, /HTTP_LARGE_MARKER/);
  const largeSerialized = JSON.stringify(largeRead.result);
  assert.equal(largeSerialized.split('HTTP_LARGE_MARKER:').length - 1, 1);
  assert.ok(
    Buffer.byteLength(largeSerialized) <= Buffer.byteLength(JSON.stringify(largeRead.result.structuredContent)) + 1024
  );

  const missing = await rpc(endpoint, token, { jsonrpc: '2.0', id: 8, method: 'tools/call', params: { name: 'read_file', arguments: { path: 'missing.txt' }, _meta: meta } });
  const missingStructured = missing.result.structuredContent;
  assert.equal(missing.result.isError, true);
  assert.equal(missingStructured.ok, false);
  assert.equal(missingStructured.status, 'error');
  assert.equal(missingStructured.summary, missingStructured.error.message);
  assert.equal(missingStructured.error.code, 'NOT_FOUND');
  assert.equal(missingStructured.error.category, 'not_found');
  assert.equal(missingStructured.error.retryable, false);
  assert.deepEqual(missingStructured.error.details, {});
  assert.equal(missing.result.content[0].text, missingStructured.error.message);

  const secretRead = await rpc(endpoint, token, { jsonrpc: '2.0', id: 9, method: 'tools/call', params: { name: 'read_file', arguments: { path: '.env' }, _meta: meta } });
  assert.equal(secretRead.result.structuredContent.content, '[REDACTED]');
  assert.equal(secretRead.result.structuredContent.sensitive_data_redacted, true);
  assert.doesNotMatch(JSON.stringify(secretRead), new RegExp(httpSecret));

  const unknown = await rpc(endpoint, token, { jsonrpc: '2.0', id: 10, method: 'tools/call', params: { name: 'missing_tool', arguments: {}, _meta: meta } });
  assert.equal(unknown.result, undefined);
  assert.equal(unknown.error.code, -32602);
  assert.equal(unknown.error.data.error_code, 'UNKNOWN_TOOL');
  assert.equal(unknown.error.data.reason, 'unknown_tool');

  const assignedPrefix = '/builtin/clients/server-assigned';
  const assignedPublicBaseUrl = `https://public.example${assignedPrefix}`;
  config.publicBaseUrl = assignedPublicBaseUrl;
  const reassignedHealth = await fetch(`${localBase}${assignedPrefix}/health`).then(response => response.json());
  assert.equal(reassignedHealth.clientCompatVersion, CLIENT_COMPAT_VERSION);
  const reassignedMetadata = await fetch(`${localBase}/.well-known/oauth-authorization-server${assignedPrefix}`).then(response => response.json());
  assert.equal(reassignedMetadata.issuer, assignedPublicBaseUrl);
  assert.equal(reassignedMetadata.authorization_endpoint, `${assignedPublicBaseUrl}/oauth/authorize`);
});
