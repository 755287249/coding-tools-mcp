import { createHash } from 'node:crypto';
import type { SkillDescriptor, SkillDiagnostic } from '../skills/types.js';
import type { EvolvedSkillRecord } from './types.js';

const MAX_GUIDANCE_ITEMS = 16;
const MAX_GUIDANCE_ITEM_BYTES = 1_000;
const MAX_GUIDANCE_TOTAL_BYTES = 8 * 1024;
const ALLOWED_OVERLAY_KEYS = new Set(['append_guidance']);

export interface EffectiveEvolvedSkills {
  skills: SkillDescriptor[];
  diagnostics: SkillDiagnostic[];
}

function sameBaseIdentity(skill: SkillDescriptor, record: EvolvedSkillRecord): boolean {
  return record.base.source === skill.source
    && record.base.name.toLocaleLowerCase('en-US') === skill.name.toLocaleLowerCase('en-US');
}

function normalizedGuidance(record: EvolvedSkillRecord): string[] {
  const keys = Object.keys(record.overlay);
  if (!keys.length || keys.some(key => !ALLOWED_OVERLAY_KEYS.has(key))) {
    throw new Error('Only append_guidance is allowed in an evolved Skill overlay.');
  }
  const raw = record.overlay.append_guidance;
  if (!Array.isArray(raw) || raw.length < 1 || raw.length > MAX_GUIDANCE_ITEMS) {
    throw new Error(`append_guidance must contain between 1 and ${MAX_GUIDANCE_ITEMS} items.`);
  }
  let totalBytes = 0;
  return raw.map((item, index) => {
    if (typeof item !== 'string') throw new Error(`append_guidance[${index}] must be a string.`);
    const value = item.trim();
    const bytes = Buffer.byteLength(value);
    if (!value || value.includes('\0') || value.includes('\n') || value.includes('\r') || bytes > MAX_GUIDANCE_ITEM_BYTES) {
      throw new Error(`append_guidance[${index}] must be a bounded single-line guidance item.`);
    }
    totalBytes += bytes;
    if (totalBytes > MAX_GUIDANCE_TOTAL_BYTES) {
      throw new Error(`append_guidance exceeds ${MAX_GUIDANCE_TOTAL_BYTES} total bytes.`);
    }
    return value;
  });
}

function applyGuidance(skill: SkillDescriptor, record: EvolvedSkillRecord, guidance: readonly string[]): SkillDescriptor {
  const eol = skill.content.includes('\r\n') ? '\r\n' : '\n';
  const novelGuidance = guidance.filter(item => !skill.body.includes(`- ${item}`));
  const section = novelGuidance.length ? [
    '## Learned guidance',
    '',
    'The following guidance was promoted from validated local workflow evidence. It remains subordinate to runtime permissions, workspace policy, and sandbox/security boundaries.',
    '',
    ...novelGuidance.map(item => `- ${item}`)
  ].join(eol) : '';
  const content = section
    ? `${skill.content.replace(/[\r\n]+$/u, '')}${eol}${eol}${section}${eol}`
    : skill.content;
  const body = section
    ? `${skill.body.replace(/[\r\n]+$/u, '')}${eol}${eol}${section}${eol}`
    : skill.body;
  return {
    ...skill,
    body,
    content,
    contentSha256: createHash('sha256').update(content).digest('hex'),
    sizeBytes: Buffer.byteLength(content),
    evolution: {
      knowledgeId: record.knowledgeId,
      generation: record.generation,
      baseContentSha256: record.base.contentSha256
    }
  };
}

export function applyEvolvedSkillRecord(skill: SkillDescriptor, record: EvolvedSkillRecord): SkillDescriptor {
  if (!sameBaseIdentity(skill, record)) throw new Error('Evolved Skill base identity does not match the selected Skill.');
  const baseSha = skill.evolution?.baseContentSha256 ?? skill.contentSha256;
  if (record.base.contentSha256 !== baseSha) throw new Error('Evolved Skill base content changed.');
  return applyGuidance(skill, record, normalizedGuidance(record));
}

export function applyPromotedEvolvedSkills(
  baseSkills: readonly SkillDescriptor[],
  records: readonly EvolvedSkillRecord[]
): EffectiveEvolvedSkills {
  const diagnostics: SkillDiagnostic[] = [];
  const promoted = records.filter(record => record.status === 'promoted');
  const matchedIdentities = new Set<string>();
  const skills = baseSkills.map(skill => {
    const identityMatches = promoted.filter(record => sameBaseIdentity(skill, record));
    if (!identityMatches.length) return skill;
    const identityKey = `${skill.source}\0${skill.name.toLocaleLowerCase('en-US')}`;
    matchedIdentities.add(identityKey);
    const current = identityMatches.filter(record => record.base.contentSha256 === skill.contentSha256);
    if (!current.length) {
      diagnostics.push({
        code: 'EVOLVED_SKILL_BASE_CHANGED',
        message: `Promoted evolved guidance for ${skill.name} was ignored because the base Skill content changed.`,
        name: skill.name,
        source: skill.source,
        scope: skill.scope,
        path: skill.relativePath
      });
      return skill;
    }
    const generation = Math.max(...current.map(record => record.generation));
    const latest = current.filter(record => record.generation === generation);
    if (latest.length !== 1) {
      diagnostics.push({
        code: 'EVOLVED_SKILL_CONFLICT',
        message: `Multiple promoted evolved Skill overlays claim generation ${generation} for ${skill.name}; all were ignored.`,
        name: skill.name,
        source: skill.source,
        scope: skill.scope,
        path: skill.relativePath
      });
      return skill;
    }
    try {
      const guidance = normalizedGuidance(latest[0]!);
      return applyGuidance(skill, latest[0]!, guidance);
    } catch (error) {
      diagnostics.push({
        code: 'EVOLVED_SKILL_INVALID_OVERLAY',
        message: error instanceof Error ? error.message : String(error),
        name: skill.name,
        source: skill.source,
        scope: skill.scope,
        path: skill.relativePath
      });
      return skill;
    }
  });

  for (const record of promoted) {
    const identityKey = `${record.base.source}\0${record.base.name.toLocaleLowerCase('en-US')}`;
    if (matchedIdentities.has(identityKey)) continue;
    diagnostics.push({
      code: 'EVOLVED_SKILL_BASE_NOT_FOUND',
      message: `Promoted evolved guidance for ${record.base.name || record.name} was ignored because its base Skill is not available.`,
      name: record.base.name || record.name
    });
  }
  return { skills, diagnostics };
}
