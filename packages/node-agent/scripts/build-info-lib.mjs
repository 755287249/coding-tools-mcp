export const MAX_TOOL_EVOLUTION_PROPOSAL_IDS = 16;

const TOOL_EVOLUTION_PROPOSAL_ID = /^[0-9a-f]{64}$/i;
const TOOL_EVOLUTION_TRAILER = /^Tool-Evolution-Proposal:\s*([0-9a-f]{64})\s*$/i;

export function normalizeToolEvolutionProposalIds(values) {
  const normalized = new Set();
  for (const value of values ?? []) {
    const candidate = String(value ?? '').trim().toLowerCase();
    if (TOOL_EVOLUTION_PROPOSAL_ID.test(candidate)) normalized.add(candidate);
  }
  return [...normalized].sort().slice(0, MAX_TOOL_EVOLUTION_PROPOSAL_IDS);
}

export function toolEvolutionProposalIdsFromCommitMessage(message) {
  const ids = [];
  for (const line of String(message ?? '').split(/\r?\n/)) {
    const match = line.match(TOOL_EVOLUTION_TRAILER);
    if (match) ids.push(match[1]);
  }
  return normalizeToolEvolutionProposalIds(ids);
}

export function toolEvolutionProposalIdsFromEnvironment(value) {
  if (typeof value !== 'string' || value.trim().length === 0) return [];
  return normalizeToolEvolutionProposalIds(value.split(/[\s,]+/));
}
