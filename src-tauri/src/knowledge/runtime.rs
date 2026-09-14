use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::harness::Harness;
use crate::tools::ToolContext;

use super::canary::{
    evaluate_canary_promotions, evaluate_canary_rollbacks, CanaryImpactStore, CanaryObservation,
    CanaryStage, KnowledgeCanaryDecision,
};
use super::compiler::ExperienceCompiler;
use super::evolved_skill_canary::{
    evaluate_evolved_skill_promotions, evaluate_evolved_skill_rollbacks,
};
use super::impact::KnowledgeImpactStore;
use super::model::{KnowledgeStatus, KnowledgeTaskOutcome};
use super::registries::{EvolvedSkillRegistry, ToolEvolutionRegistry, ToolStrategyRegistry};
use super::shadow::ShadowEvaluator;
use super::store::{KnowledgeStore, KnowledgeStoreError};
use super::strategy_implementations::tool_strategy_implementation_snapshot;
use super::telemetry::{tool_usage_record_to_observation, ExperienceAttribution};
use super::tool_evolution::{
    ToolEvolutionCandidateClaimResult, ToolEvolutionCandidateClaimer, ToolEvolutionExperimentStore,
    ToolEvolutionPlanner, ToolEvolutionProposalStore, ToolEvolutionProposalSyncResult,
    ToolEvolutionRuntimeSource,
};
use super::validation::ValidationGate;

const INGESTION_QUEUE_CAPACITY: usize = 256;
enum IngestionEntry {
    ToolUsage {
        profile_id: String,
        folder_id: String,
        record: Value,
        attribution: ExperienceAttribution,
    },
    TaskOutcome {
        knowledge_root: std::path::PathBuf,
        task_id: String,
        outcome: KnowledgeTaskOutcome,
        timestamp_ms: u64,
    },
}

static INGESTION_SENDER: OnceLock<Option<SyncSender<IngestionEntry>>> = OnceLock::new();
static INGESTION_DROPPED: AtomicU64 = AtomicU64::new(0);
static INGESTION_FAILURES: AtomicU64 = AtomicU64::new(0);

fn ingestion_sender() -> Option<&'static SyncSender<IngestionEntry>> {
    INGESTION_SENDER
        .get_or_init(|| {
            let (sender, receiver) = sync_channel::<IngestionEntry>(INGESTION_QUEUE_CAPACITY);
            thread::Builder::new()
                .name("knowledge-ingestion".into())
                .spawn(move || {
                    while let Ok(entry) = receiver.recv() {
                        let result = match entry {
                            IngestionEntry::ToolUsage {
                                profile_id,
                                folder_id,
                                record,
                                attribution,
                            } => ingest_record(&profile_id, &folder_id, &record, attribution),
                            IngestionEntry::TaskOutcome {
                                knowledge_root,
                                task_id,
                                outcome,
                                timestamp_ms,
                            } => ingest_task_outcome(
                                &knowledge_root,
                                &task_id,
                                outcome,
                                timestamp_ms,
                            ),
                        };
                        if result.is_err() {
                            INGESTION_FAILURES.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

fn runtime_tool_evolution_source() -> ToolEvolutionRuntimeSource {
    let revision = option_env!("CTMCP_BUILD_GIT_SHA")
        .unwrap_or("unknown")
        .trim();
    let source_clean = matches!(option_env!("CTMCP_BUILD_SOURCE_CLEAN"), Some("true"));
    let trusted_revision = revision.len() == 40
        && revision
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit());
    let proposal_ids = option_env!("CTMCP_TOOL_EVOLUTION_PROPOSAL_IDS")
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    ToolEvolutionRuntimeSource {
        revision: revision.to_ascii_lowercase(),
        trusted: source_clean && trusted_revision,
        proposal_ids,
    }
}

fn lifecycle_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

fn sync_tool_evolution_lifecycle(
    store: KnowledgeStore,
    source: ToolEvolutionRuntimeSource,
    timestamp_ms: u64,
) -> Result<
    (
        ToolEvolutionCandidateClaimResult,
        ToolEvolutionProposalSyncResult,
    ),
    KnowledgeStoreError,
> {
    let root = store.root().to_path_buf();
    let proposal_store = ToolEvolutionProposalStore::new(&root);
    let experiment_store = ToolEvolutionExperimentStore::new(&root);
    let claims = ToolEvolutionCandidateClaimer::new(
        store.clone(),
        proposal_store.clone(),
        experiment_store.clone(),
        source.clone(),
    )
    .sync(timestamp_ms);
    let proposals =
        ToolEvolutionPlanner::new(store, proposal_store, Some(experiment_store), source)
            .sync(timestamp_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    Ok((claims, proposals))
}

pub(crate) fn sync_tool_evolution_lifecycle_for_workspace(
    workspace_root: &Path,
) -> Result<(), KnowledgeStoreError> {
    let harness_root = Harness::default_root()
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let harness = Harness::new(workspace_root.to_path_buf(), harness_root)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let store = KnowledgeStore::new(
        harness
            .store_root()
            .join("knowledge")
            .join(harness.workspace_id()),
    );
    sync_tool_evolution_lifecycle(
        store,
        runtime_tool_evolution_source(),
        lifecycle_timestamp_ms(),
    )?;
    Ok(())
}

fn ingest_record(
    profile_id: &str,
    folder_id: &str,
    record: &Value,
    mut attribution: ExperienceAttribution,
) -> Result<(), KnowledgeStoreError> {
    let context = crate::tools::hub::resolve_profile_folder_context(profile_id, folder_id)
        .map_err(KnowledgeStoreError::Validation)?;
    if attribution.operation_id.is_none() {
        attribution.operation_id = record
            .get("conversation_operation_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    let Some(observation) = tool_usage_record_to_observation(record, Some(folder_id), &attribution)
    else {
        return Ok(());
    };
    let updated_at_ms = observation.timestamp_ms;
    let store = knowledge_store_for_context(context.as_ref());
    let root = store.root().to_path_buf();
    let runtime_source = runtime_tool_evolution_source();
    let proposal_store = ToolEvolutionProposalStore::new(&root);
    let experiment_store = ToolEvolutionExperimentStore::new(&root);
    ToolEvolutionCandidateClaimer::new(
        store.clone(),
        proposal_store.clone(),
        experiment_store.clone(),
        runtime_source.clone(),
    )
    .sync(updated_at_ms);
    let impact_store = KnowledgeImpactStore::new(&root);
    let canary_store = CanaryImpactStore::new(&root);
    if let Some(canary) = &observation.canary {
        let implementation = serde_json::from_value(Value::String(canary.implementation.clone()))
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
        let stage = match canary.stage.as_str() {
            "canary" => CanaryStage::Canary,
            "promoted" => CanaryStage::Promoted,
            _ => {
                return Err(KnowledgeStoreError::Validation(
                    "invalid canary stage".into(),
                ))
            }
        };
        canary_store
            .record_observations(&[CanaryObservation {
                event_id: observation.event_id.clone(),
                task_id: observation.task_id.clone(),
                outcome: observation.outcome,
                duration_ms: observation.duration_ms,
                timestamp_ms: observation.timestamp_ms,
                decision: KnowledgeCanaryDecision {
                    knowledge_id: canary.knowledge_id.clone(),
                    implementation,
                    eligible: canary.eligible,
                    stage,
                    applied: canary.applied,
                    bucket: canary.bucket,
                },
            }])
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    }
    let shadow = ShadowEvaluator::new(store.clone(), impact_store.clone());
    let records_before = store.list()?;
    shadow
        .evaluate(std::slice::from_ref(&observation), &records_before)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    for knowledge in ExperienceCompiler.compile(std::slice::from_ref(&observation))? {
        store.upsert(knowledge)?;
    }
    shadow
        .activate_candidates(updated_at_ms)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    ValidationGate::new(store.clone(), impact_store.clone())
        .evaluate(updated_at_ms)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let (_, tool_rolled_back) =
        evaluate_canary_rollbacks(&store, &impact_store, &canary_store, updated_at_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let (_, skill_rolled_back) =
        evaluate_evolved_skill_rollbacks(&store, &canary_store, updated_at_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    if tool_rolled_back == 0 {
        evaluate_canary_promotions(&store, &canary_store, updated_at_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    }
    if skill_rolled_back == 0 {
        evaluate_evolved_skill_promotions(&store, &canary_store, updated_at_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    }
    ToolEvolutionPlanner::new(
        store,
        proposal_store,
        Some(experiment_store),
        runtime_source,
    )
    .sync(updated_at_ms)
    .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    Ok(())
}

pub(crate) fn enqueue_tool_usage_record(
    profile_id: &str,
    record: &Value,
    attribution: ExperienceAttribution,
) {
    if record.get("event_type").and_then(Value::as_str) != Some("tool_call") {
        return;
    }
    let Some(folder_id) = record
        .get("selected_workspace_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(sender) = ingestion_sender() else {
        INGESTION_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    match sender.try_send(IngestionEntry::ToolUsage {
        profile_id: profile_id.to_string(),
        folder_id: folder_id.to_string(),
        record: record.clone(),
        attribution,
    }) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            INGESTION_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn ingestion_health() -> Value {
    json!({
        "enabled": true,
        "state": if INGESTION_SENDER.get().is_some_and(|sender| sender.is_some()) { "ready" } else { "not_initialized" },
        "queue_capacity": INGESTION_QUEUE_CAPACITY,
        "dropped_records": INGESTION_DROPPED.load(Ordering::Relaxed),
        "failures": INGESTION_FAILURES.load(Ordering::Relaxed)
    })
}

fn ingest_task_outcome(
    knowledge_root: &std::path::Path,
    task_id: &str,
    outcome: KnowledgeTaskOutcome,
    timestamp_ms: u64,
) -> Result<(), KnowledgeStoreError> {
    let store = KnowledgeStore::new(knowledge_root);
    let impact_store = KnowledgeImpactStore::new(knowledge_root);
    impact_store
        .credit_task_outcome(task_id, outcome, timestamp_ms)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let canary_store = CanaryImpactStore::new(knowledge_root);
    canary_store
        .credit_task_outcome(task_id, outcome, timestamp_ms)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    ValidationGate::new(store.clone(), impact_store.clone())
        .evaluate(timestamp_ms)
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let (_, tool_rolled_back) =
        evaluate_canary_rollbacks(&store, &impact_store, &canary_store, timestamp_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let (_, skill_rolled_back) =
        evaluate_evolved_skill_rollbacks(&store, &canary_store, timestamp_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    if tool_rolled_back == 0 {
        evaluate_canary_promotions(&store, &canary_store, timestamp_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    }
    if skill_rolled_back == 0 {
        evaluate_evolved_skill_promotions(&store, &canary_store, timestamp_ms)
            .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    }
    Ok(())
}

pub(crate) fn enqueue_task_outcome(
    ctx: &ToolContext,
    task_id: &str,
    outcome: KnowledgeTaskOutcome,
    timestamp_ms: u64,
) {
    let Some(sender) = ingestion_sender() else {
        INGESTION_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let knowledge_root = knowledge_store_for_context(ctx).root().to_path_buf();
    match sender.try_send(IngestionEntry::TaskOutcome {
        knowledge_root,
        task_id: task_id.to_string(),
        outcome,
        timestamp_ms,
    }) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            INGESTION_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn knowledge_store_for_context(ctx: &ToolContext) -> KnowledgeStore {
    KnowledgeStore::new(
        ctx.harness
            .store_root()
            .join("knowledge")
            .join(ctx.harness.workspace_id()),
    )
}

pub fn learning_bootstrap_summary(ctx: &ToolContext) -> Result<Value, KnowledgeStoreError> {
    let store = knowledge_store_for_context(ctx);
    let knowledge_revision = store.revision()?;
    let tool_strategies = ToolStrategyRegistry::new(store.clone()).snapshot()?;
    let strategy_implementations = tool_strategy_implementation_snapshot();
    let tool_evolution = ToolEvolutionRegistry::with_lifecycle_stores(
        store.clone(),
        ToolEvolutionExperimentStore::new(store.root()),
        ToolEvolutionProposalStore::new(store.root()),
    )
    .snapshot()?;
    let evolved_skills = EvolvedSkillRegistry::new(store.clone()).snapshot(false)?;
    let shadow_store = KnowledgeImpactStore::new(store.root());
    let shadow_revision = shadow_store
        .revision()
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let shadow_impacts = shadow_store
        .list()
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let canary_store = CanaryImpactStore::new(store.root());
    let canary_revision = canary_store
        .revision()
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;
    let canary_impacts = canary_store
        .list()
        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?;

    Ok(json!({
        "knowledge_revision": knowledge_revision,
        "tool_strategy_revision": tool_strategies.revision,
        "tool_strategy_count": tool_strategies.strategies.len(),
        "tool_strategy_implementation_revision": strategy_implementations["revision"],
        "tool_strategy_implementation_count": strategy_implementations["count"],
        "tool_evolution_revision": tool_evolution.revision,
        "tool_evolution_proposal_revision": tool_evolution.proposal_revision,
        "tool_evolution_experiment_revision": tool_evolution.experiment_revision,
        "tool_evolution_candidate_count": tool_evolution.candidates.len(),
        "evolved_skill_revision": evolved_skills.revision,
        "promoted_evolved_skill_count": evolved_skills.skills.len(),
        "shadow_impact_revision": shadow_revision,
        "shadow_impact_count": shadow_impacts.len(),
        "canary_impact_revision": canary_revision,
        "canary_impact_count": canary_impacts.len(),
        "canary_promoted_count": canary_impacts.iter().filter(|item| item.promoted_at_ms.is_some() && item.rolled_back_at_ms.is_none()).count(),
        "canary_rolled_back_count": canary_impacts.iter().filter(|item| item.rolled_back_at_ms.is_some()).count(),
        "validated_tool_strategy_count": tool_strategies.strategies.iter().filter(|item| item.status == KnowledgeStatus::Validated).count(),
        "promoted_tool_strategy_count": tool_strategies.strategies.iter().filter(|item| item.status == KnowledgeStatus::Promoted).count(),
        "validated_tool_evolution_count": tool_evolution.candidates.iter().filter(|item| item.status == KnowledgeStatus::Validated).count(),
        "ingestion": ingestion_health(),
        "runtime_policy": "validated_canary_rust_parity",
        "canary_policy": "Validated strategies use a deterministic 25% semantics-preserving canary; evidence-positive strategies promote to 100% rollout, while post-promotion regressions automatically reject and disable them. Caller arguments and static security policy remain authoritative.",
        "tool_evolution_resource_policy": "proposed_only_read_only",
        "knowledge_loading_policy": "Persistent knowledge and candidates are not injected into inference context. Shadow validation, bounded canary intervention, rollback, automatic promotion, and promoted runtime execution are active; learned policy never overrides hard workspace, permission, or sandbox policy."
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::knowledge::{ExperienceCompiler, ExperienceObservation, ExperienceOutcome};

    fn observation(event_id: &str) -> ExperienceObservation {
        ExperienceObservation {
            event_id: event_id.into(),
            tool: "search_text".into(),
            outcome: ExperienceOutcome::Success,
            duration_ms: 2_000,
            request_bytes: None,
            response_bytes: None,
            error_code: None,
            features: Some(
                json!({"calculate_total": true, "files_considered": 800})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
            verification_outcome: None,
            recovery_outcome: None,
            canary: None,
            operation_id: None,
            task_id: None,
            conversation_context_id: None,
            skill: None,
            runtime_boot_id: None,
            timestamp_ms: 1_000,
        }
    }

    #[test]
    fn bootstrap_summary_exposes_only_compact_read_only_learning_metadata() {
        let workspace = tempdir().unwrap();
        let harness = tempdir().unwrap();
        let ctx =
            ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
                .unwrap();
        let store = knowledge_store_for_context(&ctx);
        let record = ExperienceCompiler
            .compile(&[observation("event-1")])
            .unwrap()
            .remove(0);
        store.upsert(record).unwrap();

        let summary = learning_bootstrap_summary(&ctx).unwrap();
        assert_eq!(summary["tool_strategy_count"], 1);
        assert_eq!(summary["tool_strategy_implementation_count"], 1);
        assert!(summary["tool_strategy_implementation_revision"].is_string());
        assert!(summary["shadow_impact_revision"].is_string());
        assert_eq!(summary["shadow_impact_count"], 0);
        assert!(summary["canary_impact_revision"].is_string());
        assert_eq!(summary["canary_impact_count"], 0);
        assert_eq!(summary["canary_promoted_count"], 0);
        assert_eq!(summary["canary_rolled_back_count"], 0);
        assert_eq!(summary["runtime_policy"], "validated_canary_rust_parity");
        assert_eq!(summary["ingestion"]["enabled"], true);
        assert_eq!(
            summary["ingestion"]["queue_capacity"],
            INGESTION_QUEUE_CAPACITY
        );
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains("Exact total counting"));
        assert!(!serialized.contains("recommended_action"));
    }
}
