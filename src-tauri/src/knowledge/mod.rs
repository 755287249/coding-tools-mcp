mod canary;
mod compiler;
mod evolved_skill_canary;
mod impact;
mod mcp;
mod model;
mod persistence;
mod registries;
mod runtime;
mod shadow;
mod store;
mod strategy_implementations;
mod telemetry;
mod tool_evolution;
mod validation;

pub use canary::{
    evaluate_canary_promotions, evaluate_canary_rollbacks, CanaryError, CanaryImpactRecord,
    CanaryImpactStore, CanaryObservation, CanaryStage, CanaryStrategyEngine,
    KnowledgeCanaryDecision,
};
pub use compiler::ExperienceCompiler;
pub use evolved_skill_canary::{
    evaluate_evolved_skill_promotions, evaluate_evolved_skill_rollbacks,
    EvolvedSkillCanaryDecision, EvolvedSkillCanaryEngine, EVOLVED_SKILL_CANARY_IMPLEMENTATION,
};
pub use impact::{KnowledgeImpactError, KnowledgeImpactRecord, KnowledgeImpactStore};
pub use mcp::{
    is_tool_evolution_resource_uri, list_tool_evolution_resources, merge_resource_lists,
    read_tool_evolution_resource, tool_evolution_resource_uri,
};
pub use model::{
    EvidenceOutcome, ExperienceCanary, ExperienceFeatures, ExperienceObservation,
    ExperienceOutcome, ExperienceSkillAttribution, KnowledgeArtifactTarget, KnowledgeCompatibility,
    KnowledgeEvidence, KnowledgeRecord, KnowledgeScope, KnowledgeStatus, KnowledgeTaskOutcome,
};
pub use registries::{
    EvolvedSkillBase, EvolvedSkillRecord, EvolvedSkillRegistry, EvolvedSkillSnapshot,
    ToolEvolutionChangeProposalSummary, ToolEvolutionExperimentSummary, ToolEvolutionRecord,
    ToolEvolutionRegistry, ToolEvolutionSnapshot, ToolStrategyRecord, ToolStrategyRegistry,
    ToolStrategySnapshot,
};
pub(crate) use runtime::{
    enqueue_task_outcome, enqueue_tool_usage_record, sync_tool_evolution_lifecycle_for_workspace,
};
pub use runtime::{knowledge_store_for_context, learning_bootstrap_summary};
pub use shadow::{ShadowError, ShadowEvaluator};
pub use store::{
    knowledge_evidence_candidate_ready, knowledge_record_id, KnowledgeStore, KnowledgeStoreError,
};
pub use strategy_implementations::{
    resolve_tool_strategy_implementation, tool_strategy_implementation_snapshot,
    ToolStrategyImplementationId, ToolStrategyImplementationSafety, ToolStrategyImplementationSpec,
};
pub use telemetry::{tool_usage_record_to_observation, ExperienceAttribution};
pub use tool_evolution::{
    create_tool_evolution_experiment, materialize_tool_evolution_proposal,
    promote_tool_evolution_experiment, tool_evolution_experiment_id, tool_evolution_proposal_id,
    ToolEvolutionBenchmarkAggregate, ToolEvolutionBenchmarkArm, ToolEvolutionBenchmarkSample,
    ToolEvolutionCandidateClaimResult, ToolEvolutionCandidateClaimer,
    ToolEvolutionChangeProposalRecord, ToolEvolutionChangeProposalStatus, ToolEvolutionError,
    ToolEvolutionExperimentPlan, ToolEvolutionExperimentRecord, ToolEvolutionExperimentStatus,
    ToolEvolutionExperimentStore, ToolEvolutionPlanner, ToolEvolutionPromotionResult,
    ToolEvolutionProposalStore, ToolEvolutionProposalSyncResult, ToolEvolutionProposalType,
    ToolEvolutionRuntimeSource, ToolEvolutionSampleDigest, ToolEvolutionTaskOutcome,
};
pub use validation::{ValidationError, ValidationGate, ValidationGateResult};
