import type {
  EvolvedSkillRecord,
  KnowledgeRecord,
  ToolEvolutionChangeProposalSummary,
  ToolEvolutionExperimentSummary,
  ToolEvolutionRecord,
  ToolStrategyRecord
} from './types.js';
import { KnowledgeStore } from './store.js';
import { ToolEvolutionExperimentStore, ToolEvolutionProposalStore } from './toolEvolution.js';

function toolName(record: KnowledgeRecord): string {
  return typeof record.trigger.tool === 'string' ? record.trigger.tool : '';
}

function proposalStatusRank(status: ToolEvolutionChangeProposalSummary['status']): number {
  if (status === 'materialized') return 3;
  if (status === 'proposed') return 2;
  if (status === 'superseded') return 1;
  return 0;
}

export class ToolStrategyRegistry {
  constructor(private readonly store: KnowledgeStore) {}

  async snapshot(): Promise<{ revision: string; strategies: ToolStrategyRecord[] }> {
    const records = (await this.store.list())
      .filter(record => record.target === 'tool_strategy')
      .map(record => ({
        knowledgeId: record.id,
        tool: toolName(record),
        status: record.status,
        confidence: record.confidence,
        trigger: structuredClone(record.trigger),
        action: structuredClone(record.recommendedAction),
        evidence: structuredClone(record.evidence)
      }))
      .sort((left, right) => left.knowledgeId.localeCompare(right.knowledgeId));
    return { revision: await this.store.revision(), strategies: records };
  }
}

export class ToolEvolutionRegistry {
  constructor(
    private readonly store: KnowledgeStore,
    private readonly experimentStore?: ToolEvolutionExperimentStore,
    private readonly proposalStore?: ToolEvolutionProposalStore
  ) {}

  async snapshot(): Promise<{
    revision: string;
    proposalRevision?: string;
    experimentRevision?: string;
    candidates: ToolEvolutionRecord[];
  }> {
    const [knowledgeRecords, revision, proposals, proposalRevision, experiments, experimentRevision] = await Promise.all([
      this.store.list(),
      this.store.revision(),
      this.proposalStore ? this.proposalStore.list() : Promise.resolve([]),
      this.proposalStore ? this.proposalStore.revision() : Promise.resolve(undefined),
      this.experimentStore ? this.experimentStore.list() : Promise.resolve([]),
      this.experimentStore ? this.experimentStore.revision() : Promise.resolve(undefined)
    ]);
    const latestProposal = new Map<string, ToolEvolutionChangeProposalSummary>();
    for (const proposal of proposals) {
      const summary: ToolEvolutionChangeProposalSummary = {
        proposalId: proposal.proposalId,
        status: proposal.status,
        baseRevision: proposal.baseRevision,
        updatedAtMs: proposal.updatedAtMs,
        ...(proposal.candidateRevision ? { candidateRevision: proposal.candidateRevision } : {}),
        ...(proposal.experimentId ? { experimentId: proposal.experimentId } : {})
      };
      const current = latestProposal.get(proposal.knowledgeId);
      if (!current || summary.updatedAtMs > current.updatedAtMs
        || (summary.updatedAtMs === current.updatedAtMs && proposalStatusRank(summary.status) > proposalStatusRank(current.status))
        || (summary.updatedAtMs === current.updatedAtMs
          && proposalStatusRank(summary.status) === proposalStatusRank(current.status)
          && summary.proposalId.localeCompare(current.proposalId) > 0)) {
        latestProposal.set(proposal.knowledgeId, summary);
      }
    }
    const latestExperiment = new Map<string, ToolEvolutionExperimentSummary>();
    for (const experiment of experiments) {
      const summary: ToolEvolutionExperimentSummary = {
        experimentId: experiment.experimentId,
        status: experiment.status,
        baseRevision: experiment.baseRevision,
        candidateRevision: experiment.candidateRevision,
        updatedAtMs: experiment.updatedAtMs,
        ...(experiment.decisionReason ? { decisionReason: experiment.decisionReason } : {})
      };
      const current = latestExperiment.get(experiment.knowledgeId);
      if (!current || summary.updatedAtMs > current.updatedAtMs
        || (summary.updatedAtMs === current.updatedAtMs && summary.experimentId.localeCompare(current.experimentId) > 0)) {
        latestExperiment.set(experiment.knowledgeId, summary);
      }
    }
    const records = knowledgeRecords
      .filter(record => record.target === 'tool_evolution')
      .map(record => ({
        knowledgeId: record.id,
        tool: toolName(record),
        status: record.status,
        confidence: record.confidence,
        proposalType: String(record.metadata?.proposalType ?? 'performance') as ToolEvolutionRecord['proposalType'],
        problem: record.hypothesis,
        proposal: structuredClone(record.recommendedAction),
        evidence: structuredClone(record.evidence),
        ...(latestProposal.has(record.id) ? { latestProposal: structuredClone(latestProposal.get(record.id)!) } : {}),
        ...(latestExperiment.has(record.id) ? { latestExperiment: structuredClone(latestExperiment.get(record.id)!) } : {})
      }))
      .sort((left, right) => left.knowledgeId.localeCompare(right.knowledgeId));
    return {
      revision,
      ...(proposalRevision ? { proposalRevision } : {}),
      ...(experimentRevision ? { experimentRevision } : {}),
      candidates: records
    };
  }
}

export class EvolvedSkillRegistry {
  constructor(private readonly store: KnowledgeStore) {}

  async snapshot(options: { includeNonPromoted?: boolean } = {}): Promise<{ revision: string; skills: EvolvedSkillRecord[] }> {
    const records = (await this.store.list())
      .filter(record => record.target === 'evolved_skill')
      .filter(record => options.includeNonPromoted || record.status === 'promoted')
      .map(record => {
        const base = record.metadata?.base as EvolvedSkillRecord['base'] | undefined;
        return {
          knowledgeId: record.id,
          name: String(record.metadata?.name ?? ''),
          status: record.status,
          confidence: record.confidence,
          base: base ?? { source: '', name: '', contentSha256: '' },
          generation: Number(record.metadata?.generation ?? 1),
          overlay: structuredClone(record.recommendedAction),
          evidence: structuredClone(record.evidence)
        };
      })
      .sort((left, right) => left.name.localeCompare(right.name) || left.knowledgeId.localeCompare(right.knowledgeId));
    return { revision: await this.store.revision(), skills: records };
  }
}
