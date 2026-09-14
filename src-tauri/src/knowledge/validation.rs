use std::collections::BTreeMap;

use super::impact::{KnowledgeImpactError, KnowledgeImpactRecord, KnowledgeImpactStore};
use super::model::{KnowledgeArtifactTarget, KnowledgeRecord, KnowledgeStatus};
use super::store::{KnowledgeStore, KnowledgeStoreError};

const MIN_SHADOW_MATCHES: u64 = 10;
const MIN_TASK_OUTCOMES: u64 = 3;
const MIN_VERIFIED_TASK_RATE: f64 = 0.8;
const MAX_TASK_FAILURE_RATE: f64 = 0.2;
const MAX_TOOL_FAILURE_RATE: f64 = 0.25;
const TOOL_EVOLUTION_FALLBACK_MATCHES: u64 = 20;

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator > 0 {
        numerator as f64 / denominator as f64
    } else {
        0.0
    }
}
fn task_outcome_count(impact: &KnowledgeImpactRecord) -> u64 {
    impact.task_verified_successes
        + impact.task_unverified_successes
        + impact.task_failures
        + impact.task_rollbacks
}
fn tool_strategy_ready(impact: &KnowledgeImpactRecord) -> bool {
    let tasks = task_outcome_count(impact);
    impact.matched >= MIN_SHADOW_MATCHES
        && tasks >= MIN_TASK_OUTCOMES
        && impact.task_rollbacks == 0
        && ratio(impact.task_verified_successes, tasks) >= MIN_VERIFIED_TASK_RATE
        && ratio(impact.task_failures, tasks) <= MAX_TASK_FAILURE_RATE
        && ratio(impact.actual_failures, impact.matched) <= MAX_TOOL_FAILURE_RATE
}
fn tool_evolution_ready(impact: &KnowledgeImpactRecord) -> bool {
    if impact.matched < MIN_SHADOW_MATCHES
        || ratio(impact.total_observed_duration_ms, impact.matched) < 1_000.0
    {
        return false;
    }
    let tasks = task_outcome_count(impact);
    if tasks < MIN_TASK_OUTCOMES {
        return impact.matched >= TOOL_EVOLUTION_FALLBACK_MATCHES;
    }
    impact.task_rollbacks == 0 && ratio(impact.task_failures, tasks) <= MAX_TASK_FAILURE_RATE
}
fn evolved_skill_ready(impact: &KnowledgeImpactRecord) -> bool {
    let tasks = task_outcome_count(impact);
    impact.matched >= MIN_SHADOW_MATCHES
        && tasks >= MIN_TASK_OUTCOMES
        && impact.task_rollbacks == 0
        && ratio(impact.task_verified_successes, tasks) >= MIN_VERIFIED_TASK_RATE
        && ratio(impact.task_failures, tasks) <= MAX_TASK_FAILURE_RATE
}

fn validation_ready(record: &KnowledgeRecord, impact: &KnowledgeImpactRecord) -> bool {
    match record.target {
        KnowledgeArtifactTarget::ToolStrategy => tool_strategy_ready(impact),
        KnowledgeArtifactTarget::ToolEvolution => tool_evolution_ready(impact),
        KnowledgeArtifactTarget::EvolvedSkill => evolved_skill_ready(impact),
        _ => false,
    }
}

#[derive(Debug)]
pub enum ValidationError {
    Knowledge(KnowledgeStoreError),
    Impact(KnowledgeImpactError),
}
impl From<KnowledgeStoreError> for ValidationError {
    fn from(v: KnowledgeStoreError) -> Self {
        Self::Knowledge(v)
    }
}
impl From<KnowledgeImpactError> for ValidationError {
    fn from(v: KnowledgeImpactError) -> Self {
        Self::Impact(v)
    }
}
impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Knowledge(e) => write!(f, "{e}"),
            Self::Impact(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for ValidationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationGateResult {
    pub evaluated: usize,
    pub validated: usize,
    pub blocked: usize,
}
#[derive(Debug, Clone)]
pub struct ValidationGate {
    knowledge_store: KnowledgeStore,
    impact_store: KnowledgeImpactStore,
}
impl ValidationGate {
    pub fn new(knowledge_store: KnowledgeStore, impact_store: KnowledgeImpactStore) -> Self {
        Self {
            knowledge_store,
            impact_store,
        }
    }
    pub fn evaluate(&self, updated_at_ms: u64) -> Result<ValidationGateResult, ValidationError> {
        let impacts = self
            .impact_store
            .list()?
            .into_iter()
            .map(|impact| (impact.knowledge_id.clone(), impact))
            .collect::<BTreeMap<_, _>>();
        let mut result = ValidationGateResult {
            evaluated: 0,
            validated: 0,
            blocked: 0,
        };
        for record in self.knowledge_store.list()? {
            if record.status != KnowledgeStatus::Shadow
                || !matches!(
                    record.target,
                    KnowledgeArtifactTarget::ToolStrategy
                        | KnowledgeArtifactTarget::ToolEvolution
                        | KnowledgeArtifactTarget::EvolvedSkill
                )
            {
                continue;
            }
            let Some(impact) = impacts.get(&record.id) else {
                continue;
            };
            result.evaluated += 1;
            if !validation_ready(&record, impact) {
                result.blocked += 1;
                continue;
            }
            if self
                .knowledge_store
                .transition_status(
                    &record.id,
                    &[KnowledgeStatus::Shadow],
                    KnowledgeStatus::Validated,
                    updated_at_ms,
                )?
                .is_some()
            {
                result.validated += 1;
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeEvidence, KnowledgeScope};
    use serde_json::json;
    use tempfile::tempdir;

    fn record(target: KnowledgeArtifactTarget) -> KnowledgeRecord {
        KnowledgeRecord {
            id: String::new(),
            schema_version: 1,
            scope: KnowledgeScope::Workspace,
            target,
            status: KnowledgeStatus::Shadow,
            trigger: json!({"tool":"project_state"}).as_object().unwrap().clone(),
            hypothesis: "slow baseline".into(),
            recommended_action: json!({"proposal":"incremental_or_cached_baseline"})
                .as_object()
                .unwrap()
                .clone(),
            evidence: KnowledgeEvidence {
                observations: 5,
                successes: 5,
                failures: 0,
                verified_successes: None,
                verification_failures: None,
                recovery_successes: None,
                recovery_failures: None,
                total_duration_ms: 10_000,
                total_request_bytes: 0,
                total_response_bytes: 0,
                first_seen_at_ms: 1,
                last_seen_at_ms: 2,
                task_context_ids: Some(vec!["a".repeat(64), "b".repeat(64), "c".repeat(64)]),
                conversation_context_ids: None,
                runtime_boot_context_ids: None,
            },
            confidence: 0.5,
            compatibility: None,
            source_event_ids: vec!["seed".into()],
            counterexample_event_ids: vec![],
            supersedes: None,
            superseded_by: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            metadata: None,
        }
    }
    fn impact(id: String, target: KnowledgeArtifactTarget, matched: u64) -> KnowledgeImpactRecord {
        KnowledgeImpactRecord {
            schema_version: 1,
            knowledge_id: id,
            target,
            tool: "project_state".into(),
            source_event_ids: (0..matched).map(|i| format!("e{i}")).collect(),
            matched_task_ids: vec![],
            credited_task_ids: vec![],
            considered: matched,
            matched,
            actual_successes: matched,
            actual_failures: 0,
            verified_successes: 0,
            verification_failures: 0,
            recovery_successes: 0,
            recovery_failures: 0,
            task_verified_successes: 0,
            task_unverified_successes: 0,
            task_failures: 0,
            task_rollbacks: 0,
            total_observed_duration_ms: matched * 1_500,
            total_observed_request_bytes: 0,
            total_observed_response_bytes: 0,
            first_seen_at_ms: 1,
            last_seen_at_ms: 2,
        }
    }

    #[test]
    fn tool_evolution_uses_twenty_match_fallback_without_task_terminal_telemetry() {
        let root = tempdir().unwrap();
        let store = KnowledgeStore::new(root.path());
        let impacts = KnowledgeImpactStore::new(root.path());
        let stored = store
            .upsert(record(KnowledgeArtifactTarget::ToolEvolution))
            .unwrap();
        impacts
            .record(impact(stored.id.clone(), stored.target, 20))
            .unwrap();
        let result = ValidationGate::new(store.clone(), impacts)
            .evaluate(10)
            .unwrap();
        assert_eq!(result.validated, 1);
        assert_eq!(
            store.get(&stored.id).unwrap().unwrap().status,
            KnowledgeStatus::Validated
        );
    }

    #[test]
    fn tool_strategy_does_not_fake_task_outcomes_from_tool_success() {
        let root = tempdir().unwrap();
        let store = KnowledgeStore::new(root.path());
        let impacts = KnowledgeImpactStore::new(root.path());
        let stored = store
            .upsert(record(KnowledgeArtifactTarget::ToolStrategy))
            .unwrap();
        impacts
            .record(impact(stored.id.clone(), stored.target, 30))
            .unwrap();
        let result = ValidationGate::new(store.clone(), impacts)
            .evaluate(10)
            .unwrap();
        assert_eq!(result.validated, 0);
        assert_eq!(result.blocked, 1);
    }
}
