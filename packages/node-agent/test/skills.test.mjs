import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { parseSkillMarkdown } from '../dist/skills/parser.js';
import { SkillRegistry } from '../dist/skills/registry.js';
import { getSkillPrompt, readSkillResource } from '../dist/skills/mcp.js';
import { selectedSkillAttribution } from '../dist/knowledge/skillAttribution.js';
import { CanaryImpactStore, EvolvedSkillCanaryEngine, KnowledgeStore, knowledgeRecordId } from '../dist/knowledge/index.js';

async function writeSkill(root, relativeRoot, name, description, body = '# Skill') {
  const directory = path.join(root, relativeRoot);
  await mkdir(directory, { recursive: true });
  await writeFile(path.join(directory, 'SKILL.md'), `---\nname: ${name}\ndescription: ${JSON.stringify(description)}\n---\n\n${body}\n`);
}

test('SKILL.md parser accepts quoted metadata and requires name and description', () => {
  const parsed = parseSkillMarkdown('---\nname: demo\ndescription: "Quoted description"\n---\n\n# Demo\n');
  assert.equal(parsed.name, 'demo');
  assert.equal(parsed.description, 'Quoted description');
  assert.equal(parsed.body, '# Demo\n');
  assert.throws(() => parseSkillMarkdown('---\nname: demo\n---\n'), /requires description/);
});

test('project Skill registry discovers compatibility roots with deterministic precedence', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-'));
  await writeSkill(root, path.join('.claude', 'skills', 'shared'), 'shared', 'Claude compatibility copy');
  await writeSkill(root, path.join('.agents', 'skills', 'shared'), 'shared', 'Agents compatibility copy');
  await writeSkill(root, path.join('skills', 'shared'), 'shared', 'Canonical project copy');
  await writeSkill(root, path.join('.claude', 'skills', 'gitnexus', 'debugging'), 'gitnexus-debugging', 'Debug with GitNexus');
  await writeFile(path.join(root, 'skills', 'shared', 'VERSION'), '2.3.4\n');

  const snapshot = await new SkillRegistry(root, { homeDir: null }).snapshot();
  assert.deepEqual(snapshot.skills.map(skill => skill.name), ['gitnexus-debugging', 'shared']);
  const shared = snapshot.skills.find(skill => skill.name === 'shared');
  assert.equal(shared.source, 'project');
  assert.equal(shared.scope, 'workspace');
  assert.equal(shared.description, 'Canonical project copy');
  assert.equal(shared.version, '2.3.4');
  assert.equal(snapshot.diagnostics.filter(item => item.code === 'SKILL_SHADOWED').length, 2);
  assert.match(snapshot.revision, /^[a-f0-9]{64}$/);
});

test('Skill registry discovers Codex and Claude Code user Skills and filters controls after shadowing', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-workspace-'));
  const home = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-home-'));
  await writeSkill(home, path.join('.agents', 'skills', 'user-only'), 'user-only', 'Personal Codex workflow');
  await writeSkill(home, path.join('.agents', 'skills', 'shared'), 'shared', 'Personal shared workflow');
  await writeSkill(home, path.join('.claude', 'skills', 'claude-only'), 'claude-only', 'Personal Claude workflow');
  await writeSkill(root, path.join('skills', 'shared'), 'shared', 'Workspace shared workflow');

  const registry = new SkillRegistry(root, { homeDir: home, workspaceKey: 'repo' });
  const inventory = await registry.inventory();
  assert.deepEqual(inventory.skills.map(item => item.skill.name), ['claude-only', 'shared', 'user-only']);
  const userOnly = inventory.skills.find(item => item.skill.name === 'user-only').skill;
  assert.equal(userOnly.source, 'codex-user');
  assert.equal(userOnly.scope, 'user');
  assert.equal(userOnly.relativePath, '~/.agents/skills/user-only/SKILL.md');
  assert.equal(userOnly.relativePath.includes(home), false);
  const claudeOnly = inventory.skills.find(item => item.skill.name === 'claude-only').skill;
  assert.equal(claudeOnly.source, 'claude-user');
  assert.equal(claudeOnly.scope, 'user');
  assert.equal(claudeOnly.relativePath, '~/.claude/skills/claude-only/SKILL.md');
  assert.equal(claudeOnly.relativePath.includes(home), false);

  const sharedItem = inventory.skills.find(item => item.skill.name === 'shared');
  assert.equal(sharedItem.skill.source, 'project');
  assert.equal(sharedItem.skill.scope, 'workspace');
  assert.equal(sharedItem.skill.description, 'Workspace shared workflow');
  assert.match(sharedItem.skill.key, /^workspace:repo:project:/);
  const before = await registry.snapshot();
  registry.setDisabledSkillKeys([sharedItem.skill.key]);
  const disabledInventory = await registry.inventory();
  assert.equal(disabledInventory.skills.find(item => item.skill.name === 'shared').enabled, false);
  const after = await registry.snapshot();
  assert.deepEqual(after.skills.map(skill => skill.name), ['claude-only', 'user-only']);
  assert.equal(after.skills.some(skill => skill.name === 'shared'), false);
  assert.notEqual(after.revision, before.revision);

  registry.setActive(false);
  const masterDisabledInventory = await registry.inventory();
  assert.equal(masterDisabledInventory.skills.find(item => item.skill.name === 'user-only').selected, true);
  assert.equal(masterDisabledInventory.skills.find(item => item.skill.name === 'user-only').enabled, false);
  assert.equal(masterDisabledInventory.skills.find(item => item.skill.name === 'shared').selected, false);
  const masterDisabled = await registry.snapshot();
  assert.deepEqual(masterDisabled.skills, []);

  registry.setActive(true);
  const restored = await registry.snapshot();
  assert.deepEqual(restored.skills.map(skill => skill.name), ['claude-only', 'user-only']);

  const shadowed = after.diagnostics.find(item => item.code === 'SKILL_SHADOWED' && item.name === 'shared');
  assert.equal(shadowed.source, 'codex-user');
  assert.equal(shadowed.scope, 'user');
  assert.equal(shadowed.path, '~/.agents/skills/shared/SKILL.md');
});

test('promoted evolved Skill guidance is hash-bound, fail-closed, and preserves base controls', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-evolved-'));
  await writeSkill(root, path.join('skills', 'demo'), 'demo', 'Demo workflow', 'Base workflow');
  const skillPath = path.join(root, 'skills', 'demo', 'SKILL.md');
  const diskBefore = await readFile(skillPath, 'utf8');
  const base = (await new SkillRegistry(root, { homeDir: null }).snapshot()).skills[0];
  const evidence = {
    observations: 30, successes: 30, failures: 0, totalDurationMs: 100, totalRequestBytes: 0, totalResponseBytes: 0,
    firstSeenAtMs: 1, lastSeenAtMs: 2
  };
  const promoted = {
    knowledgeId: 'evolved-demo-1', name: 'demo', status: 'promoted', confidence: 0.95,
    base: { source: base.source, name: base.name, contentSha256: base.contentSha256 },
    generation: 1, overlay: { append_guidance: ['Re-read the current file before rebuilding a stale guarded edit.'] }, evidence
  };
  const registry = new SkillRegistry(root, { homeDir: null, evolvedSkillProvider: async () => [promoted] });
  const effective = await registry.snapshot();
  assert.match(effective.skills[0].body, /## Learned guidance/);
  assert.match(effective.skills[0].body, /Re-read the current file/);
  assert.deepEqual(effective.skills[0].evolution, {
    knowledgeId: promoted.knowledgeId, generation: 1, baseContentSha256: base.contentSha256
  });
  assert.equal(await readFile(skillPath, 'utf8'), diskBefore);

  registry.setDisabledSkillKeys([base.key]);
  assert.deepEqual((await registry.snapshot()).skills, []);
  registry.setDisabledSkillKeys([]);

  const stale = new SkillRegistry(root, {
    homeDir: null,
    evolvedSkillProvider: async () => [{ ...promoted, base: { ...promoted.base, contentSha256: '0'.repeat(64) } }]
  });
  const staleSnapshot = await stale.snapshot();
  assert.doesNotMatch(staleSnapshot.skills[0].body, /## Learned guidance/);
  assert.equal(staleSnapshot.diagnostics.some(item => item.code === 'EVOLVED_SKILL_BASE_CHANGED'), true);

  const invalid = new SkillRegistry(root, {
    homeDir: null,
    evolvedSkillProvider: async () => [{ ...promoted, overlay: { replace_body: 'unsafe replacement' } }]
  });
  const invalidSnapshot = await invalid.snapshot();
  assert.doesNotMatch(invalidSnapshot.skills[0].body, /## Learned guidance/);
  assert.equal(invalidSnapshot.diagnostics.some(item => item.code === 'EVOLVED_SKILL_INVALID_OVERLAY'), true);

  const candidate = new SkillRegistry(root, {
    homeDir: null,
    evolvedSkillProvider: async () => [{ ...promoted, status: 'candidate' }]
  });
  assert.doesNotMatch((await candidate.snapshot()).skills[0].body, /## Learned guidance/);
});

test('validated evolved Skill guidance is served only to deterministic prompt/resource canary cohorts', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-canary-'));
  const dataDir = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-canary-data-'));
  await writeSkill(root, path.join('skills', 'demo'), 'demo', 'Demo workflow', 'Base workflow');
  const registry = new SkillRegistry(root, { homeDir: null, workspaceKey: 'repo' });
  const base = (await registry.snapshot()).skills[0];
  const store = new KnowledgeStore(dataDir);
  const impactStore = new CanaryImpactStore(dataDir);
  const engine = new EvolvedSkillCanaryEngine(store, impactStore);
  const knowledge = {
    id: '', schemaVersion: 1, scope: 'workspace', target: 'evolved_skill', status: 'validated',
    trigger: { tool: 'edit', pattern: 'stale_guarded_edit_recovery' },
    hypothesis: 'Serve validated local guidance to a session canary before promotion.',
    recommendedAction: { append_guidance: ['Re-read the current file before rebuilding a stale guarded edit.'] },
    evidence: { observations: 15, successes: 0, failures: 15, totalDurationMs: 1000, totalRequestBytes: 0, totalResponseBytes: 0, firstSeenAtMs: 1, lastSeenAtMs: 15 },
    confidence: 0.9, sourceEventIds: ['evolved-skill-canary'], counterexampleEventIds: [], createdAtMs: 1, updatedAtMs: 15,
    metadata: { name: base.name, base: { source: base.source, name: base.name, contentSha256: base.contentSha256 }, generation: 1 }
  };
  knowledge.id = knowledgeRecordId(knowledge);
  await store.upsert(knowledge);
  const conversations = {};
  const ctx = {
    config: { folders: [{ id: 'repo', name: 'Repo' }] },
    folderRuntimes: new Map([['repo', { skillRegistry: registry, evolvedSkillCanaryEngine: engine }]]),
    conversations
  };
  let interventionSession;
  let controlSession;
  for (let index = 0; index < 100 && (!interventionSession || !controlSession); index += 1) {
    const session = `skill-canary-session-${index}`;
    const decision = await engine.decide(base, session);
    if (decision?.applied && !interventionSession) interventionSession = session;
    if (decision && !decision.applied && !controlSession) controlSession = session;
  }
  assert.ok(interventionSession);
  assert.ok(controlSession);
  const intervention = await getSkillPrompt(ctx, 'project-skill/repo/demo', interventionSession);
  const control = await getSkillPrompt(ctx, 'project-skill/repo/demo', controlSession);
  assert.match(intervention.messages[0].content.text, /## Learned guidance/);
  assert.match(intervention.messages[0].content.text, /Re-read the current file/);
  assert.doesNotMatch(control.messages[0].content.text, /## Learned guidance/);
  const resource = await readSkillResource(ctx, 'skill://coding-tools/repo/demo', interventionSession);
  assert.match(resource.contents[0].text, /## Learned guidance/);
  const interventionAttribution = selectedSkillAttribution(conversations, interventionSession, 'repo');
  const controlAttribution = selectedSkillAttribution(conversations, controlSession, 'repo');
  assert.equal(interventionAttribution.canary.knowledgeId, knowledge.id);
  assert.equal(interventionAttribution.canary.applied, true);
  assert.equal(controlAttribution.canary.knowledgeId, knowledge.id);
  assert.equal(controlAttribution.canary.applied, false);
  assert.doesNotMatch((await registry.snapshot()).skills[0].body, /## Learned guidance/, 'validated canary must not alter list/bootstrap snapshots');
});
test('Skill registry revision changes when project instructions change', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'ctmcp-skills-revision-'));
  await writeSkill(root, path.join('skills', 'demo'), 'demo', 'First description', 'First body');
  const registry = new SkillRegistry(root, { homeDir: null });
  const first = await registry.snapshot();
  await writeSkill(root, path.join('skills', 'demo'), 'demo', 'Second description', 'Second body');
  const second = await registry.snapshot();
  assert.notEqual(first.revision, second.revision);
  assert.equal(second.skills[0].description, 'Second description');
});
