import { createHash } from 'node:crypto';
import { lstat, readFile, readdir, realpath } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import { parseSkillMarkdown } from './parser.js';
import { applyPromotedEvolvedSkills } from '../knowledge/evolvedSkills.js';
import type { EvolvedSkillRecord } from '../knowledge/types.js';
import type {
  SkillDescriptor,
  SkillDiagnostic,
  SkillInventorySnapshot,
  SkillScope,
  SkillSnapshot,
  SkillSource
} from './types.js';

const MAX_SKILL_BYTES = 256 * 1024;
const MAX_SKILL_FILES = 256;

interface DiscoveryRoot {
  discoveryRoot: string;
  containmentRoot: string;
  displayPrefix?: string;
  source: SkillSource;
  scope: SkillScope;
  precedence: number;
  maxDepth: number;
}

interface WorkspaceDiscoveryRoot {
  relative: string;
  source: SkillSource;
  precedence: number;
  maxDepth: number;
}

interface UserDiscoveryRoot extends WorkspaceDiscoveryRoot {
  displayPrefix: string;
}

export interface SkillRegistryOptions {
  /** Override the OS home directory for deterministic tests. Use null to disable user-level discovery. */
  homeDir?: string | null;
  /** Stable folder identity used to keep same-path workspace Skills independently controllable. */
  workspaceKey?: string;
  /** Master switch for exposing Skills from this workspace profile. */
  active?: boolean;
  /** Skill control keys disabled for this workspace profile. */
  disabledSkillKeys?: readonly string[];
  /** Promoted machine-evolved guidance. It may augment discovered Skills but never create or enable one. */
  evolvedSkillProvider?: () => Promise<readonly EvolvedSkillRecord[]>;
}

const WORKSPACE_DISCOVERY_ROOTS: readonly WorkspaceDiscoveryRoot[] = [
  { relative: 'skills', source: 'project', precedence: 0, maxDepth: 3 },
  { relative: '.agents/skills', source: 'agents', precedence: 10, maxDepth: 5 },
  { relative: '.claude/skills', source: 'claude', precedence: 20, maxDepth: 7 }
];
const USER_DISCOVERY_ROOTS: readonly UserDiscoveryRoot[] = [
  { relative: '.agents/skills', displayPrefix: '~/.agents/skills', source: 'codex-user', precedence: 100, maxDepth: 5 },
  { relative: '.claude/skills', displayPrefix: '~/.claude/skills', source: 'claude-user', precedence: 110, maxDepth: 7 }
];

function posixRelative(root: string, candidate: string): string {
  return path.relative(root, candidate).split(path.sep).join('/');
}

function inside(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

function displayPath(root: DiscoveryRoot, candidate: string): string {
  if (root.scope === 'user') {
    const relative = posixRelative(root.discoveryRoot, candidate);
    return relative ? `${root.displayPrefix}/${relative}` : String(root.displayPrefix);
  }
  return posixRelative(root.containmentRoot, candidate);
}

function controlKey(
  scope: SkillScope,
  source: SkillSource,
  relativePath: string,
  workspaceKey: string
): string {
  return scope === 'user'
    ? `user:${source}:${relativePath}`
    : `workspace:${workspaceKey}:${source}:${relativePath}`;
}

async function readVersion(skillRoot: string, containmentRealRoot: string): Promise<string | undefined> {
  const versionPath = path.join(skillRoot, 'VERSION');
  try {
    const info = await lstat(versionPath);
    if (!info.isFile() || info.isSymbolicLink() || info.size > 256) return undefined;
    const resolved = await realpath(versionPath);
    if (!inside(containmentRealRoot, resolved)) return undefined;
    const version = (await readFile(resolved, 'utf8')).trim();
    return version || undefined;
  } catch {
    return undefined;
  }
}

async function collectSkillFiles(root: DiscoveryRoot, diagnostics: SkillDiagnostic[]): Promise<string[]> {
  const files: string[] = [];
  const visit = async (directory: string, depth: number): Promise<void> => {
    if (depth > root.maxDepth || files.length >= MAX_SKILL_FILES) return;
    let entries;
    try {
      entries = await readdir(directory, { withFileTypes: true });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
        diagnostics.push({
          code: 'SKILL_DISCOVERY_FAILED',
          message: error instanceof Error ? error.message : String(error),
          path: displayPath(root, directory),
          source: root.source,
          scope: root.scope
        });
      }
      return;
    }
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      if (files.length >= MAX_SKILL_FILES) return;
      const full = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) {
        diagnostics.push({
          code: 'SKILL_SYMLINK_SKIPPED',
          message: 'Skill discovery does not follow symlinks.',
          path: displayPath(root, full),
          source: root.source,
          scope: root.scope
        });
        continue;
      }
      if (entry.isDirectory()) await visit(full, depth + 1);
      else if (entry.isFile() && entry.name.toLowerCase() === 'skill.md') files.push(full);
    }
  };
  await visit(root.discoveryRoot, 0);
  return files;
}

interface DiscoverySnapshot {
  skills: SkillDescriptor[];
  diagnostics: SkillDiagnostic[];
  scannedAtMs: number;
}

export class SkillRegistry {
  private active: boolean;
  private disabledSkillKeys: Set<string>;

  constructor(readonly workspaceRoot: string, private readonly options: SkillRegistryOptions = {}) {
    this.active = options.active ?? true;
    this.disabledSkillKeys = new Set(options.disabledSkillKeys ?? []);
  }

  setActive(active: boolean): void {
    this.active = active;
  }

  setDisabledSkillKeys(keys: readonly string[]): void {
    this.disabledSkillKeys = new Set(keys);
  }

  private async discoveryRoots(workspaceRealRoot: string, diagnostics: SkillDiagnostic[]): Promise<DiscoveryRoot[]> {
    const roots: DiscoveryRoot[] = WORKSPACE_DISCOVERY_ROOTS.map(root => ({
      discoveryRoot: path.join(this.workspaceRoot, root.relative),
      containmentRoot: workspaceRealRoot,
      source: root.source,
      scope: 'workspace',
      precedence: root.precedence,
      maxDepth: root.maxDepth
    }));
    if (this.options.homeDir === null) return roots;
    const home = String(this.options.homeDir ?? homedir()).trim();
    if (!home) return roots;
    try {
      const homeRealRoot = await realpath(home);
      for (const root of USER_DISCOVERY_ROOTS) {
        roots.push({
          discoveryRoot: path.join(home, root.relative),
          containmentRoot: homeRealRoot,
          displayPrefix: root.displayPrefix,
          source: root.source,
          scope: 'user',
          precedence: root.precedence,
          maxDepth: root.maxDepth
        });
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
        diagnostics.push({
          code: 'SKILL_USER_HOME_FAILED',
          message: error instanceof Error ? error.message : String(error),
          path: '~',
          scope: 'user'
        });
      }
    }
    return roots;
  }

  private async discover(): Promise<DiscoverySnapshot> {
    const diagnostics: SkillDiagnostic[] = [];
    const workspaceRealRoot = await realpath(this.workspaceRoot);
    const discoveryRoots = await this.discoveryRoots(workspaceRealRoot, diagnostics);
    const candidates: SkillDescriptor[] = [];
    let remainingSkillFiles = MAX_SKILL_FILES;
    const workspaceKey = this.options.workspaceKey?.trim() || 'workspace';

    for (const root of discoveryRoots) {
      if (remainingSkillFiles <= 0) {
        diagnostics.push({
          code: 'SKILL_DISCOVERY_LIMIT_REACHED',
          message: `Skill discovery is limited to ${MAX_SKILL_FILES} SKILL.md files per workspace.`
        });
        break;
      }
      const files = (await collectSkillFiles(root, diagnostics)).slice(0, remainingSkillFiles);
      remainingSkillFiles -= files.length;
      for (const file of files) {
        try {
          const info = await lstat(file);
          if (!info.isFile() || info.isSymbolicLink()) continue;
          if (info.size > MAX_SKILL_BYTES) {
            diagnostics.push({
              code: 'SKILL_TOO_LARGE',
              message: `SKILL.md exceeds ${MAX_SKILL_BYTES} bytes.`,
              path: displayPath(root, file),
              source: root.source,
              scope: root.scope
            });
            continue;
          }
          const resolved = await realpath(file);
          if (!inside(root.containmentRoot, resolved)) {
            diagnostics.push({
              code: root.scope === 'workspace' ? 'SKILL_OUTSIDE_WORKSPACE' : 'SKILL_OUTSIDE_USER_HOME',
              message: root.scope === 'workspace'
                ? 'Resolved SKILL.md escapes the configured workspace.'
                : 'Resolved user-level SKILL.md escapes the configured user home.',
              path: displayPath(root, file),
              source: root.source,
              scope: root.scope
            });
            continue;
          }
          const content = await readFile(resolved, 'utf8');
          const parsed = parseSkillMarkdown(content);
          const skillRoot = path.dirname(resolved);
          const relativePath = displayPath(root, file);
          candidates.push({
            key: controlKey(root.scope, root.source, relativePath, workspaceKey),
            name: parsed.name,
            description: parsed.description,
            source: root.source,
            scope: root.scope,
            precedence: root.precedence,
            entrypoint: resolved,
            relativePath,
            root: skillRoot,
            rootRelativePath: displayPath(root, path.dirname(file)),
            body: parsed.body,
            content,
            contentSha256: createHash('sha256').update(content).digest('hex'),
            version: await readVersion(skillRoot, root.containmentRoot),
            sizeBytes: info.size
          });
        } catch (error) {
          diagnostics.push({
            code: 'SKILL_INVALID',
            message: error instanceof Error ? error.message : String(error),
            path: displayPath(root, file),
            source: root.source,
            scope: root.scope
          });
        }
      }
    }

    candidates.sort((left, right) =>
      left.precedence - right.precedence
      || left.relativePath.localeCompare(right.relativePath)
      || left.name.localeCompare(right.name)
    );
    const selected = new Map<string, SkillDescriptor>();
    for (const skill of candidates) {
      const nameKey = skill.name.toLocaleLowerCase('en-US');
      const existing = selected.get(nameKey);
      if (!existing) {
        selected.set(nameKey, skill);
        continue;
      }
      diagnostics.push({
        code: 'SKILL_SHADOWED',
        message: `${skill.relativePath} is shadowed by ${existing.relativePath}.`,
        path: skill.relativePath,
        name: skill.name,
        source: skill.source,
        scope: skill.scope
      });
    }
    let skills = [...selected.values()].sort((left, right) => left.name.localeCompare(right.name));
    if (this.options.evolvedSkillProvider) {
      try {
        const effective = applyPromotedEvolvedSkills(skills, await this.options.evolvedSkillProvider());
        skills = effective.skills;
        diagnostics.push(...effective.diagnostics);
      } catch (error) {
        diagnostics.push({
          code: 'EVOLVED_SKILL_LOAD_FAILED',
          message: error instanceof Error ? error.message : String(error)
        });
      }
    }
    return { skills, diagnostics, scannedAtMs: Date.now() };
  }

  async inventory(): Promise<SkillInventorySnapshot> {
    const discovered = await this.discover();
    return {
      skills: discovered.skills.map(skill => {
        const selected = !this.disabledSkillKeys.has(skill.key);
        return {
          skill,
          selected,
          enabled: this.active && selected
        };
      }),
      diagnostics: discovered.diagnostics,
      scannedAtMs: discovered.scannedAtMs
    };
  }

  async snapshot(): Promise<SkillSnapshot> {
    const discovered = await this.discover();
    const skills = this.active
      ? discovered.skills.filter(skill => !this.disabledSkillKeys.has(skill.key))
      : [];
    const revisionMaterial = skills.map(skill => ({
      key: skill.key,
      name: skill.name,
      source: skill.source,
      scope: skill.scope,
      path: skill.relativePath,
      sha256: skill.contentSha256
    }));
    const revision = createHash('sha256').update(JSON.stringify(revisionMaterial)).digest('hex');
    return {
      skills,
      diagnostics: discovered.diagnostics,
      revision,
      scannedAtMs: discovered.scannedAtMs
    };
  }

  async read(name: string): Promise<{ snapshot: SkillSnapshot; skill?: SkillDescriptor }> {
    const snapshot = await this.snapshot();
    const normalized = name.trim().toLocaleLowerCase('en-US');
    return {
      snapshot,
      skill: snapshot.skills.find(candidate => candidate.name.toLocaleLowerCase('en-US') === normalized)
    };
  }
}
