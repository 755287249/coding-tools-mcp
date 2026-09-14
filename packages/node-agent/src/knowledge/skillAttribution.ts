import type { SkillDescriptor } from '../skills/types.js';

const MAX_ACTIVE_SKILL_CONTEXTS = 128;

export interface SkillCanaryAttribution {
  knowledgeId: string;
  implementation: string;
  eligible: true;
  applied: boolean;
  bucket: number;
}

export interface SkillLearningAttribution {
  source: string;
  name: string;
  contentSha256: string;
  generation: number;
  canary?: SkillCanaryAttribution;
}

const activeSkills = new WeakMap<object, Map<string, SkillLearningAttribution>>();

function attributionKey(conversationKey: string, folderId: string): string {
  return `${conversationKey}\0${folderId}`;
}

function boundedAttribution(skill: SkillDescriptor): SkillLearningAttribution {
  return {
    source: skill.source,
    name: skill.name.slice(0, 128),
    contentSha256: skill.evolution?.baseContentSha256 ?? skill.contentSha256,
    generation: skill.evolution?.generation ?? 0
  };
}

export function rememberSkillSelection(
  owner: object,
  conversationKey: string,
  folderId: string,
  skill: SkillDescriptor,
  canary?: SkillCanaryAttribution
): SkillLearningAttribution {
  let selections = activeSkills.get(owner);
  if (!selections) {
    selections = new Map();
    activeSkills.set(owner, selections);
  }
  const key = attributionKey(conversationKey, folderId);
  selections.delete(key);
  const attribution = { ...boundedAttribution(skill), ...(canary ? { canary: { ...canary } } : {}) };
  selections.set(key, attribution);
  while (selections.size > MAX_ACTIVE_SKILL_CONTEXTS) {
    const oldest = selections.keys().next().value;
    if (oldest === undefined) break;
    selections.delete(oldest);
  }
  return { ...attribution, ...(attribution.canary ? { canary: { ...attribution.canary } } : {}) };
}

export function selectedSkillAttribution(
  owner: object,
  conversationKey: string,
  folderId: string | undefined
): SkillLearningAttribution | undefined {
  if (!folderId) return undefined;
  const selections = activeSkills.get(owner);
  if (!selections) return undefined;
  const key = attributionKey(conversationKey, folderId);
  const attribution = selections.get(key);
  if (!attribution) return undefined;
  selections.delete(key);
  selections.set(key, attribution);
  return { ...attribution, ...(attribution.canary ? { canary: { ...attribution.canary } } : {}) };
}
