use sha2::{Digest, Sha256};

use super::impact::{KnowledgeImpactError, KnowledgeImpactRecord, KnowledgeImpactStore};
use super::model::{
    EvidenceOutcome, ExperienceObservation, ExperienceOutcome, KnowledgeArtifactTarget,
    KnowledgeRecord, KnowledgeStatus,
};
use super::store::{KnowledgeStore, KnowledgeStoreError};

fn tool_for(record: &KnowledgeRecord) -> &str {
    record
        .trigger
        .get("tool")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn evolved_skill_trigger_matches(
    record: &KnowledgeRecord,
    observation: &ExperienceObservation,
) -> bool {
    if tool_for(record) != observation.tool
        || record.target != KnowledgeArtifactTarget::EvolvedSkill
    {
        return false;
    }
    let Some(skill) = observation.skill.as_ref() else {
        return false;
    };
    if record
        .trigger
        .get("pattern")
        .and_then(|value| value.as_str())
        != Some("stale_guarded_edit_recovery")
        || !matches!(
            observation.error_code.as_deref(),
            Some("FILE_VERSION_MISMATCH" | "EDIT_MATCH_COUNT_MISMATCH")
        )
        || record
            .trigger
            .get("skill_source")
            .and_then(|value| value.as_str())
            != Some(skill.source.as_str())
        || record
            .trigger
            .get("base_content_sha256")
            .and_then(|value| value.as_str())
            != Some(skill.content_sha256.as_str())
    {
        return false;
    }
    let name_hash = format!("{:x}", Sha256::digest(skill.name.as_bytes()));
    record
        .trigger
        .get("skill_name_sha256")
        .and_then(|value| value.as_str())
        == Some(name_hash.as_str())
}

fn trigger_matches(record: &KnowledgeRecord, observation: &ExperienceObservation) -> bool {
    if record.target == KnowledgeArtifactTarget::EvolvedSkill {
        return evolved_skill_trigger_matches(record, observation);
    }
    if tool_for(record) != observation.tool {
        return false;
    }
    if let Some(expected) = record
        .trigger
        .get("errorCode")
        .and_then(|value| value.as_str())
    {
        if observation.error_code.as_deref() != Some(expected) {
            return false;
        }
    }
    for (key, expected) in &record.trigger {
        if key == "tool" || key == "errorCode" {
            continue;
        }
        if observation
            .features
            .as_ref()
            .and_then(|features| features.get(key))
            != Some(expected)
        {
            return false;
        }
    }
    if record
        .recommended_action
        .get("recommendation")
        .and_then(|value| value.as_str())
        == Some("prefer_bounded_search")
    {
        let files = observation
            .features
            .as_ref()
            .and_then(|features| features.get("files_considered"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        return files >= 500 || observation.duration_ms >= 1_000;
    }
    if record
        .recommended_action
        .get("proposal")
        .and_then(|value| value.as_str())
        == Some("incremental_or_cached_baseline")
    {
        let baseline_ms = observation
            .features
            .as_ref()
            .and_then(|features| features.get("phase_baseline_capture_ms"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        return baseline_ms >= 1_000;
    }
    true
}

fn delta(
    record: &KnowledgeRecord,
    observations: &[ExperienceObservation],
) -> Option<KnowledgeImpactRecord> {
    let relevant = observations
        .iter()
        .filter(|item| item.tool == tool_for(record))
        .collect::<Vec<_>>();
    if relevant.is_empty() {
        return None;
    }
    let matched = relevant
        .iter()
        .copied()
        .filter(|item| trigger_matches(record, item))
        .collect::<Vec<_>>();
    Some(KnowledgeImpactRecord {
        schema_version: 1,
        knowledge_id: record.id.clone(),
        target: record.target,
        tool: tool_for(record).to_string(),
        source_event_ids: relevant.iter().map(|item| item.event_id.clone()).collect(),
        matched_task_ids: matched
            .iter()
            .filter_map(|item| item.task_id.clone())
            .collect(),
        credited_task_ids: vec![],
        considered: relevant.len() as u64,
        matched: matched.len() as u64,
        actual_successes: matched
            .iter()
            .filter(|item| item.outcome == ExperienceOutcome::Success)
            .count() as u64,
        actual_failures: matched
            .iter()
            .filter(|item| item.outcome == ExperienceOutcome::Failure)
            .count() as u64,
        verified_successes: matched
            .iter()
            .filter(|item| item.verification_outcome == Some(EvidenceOutcome::Passed))
            .count() as u64,
        verification_failures: matched
            .iter()
            .filter(|item| item.verification_outcome == Some(EvidenceOutcome::Failed))
            .count() as u64,
        recovery_successes: matched
            .iter()
            .filter(|item| item.recovery_outcome == Some(EvidenceOutcome::Passed))
            .count() as u64,
        recovery_failures: matched
            .iter()
            .filter(|item| item.recovery_outcome == Some(EvidenceOutcome::Failed))
            .count() as u64,
        task_verified_successes: 0,
        task_unverified_successes: 0,
        task_failures: 0,
        task_rollbacks: 0,
        total_observed_duration_ms: matched.iter().map(|item| item.duration_ms).sum(),
        total_observed_request_bytes: matched
            .iter()
            .map(|item| item.request_bytes.unwrap_or(0))
            .sum(),
        total_observed_response_bytes: matched
            .iter()
            .map(|item| item.response_bytes.unwrap_or(0))
            .sum(),
        first_seen_at_ms: relevant
            .iter()
            .map(|item| item.timestamp_ms)
            .min()
            .unwrap_or(0),
        last_seen_at_ms: relevant
            .iter()
            .map(|item| item.timestamp_ms)
            .max()
            .unwrap_or(0),
    })
}

#[derive(Debug)]
pub enum ShadowError {
    Knowledge(KnowledgeStoreError),
    Impact(KnowledgeImpactError),
}
impl From<KnowledgeStoreError> for ShadowError {
    fn from(value: KnowledgeStoreError) -> Self {
        Self::Knowledge(value)
    }
}
impl From<KnowledgeImpactError> for ShadowError {
    fn from(value: KnowledgeImpactError) -> Self {
        Self::Impact(value)
    }
}
impl std::fmt::Display for ShadowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Knowledge(e) => write!(f, "{e}"),
            Self::Impact(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for ShadowError {}

#[derive(Debug, Clone)]
pub struct ShadowEvaluator {
    knowledge_store: KnowledgeStore,
    impact_store: KnowledgeImpactStore,
}
impl ShadowEvaluator {
    pub fn new(knowledge_store: KnowledgeStore, impact_store: KnowledgeImpactStore) -> Self {
        Self {
            knowledge_store,
            impact_store,
        }
    }

    pub fn evaluate(
        &self,
        observations: &[ExperienceObservation],
        records_before_batch: &[KnowledgeRecord],
    ) -> Result<(u64, u64), ShadowError> {
        let mut considered = 0;
        let mut matched = 0;
        for record in records_before_batch {
            if !matches!(
                record.status,
                KnowledgeStatus::Candidate | KnowledgeStatus::Shadow
            ) {
                continue;
            }
            if !matches!(
                record.target,
                KnowledgeArtifactTarget::ToolStrategy
                    | KnowledgeArtifactTarget::ToolEvolution
                    | KnowledgeArtifactTarget::EvolvedSkill
            ) {
                continue;
            }
            let Some(impact) = delta(record, observations) else {
                continue;
            };
            considered += impact.considered;
            matched += impact.matched;
            self.impact_store.record(impact)?;
        }
        Ok((considered, matched))
    }

    pub fn activate_candidates(&self, updated_at_ms: u64) -> Result<usize, ShadowError> {
        let mut activated = 0;
        for record in self.knowledge_store.list()? {
            if record.status != KnowledgeStatus::Candidate
                || !matches!(
                    record.target,
                    KnowledgeArtifactTarget::ToolStrategy
                        | KnowledgeArtifactTarget::ToolEvolution
                        | KnowledgeArtifactTarget::EvolvedSkill
                )
            {
                continue;
            }
            if self
                .knowledge_store
                .transition_status(
                    &record.id,
                    &[KnowledgeStatus::Candidate],
                    KnowledgeStatus::Shadow,
                    updated_at_ms,
                )?
                .is_some()
            {
                activated += 1;
            }
        }
        Ok(activated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{ExperienceCompiler, ExperienceFeatures};
    use serde_json::json;
    use tempfile::tempdir;

    fn obs(id: &str, files: u64, timestamp_ms: u64) -> ExperienceObservation {
        ExperienceObservation {
            event_id: id.into(),
            tool: "search_text".into(),
            outcome: ExperienceOutcome::Success,
            duration_ms: 1_500,
            request_bytes: Some(10),
            response_bytes: Some(20),
            error_code: None,
            features: Some(
                json!({"calculate_total": true, "files_considered": files})
                    .as_object()
                    .unwrap()
                    .clone() as ExperienceFeatures,
            ),
            verification_outcome: Some(EvidenceOutcome::Passed),
            recovery_outcome: None,
            canary: None,
            operation_id: None,
            task_id: Some(format!("task-{id}")),
            conversation_context_id: None,
            skill: None,
            runtime_boot_id: None,
            timestamp_ms,
        }
    }

    #[test]
    fn candidate_is_shadowed_only_after_prior_batch_is_observed() {
        let root = tempdir().unwrap();
        let store = KnowledgeStore::new(root.path());
        let impact = KnowledgeImpactStore::new(root.path());
        let evaluator = ShadowEvaluator::new(store.clone(), impact.clone());
        let initial = (0..5)
            .map(|i| obs(&format!("seed-{i}"), 800, 1_000 + i))
            .collect::<Vec<_>>();
        let candidate = ExperienceCompiler.compile(&initial).unwrap().remove(0);
        store.upsert(candidate).unwrap();
        let before = store.list().unwrap();
        evaluator
            .evaluate(&[obs("next", 900, 2_000)], &before)
            .unwrap();
        assert_eq!(impact.list().unwrap()[0].matched, 1);
        assert_eq!(evaluator.activate_candidates(2_000).unwrap(), 1);
        assert_eq!(store.list().unwrap()[0].status, KnowledgeStatus::Shadow);
    }
}
