export type KnowledgeStatus =
  | 'observed'
  | 'candidate'
  | 'shadow'
  | 'validated'
  | 'promoted'
  | 'rejected'
  | 'deprecated'
  | 'superseded';

export type KnowledgeArtifactTarget =
  | 'tool_strategy'
  | 'runtime_policy'
  | 'tool_evolution'
  | 'evolved_skill'
  | 'product_optimization'
  | 'ignore';

export type KnowledgeScope = 'runtime' | 'workspace' | 'user' | 'global';

export interface KnowledgeCompatibility {
  hosts?: string[];
  tools?: string[];
  models?: string[];
  minimumCapability?: 'small' | 'medium' | 'large';
}

export interface KnowledgeEvidence {
  observations: number;
  successes: number;
  failures: number;
  verifiedSuccesses?: number;
  verificationFailures?: number;
  recoverySuccesses?: number;
  recoveryFailures?: number;
  totalDurationMs: number;
  totalRequestBytes: number;
  totalResponseBytes: number;
  firstSeenAtMs: number;
  lastSeenAtMs: number;
  taskContextIds?: string[];
  conversationContextIds?: string[];
  runtimeBootContextIds?: string[];
}

export interface KnowledgeRecord {
  id: string;
  schemaVersion: 1;
  scope: KnowledgeScope;
  target: KnowledgeArtifactTarget;
  status: KnowledgeStatus;
  trigger: Record<string, unknown>;
  hypothesis: string;
  recommendedAction: Record<string, unknown>;
  evidence: KnowledgeEvidence;
  confidence: number;
  compatibility?: KnowledgeCompatibility;
  sourceEventIds: string[];
  counterexampleEventIds: string[];
  supersedes?: string[];
  supersededBy?: string;
  createdAtMs: number;
  updatedAtMs: number;
  metadata?: Record<string, unknown>;
}

export type TaskOutcome = 'verified_success' | 'unverified_success' | 'failure' | 'rolled_back';

export interface ExperienceObservation {
  eventId: string;
  tool: string;
  outcome: 'success' | 'failure';
  durationMs: number;
  requestBytes?: number;
  responseBytes?: number;
  errorCode?: string;
  features?: Record<string, string | number | boolean | null>;
  verificationOutcome?: 'passed' | 'failed';
  recoveryOutcome?: 'passed' | 'failed';
  operationId?: string;
  taskId?: string;
  conversationContextId?: string;
  skill?: {
    source: string;
    name: string;
    contentSha256: string;
    generation: number;
  };
  runtimeBootId?: string;
  taskOutcome?: TaskOutcome;
  canary?: {
    knowledgeId: string;
    implementation: string;
    eligible: boolean;
    applied: boolean;
    bucket: number;
  };
  timestampMs: number;
}

export interface ToolStrategyRecord {
  knowledgeId: string;
  tool: string;
  status: KnowledgeStatus;
  confidence: number;
  trigger: Record<string, unknown>;
  action: Record<string, unknown>;
  evidence: KnowledgeEvidence;
}

export type ToolEvolutionProposalType = 'schema' | 'primitive' | 'recovery' | 'performance' | 'result';

export type ToolEvolutionChangeProposalStatus = 'proposed' | 'materialized' | 'superseded' | 'dismissed';

export interface ToolEvolutionChangeProposalRecord {
  schemaVersion: 1;
  proposalId: string;
  knowledgeId: string;
  tool: string;
  proposalType: ToolEvolutionProposalType;
  baseRevision: string;
  action: Record<string, unknown>;
  status: ToolEvolutionChangeProposalStatus;
  createdAtMs: number;
  updatedAtMs: number;
  candidateRevision?: string;
  experimentId?: string;
}

export interface ToolEvolutionChangeProposalSummary {
  proposalId: string;
  status: ToolEvolutionChangeProposalStatus;
  baseRevision: string;
  updatedAtMs: number;
  candidateRevision?: string;
  experimentId?: string;
}

export type ToolEvolutionExperimentStatus =
  | 'planned'
  | 'running'
  | 'validated'
  | 'rejected'
  | 'promoted'
  | 'stale';

export interface ToolEvolutionBenchmarkSample {
  sampleId: string;
  sourceRevision: string;
  taskOutcome?: TaskOutcome;
  toolCalls: number;
  toolFailures: number;
  durationMs: number;
  responseBytes: number;
}

export interface ToolEvolutionBenchmarkAggregate {
  samples: number;
  taskOutcomes: number;
  verifiedTaskSuccesses: number;
  unverifiedTaskSuccesses: number;
  taskFailures: number;
  taskRollbacks: number;
  toolCalls: number;
  toolFailures: number;
  durationMs: number;
  responseBytes: number;
  sampleDigests: Array<{ sampleId: string; digest: string }>;
}

export interface ToolEvolutionExperimentPlan {
  knowledgeId: string;
  tool: string;
  proposalType: ToolEvolutionProposalType;
  baseRevision: string;
  candidateRevision: string;
  createdAtMs: number;
}

export interface ToolEvolutionExperimentRecord extends ToolEvolutionExperimentPlan {
  schemaVersion: 1;
  experimentId: string;
  status: ToolEvolutionExperimentStatus;
  baseline: ToolEvolutionBenchmarkAggregate;
  candidate: ToolEvolutionBenchmarkAggregate;
  updatedAtMs: number;
  decisionReason?: string;
}

export interface ToolEvolutionExperimentSummary {
  experimentId: string;
  status: ToolEvolutionExperimentStatus;
  baseRevision: string;
  candidateRevision: string;
  updatedAtMs: number;
  decisionReason?: string;
}

export interface ToolEvolutionRecord {
  knowledgeId: string;
  tool: string;
  status: KnowledgeStatus;
  confidence: number;
  proposalType: ToolEvolutionProposalType;
  problem: string;
  proposal: Record<string, unknown>;
  evidence: KnowledgeEvidence;
  latestProposal?: ToolEvolutionChangeProposalSummary;
  latestExperiment?: ToolEvolutionExperimentSummary;
}

export interface EvolvedSkillRecord {
  knowledgeId: string;
  name: string;
  status: KnowledgeStatus;
  confidence: number;
  base: {
    source: string;
    name: string;
    contentSha256: string;
  };
  generation: number;
  overlay: Record<string, unknown>;
  evidence: KnowledgeEvidence;
}
