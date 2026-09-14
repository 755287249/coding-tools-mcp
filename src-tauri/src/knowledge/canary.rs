use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::impact::KnowledgeImpactStore;
use super::model::{
    ExperienceOutcome, KnowledgeArtifactTarget, KnowledgeStatus, KnowledgeTaskOutcome,
};
use super::persistence::write_private_atomic;
use super::store::{KnowledgeStore, KnowledgeStoreError};
use super::strategy_implementations::{
    resolve_tool_strategy_implementation, ToolStrategyImplementationId,
};

const CANARY_PERCENT: u32 = 25;
const MIN_INTERVENTION_CALLS: u64 = 10;
const MIN_CONTROL_CALLS: u64 = 5;
const MAX_DURATION_REGRESSION_RATIO: f64 = 1.25;
const MAX_FAILURE_RATE: f64 = 0.25;
const MAX_FAILURE_RATE_DELTA: f64 = 0.15;
const MIN_PROMOTION_INTERVENTION_CALLS: u64 = 20;
const MIN_PROMOTION_CONTROL_CALLS: u64 = 10;
const MIN_PROMOTION_TASK_OUTCOMES: u64 = 3;
const MIN_PROMOTION_VERIFIED_TASK_RATE: f64 = 0.8;
const MAX_PROMOTION_FAILURE_RATE: f64 = 0.1;
const MAX_PROMOTION_FAILURE_RATE_DELTA: f64 = 0.05;
const MAX_PROMOTION_DURATION_RATIO: f64 = 0.9;
const POST_PROMOTION_MIN_CALLS: u64 = 10;

#[derive(Debug, Error)]
pub enum CanaryError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Knowledge(#[from] KnowledgeStoreError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanaryStage {
    Canary,
    Promoted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCanaryDecision {
    pub knowledge_id: String,
    pub implementation: ToolStrategyImplementationId,
    pub eligible: bool,
    pub stage: CanaryStage,
    pub applied: bool,
    pub bucket: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanaryImpactRecord {
    pub schema_version: u8,
    pub knowledge_id: String,
    pub implementation: ToolStrategyImplementationId,
    pub control_event_ids: Vec<String>,
    pub intervention_event_ids: Vec<String>,
    pub control_task_ids: Vec<String>,
    pub intervention_task_ids: Vec<String>,
    pub credited_control_task_ids: Vec<String>,
    pub credited_intervention_task_ids: Vec<String>,
    pub control_calls: u64,
    pub intervention_calls: u64,
    pub control_successes: u64,
    pub control_failures: u64,
    pub intervention_successes: u64,
    pub intervention_failures: u64,
    pub control_duration_ms: u64,
    pub intervention_duration_ms: u64,
    pub control_task_verified_successes: u64,
    pub control_task_unverified_successes: u64,
    pub control_task_failures: u64,
    pub control_task_rollbacks: u64,
    pub intervention_task_verified_successes: u64,
    pub intervention_task_unverified_successes: u64,
    pub intervention_task_failures: u64,
    pub intervention_task_rollbacks: u64,
    pub first_seen_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub promoted_at_ms: Option<u64>,
    pub promotion_intervention_calls: Option<u64>,
    pub promotion_intervention_failures: Option<u64>,
    pub promotion_intervention_duration_ms: Option<u64>,
    pub promotion_intervention_task_verified_successes: Option<u64>,
    pub promotion_intervention_task_unverified_successes: Option<u64>,
    pub promotion_intervention_task_failures: Option<u64>,
    pub promotion_intervention_task_rollbacks: Option<u64>,
    pub rolled_back_at_ms: Option<u64>,
    pub rollback_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CanaryObservation {
    pub event_id: String,
    pub task_id: Option<String>,
    pub outcome: ExperienceOutcome,
    pub duration_ms: u64,
    pub timestamp_ms: u64,
    pub decision: KnowledgeCanaryDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CanaryImpactDocument {
    schema_version: u8,
    records: Vec<CanaryImpactRecord>,
}
impl Default for CanaryImpactDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: vec![],
        }
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn ratio(n: u64, d: u64) -> f64 {
    if d > 0 {
        n as f64 / d as f64
    } else {
        0.0
    }
}
fn task_outcomes(record: &CanaryImpactRecord, intervention: bool) -> u64 {
    if intervention {
        record.intervention_task_verified_successes
            + record.intervention_task_unverified_successes
            + record.intervention_task_failures
            + record.intervention_task_rollbacks
    } else {
        record.control_task_verified_successes
            + record.control_task_unverified_successes
            + record.control_task_failures
            + record.control_task_rollbacks
    }
}
pub(crate) fn canary_cohort_bucket(knowledge_id: &str, sample_key: &str) -> u32 {
    let digest = Sha256::digest(format!("{knowledge_id}\0{sample_key}").as_bytes());
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 100
}

fn stable_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(stable_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, child)| (key.clone(), stable_value(child)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct CanaryImpactStore {
    root_dir: PathBuf,
    file_path: PathBuf,
}
impl CanaryImpactStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        let file_path = root_dir.join("canary-impact.json");
        Self {
            root_dir,
            file_path,
        }
    }
    pub fn list(&self) -> Result<Vec<CanaryImpactRecord>, CanaryError> {
        Ok(self.read_document()?.records)
    }
    pub fn revision(&self) -> Result<String, CanaryError> {
        let value = serde_json::to_value(self.list()?)?;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&stable_value(&value))?)
        ))
    }
    pub fn record_observations(
        &self,
        observations: &[CanaryObservation],
    ) -> Result<usize, CanaryError> {
        let mut document = self.read_document()?;
        let mut recorded = 0;
        for observation in observations.iter().filter(|item| item.decision.eligible) {
            let decision = &observation.decision;
            let index = if let Some(index) = document
                .records
                .iter()
                .position(|item| item.knowledge_id == decision.knowledge_id)
            {
                index
            } else {
                document.records.push(CanaryImpactRecord {
                    schema_version: 1,
                    knowledge_id: decision.knowledge_id.clone(),
                    implementation: decision.implementation,
                    control_event_ids: vec![],
                    intervention_event_ids: vec![],
                    control_task_ids: vec![],
                    intervention_task_ids: vec![],
                    credited_control_task_ids: vec![],
                    credited_intervention_task_ids: vec![],
                    control_calls: 0,
                    intervention_calls: 0,
                    control_successes: 0,
                    control_failures: 0,
                    intervention_successes: 0,
                    intervention_failures: 0,
                    control_duration_ms: 0,
                    intervention_duration_ms: 0,
                    control_task_verified_successes: 0,
                    control_task_unverified_successes: 0,
                    control_task_failures: 0,
                    control_task_rollbacks: 0,
                    intervention_task_verified_successes: 0,
                    intervention_task_unverified_successes: 0,
                    intervention_task_failures: 0,
                    intervention_task_rollbacks: 0,
                    first_seen_at_ms: observation.timestamp_ms,
                    last_seen_at_ms: observation.timestamp_ms,
                    promoted_at_ms: None,
                    promotion_intervention_calls: None,
                    promotion_intervention_failures: None,
                    promotion_intervention_duration_ms: None,
                    promotion_intervention_task_verified_successes: None,
                    promotion_intervention_task_unverified_successes: None,
                    promotion_intervention_task_failures: None,
                    promotion_intervention_task_rollbacks: None,
                    rolled_back_at_ms: None,
                    rollback_reason: None,
                });
                document.records.len() - 1
            };
            let record = &mut document.records[index];
            let events = if decision.applied {
                &record.intervention_event_ids
            } else {
                &record.control_event_ids
            };
            if events.iter().any(|id| id == &observation.event_id) {
                continue;
            }
            if decision.applied {
                record.intervention_event_ids = unique(
                    record
                        .intervention_event_ids
                        .iter()
                        .cloned()
                        .chain(std::iter::once(observation.event_id.clone())),
                );
                record.intervention_calls += 1;
                if observation.outcome == ExperienceOutcome::Success {
                    record.intervention_successes += 1
                } else {
                    record.intervention_failures += 1
                };
                record.intervention_duration_ms += observation.duration_ms;
                if let Some(task) = &observation.task_id {
                    if !record.control_task_ids.contains(task)
                        && !record.intervention_task_ids.contains(task)
                    {
                        record.intervention_task_ids = unique(
                            record
                                .intervention_task_ids
                                .iter()
                                .cloned()
                                .chain(std::iter::once(task.clone())),
                        );
                    }
                }
            } else {
                record.control_event_ids = unique(
                    record
                        .control_event_ids
                        .iter()
                        .cloned()
                        .chain(std::iter::once(observation.event_id.clone())),
                );
                record.control_calls += 1;
                if observation.outcome == ExperienceOutcome::Success {
                    record.control_successes += 1
                } else {
                    record.control_failures += 1
                };
                record.control_duration_ms += observation.duration_ms;
                if let Some(task) = &observation.task_id {
                    if !record.control_task_ids.contains(task)
                        && !record.intervention_task_ids.contains(task)
                    {
                        record.control_task_ids = unique(
                            record
                                .control_task_ids
                                .iter()
                                .cloned()
                                .chain(std::iter::once(task.clone())),
                        );
                    }
                }
            }
            record.first_seen_at_ms = record.first_seen_at_ms.min(observation.timestamp_ms);
            record.last_seen_at_ms = record.last_seen_at_ms.max(observation.timestamp_ms);
            recorded += 1;
        }
        if recorded > 0 {
            document
                .records
                .sort_by(|a, b| a.knowledge_id.cmp(&b.knowledge_id));
            self.write_document(&document)?;
        }
        Ok(recorded)
    }
    pub fn credit_task_outcome(
        &self,
        task_id: &str,
        outcome: KnowledgeTaskOutcome,
        timestamp_ms: u64,
    ) -> Result<usize, CanaryError> {
        let mut d = self.read_document()?;
        let mut n = 0;
        for r in &mut d.records {
            for intervention in [false, true] {
                let tasks = if intervention {
                    &r.intervention_task_ids
                } else {
                    &r.control_task_ids
                };
                let credited = if intervention {
                    &r.credited_intervention_task_ids
                } else {
                    &r.credited_control_task_ids
                };
                if !tasks.iter().any(|x| x == task_id) || credited.iter().any(|x| x == task_id) {
                    continue;
                }
                if intervention {
                    r.credited_intervention_task_ids = unique(
                        r.credited_intervention_task_ids
                            .iter()
                            .cloned()
                            .chain(std::iter::once(task_id.into())),
                    )
                } else {
                    r.credited_control_task_ids = unique(
                        r.credited_control_task_ids
                            .iter()
                            .cloned()
                            .chain(std::iter::once(task_id.into())),
                    )
                };
                match (intervention, outcome) {
                    (false, KnowledgeTaskOutcome::VerifiedSuccess) => {
                        r.control_task_verified_successes += 1
                    }
                    (false, KnowledgeTaskOutcome::UnverifiedSuccess) => {
                        r.control_task_unverified_successes += 1
                    }
                    (false, KnowledgeTaskOutcome::Failure) => r.control_task_failures += 1,
                    (false, KnowledgeTaskOutcome::RolledBack) => r.control_task_rollbacks += 1,
                    (true, KnowledgeTaskOutcome::VerifiedSuccess) => {
                        r.intervention_task_verified_successes += 1
                    }
                    (true, KnowledgeTaskOutcome::UnverifiedSuccess) => {
                        r.intervention_task_unverified_successes += 1
                    }
                    (true, KnowledgeTaskOutcome::Failure) => r.intervention_task_failures += 1,
                    (true, KnowledgeTaskOutcome::RolledBack) => r.intervention_task_rollbacks += 1,
                };
                r.last_seen_at_ms = r.last_seen_at_ms.max(timestamp_ms);
                n += 1;
            }
        }
        if n > 0 {
            self.write_document(&d)?;
        }
        Ok(n)
    }
    pub fn mark_promoted(&self, id: &str, ts: u64) -> Result<bool, CanaryError> {
        let mut d = self.read_document()?;
        let Some(r) = d.records.iter_mut().find(|r| r.knowledge_id == id) else {
            return Ok(false);
        };
        if r.promoted_at_ms.is_some() || r.rolled_back_at_ms.is_some() {
            return Ok(false);
        };
        r.promoted_at_ms = Some(ts);
        r.promotion_intervention_calls = Some(r.intervention_calls);
        r.promotion_intervention_failures = Some(r.intervention_failures);
        r.promotion_intervention_duration_ms = Some(r.intervention_duration_ms);
        r.promotion_intervention_task_verified_successes =
            Some(r.intervention_task_verified_successes);
        r.promotion_intervention_task_unverified_successes =
            Some(r.intervention_task_unverified_successes);
        r.promotion_intervention_task_failures = Some(r.intervention_task_failures);
        r.promotion_intervention_task_rollbacks = Some(r.intervention_task_rollbacks);
        r.last_seen_at_ms = r.last_seen_at_ms.max(ts);
        self.write_document(&d)?;
        Ok(true)
    }
    pub fn mark_rolled_back(&self, id: &str, reason: &str, ts: u64) -> Result<bool, CanaryError> {
        let mut d = self.read_document()?;
        let Some(r) = d.records.iter_mut().find(|r| r.knowledge_id == id) else {
            return Ok(false);
        };
        if r.rolled_back_at_ms.is_some() {
            return Ok(false);
        };
        r.rolled_back_at_ms = Some(ts);
        r.rollback_reason = Some(reason.into());
        r.last_seen_at_ms = r.last_seen_at_ms.max(ts);
        self.write_document(&d)?;
        Ok(true)
    }
    fn read_document(&self) -> Result<CanaryImpactDocument, CanaryError> {
        match fs::read_to_string(&self.file_path) {
            Ok(c) => {
                let d: CanaryImpactDocument = serde_json::from_str(&c)?;
                if d.schema_version != 1 {
                    return Err(CanaryError::Validation(
                        "Unsupported canary impact schema.".into(),
                    ));
                }
                Ok(d)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(CanaryImpactDocument::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn write_document(&self, d: &CanaryImpactDocument) -> Result<(), CanaryError> {
        let contents = format!("{}\n", serde_json::to_string_pretty(d)?);
        write_private_atomic(&self.root_dir, &self.file_path, contents.as_bytes())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CanaryStrategyEngine {
    knowledge_store: KnowledgeStore,
    impact_store: CanaryImpactStore,
}
impl CanaryStrategyEngine {
    pub fn new(knowledge_store: KnowledgeStore, impact_store: CanaryImpactStore) -> Self {
        Self {
            knowledge_store,
            impact_store,
        }
    }
    pub fn decide(
        &self,
        tool: &str,
        args: &Value,
        sample_key: &str,
    ) -> Result<Option<KnowledgeCanaryDecision>, CanaryError> {
        let rolled = self
            .impact_store
            .list()?
            .into_iter()
            .filter(|r| r.rolled_back_at_ms.is_some())
            .map(|r| r.knowledge_id)
            .collect::<BTreeSet<_>>();
        for record in self.knowledge_store.list()?.into_iter().filter(|r| {
            r.target == KnowledgeArtifactTarget::ToolStrategy
                && matches!(
                    r.status,
                    KnowledgeStatus::Validated | KnowledgeStatus::Promoted
                )
        }) {
            if rolled.contains(&record.id) {
                continue;
            }
            let Some(spec) = resolve_tool_strategy_implementation(&record, tool, args) else {
                continue;
            };
            let bucket = canary_cohort_bucket(&record.id, sample_key);
            return Ok(Some(KnowledgeCanaryDecision {
                knowledge_id: record.id,
                implementation: spec.id,
                eligible: true,
                stage: if record.status == KnowledgeStatus::Promoted {
                    CanaryStage::Promoted
                } else {
                    CanaryStage::Canary
                },
                applied: record.status == KnowledgeStatus::Promoted || bucket < CANARY_PERCENT,
                bucket,
            }));
        }
        Ok(None)
    }
}

pub fn evaluate_canary_promotions(
    store: &KnowledgeStore,
    canary: &CanaryImpactStore,
    ts: u64,
) -> Result<(usize, usize), CanaryError> {
    let impacts = canary.list()?;
    let mut evaluated = 0;
    let mut promoted = 0;
    for impact in impacts {
        if impact.rolled_back_at_ms.is_some() || impact.promoted_at_ms.is_some() {
            continue;
        }
        let Some(record) = store.get(&impact.knowledge_id)? else {
            continue;
        };
        if record.target != KnowledgeArtifactTarget::ToolStrategy
            || record.status != KnowledgeStatus::Validated
            || impact.intervention_calls < MIN_PROMOTION_INTERVENTION_CALLS
            || impact.control_calls < MIN_PROMOTION_CONTROL_CALLS
        {
            continue;
        }
        evaluated += 1;
        let tasks = task_outcomes(&impact, true);
        if impact.intervention_task_rollbacks > 0
            || tasks < MIN_PROMOTION_TASK_OUTCOMES
            || ratio(impact.intervention_task_verified_successes, tasks)
                < MIN_PROMOTION_VERIFIED_TASK_RATE
        {
            continue;
        }
        let ir = ratio(impact.intervention_failures, impact.intervention_calls);
        let cr = ratio(impact.control_failures, impact.control_calls);
        if ir > MAX_PROMOTION_FAILURE_RATE || ir > cr + MAX_PROMOTION_FAILURE_RATE_DELTA {
            continue;
        }
        let ia = ratio(impact.intervention_duration_ms, impact.intervention_calls);
        let ca = ratio(impact.control_duration_ms, impact.control_calls);
        if ca <= 0.0 || ia > ca * MAX_PROMOTION_DURATION_RATIO {
            continue;
        }
        if store
            .transition_status(
                &record.id,
                &[KnowledgeStatus::Validated],
                KnowledgeStatus::Promoted,
                ts,
            )?
            .is_none()
        {
            continue;
        }
        match canary.mark_promoted(&record.id, ts) {
            Ok(true) => promoted += 1,
            Ok(false) => {
                let _ = store.transition_status(
                    &record.id,
                    &[KnowledgeStatus::Promoted],
                    KnowledgeStatus::Validated,
                    ts,
                )?;
            }
            Err(error) => {
                let _ = store.transition_status(
                    &record.id,
                    &[KnowledgeStatus::Promoted],
                    KnowledgeStatus::Validated,
                    ts,
                );
                return Err(error);
            }
        }
    }
    Ok((evaluated, promoted))
}

pub fn evaluate_canary_rollbacks(
    store: &KnowledgeStore,
    shadow: &KnowledgeImpactStore,
    canary: &CanaryImpactStore,
    ts: u64,
) -> Result<(usize, usize), CanaryError> {
    let baselines = shadow
        .list()
        .map_err(|e| CanaryError::Validation(e.to_string()))?;
    let mut evaluated = 0;
    let mut rolled = 0;
    for impact in canary.list()? {
        if impact.rolled_back_at_ms.is_some() {
            continue;
        }
        let Some(record) = store.get(&impact.knowledge_id)? else {
            continue;
        };
        if record.target != KnowledgeArtifactTarget::ToolStrategy
            || !matches!(
                record.status,
                KnowledgeStatus::Validated | KnowledgeStatus::Promoted
            )
            || impact.intervention_calls == 0
        {
            continue;
        }
        evaluated += 1;
        let baseline = baselines
            .iter()
            .find(|x| x.knowledge_id == impact.knowledge_id);
        let baseline_avg = if impact.control_calls >= MIN_CONTROL_CALLS {
            ratio(impact.control_duration_ms, impact.control_calls)
        } else {
            baseline
                .map(|x| ratio(x.total_observed_duration_ms, x.matched))
                .unwrap_or(0.0)
        };
        let control_failure = ratio(impact.control_failures, impact.control_calls);
        let mut reason = None;
        if record.status == KnowledgeStatus::Promoted {
            let calls = impact.intervention_calls.saturating_sub(
                impact
                    .promotion_intervention_calls
                    .unwrap_or(impact.intervention_calls),
            );
            let failures = impact.intervention_failures.saturating_sub(
                impact
                    .promotion_intervention_failures
                    .unwrap_or(impact.intervention_failures),
            );
            let duration = impact.intervention_duration_ms.saturating_sub(
                impact
                    .promotion_intervention_duration_ms
                    .unwrap_or(impact.intervention_duration_ms),
            );
            let task_failures = impact.intervention_task_failures.saturating_sub(
                impact
                    .promotion_intervention_task_failures
                    .unwrap_or(impact.intervention_task_failures),
            );
            let task_rollbacks = impact.intervention_task_rollbacks.saturating_sub(
                impact
                    .promotion_intervention_task_rollbacks
                    .unwrap_or(impact.intervention_task_rollbacks),
            );
            let verified = impact.intervention_task_verified_successes.saturating_sub(
                impact
                    .promotion_intervention_task_verified_successes
                    .unwrap_or(impact.intervention_task_verified_successes),
            );
            let unverified = impact
                .intervention_task_unverified_successes
                .saturating_sub(
                    impact
                        .promotion_intervention_task_unverified_successes
                        .unwrap_or(impact.intervention_task_unverified_successes),
                );
            if task_rollbacks > 0 {
                reason = Some("promoted_task_rollback")
            } else if calls >= POST_PROMOTION_MIN_CALLS {
                if ratio(failures, calls)
                    > MAX_FAILURE_RATE.max(control_failure + MAX_FAILURE_RATE_DELTA)
                {
                    reason = Some("promoted_failure_rate_regression")
                } else if baseline_avg > 0.0
                    && ratio(duration, calls) > baseline_avg * MAX_DURATION_REGRESSION_RATIO
                {
                    reason = Some("promoted_duration_regression")
                } else if verified + unverified + task_failures + task_rollbacks >= 3
                    && ratio(
                        task_failures,
                        verified + unverified + task_failures + task_rollbacks,
                    ) > 0.3
                {
                    reason = Some("promoted_task_failure_regression")
                }
            }
        } else if impact.intervention_task_rollbacks > 0 {
            reason = Some("intervention_task_rollback")
        } else if impact.intervention_calls >= MIN_INTERVENTION_CALLS {
            let ir = ratio(impact.intervention_failures, impact.intervention_calls);
            if ir > MAX_FAILURE_RATE.max(control_failure + MAX_FAILURE_RATE_DELTA) {
                reason = Some("intervention_failure_rate_regression")
            } else if baseline_avg > 0.0
                && ratio(impact.intervention_duration_ms, impact.intervention_calls)
                    > baseline_avg * MAX_DURATION_REGRESSION_RATIO
            {
                reason = Some("intervention_duration_regression")
            } else if task_outcomes(&impact, true) >= 3
                && ratio(
                    impact.intervention_task_failures,
                    task_outcomes(&impact, true),
                ) > 0.3
            {
                reason = Some("intervention_task_failure_regression")
            }
        }
        if let Some(reason) = reason {
            if store
                .transition_status(&record.id, &[record.status], KnowledgeStatus::Rejected, ts)?
                .is_none()
            {
                continue;
            }
            match canary.mark_rolled_back(&record.id, reason, ts) {
                Ok(true) => rolled += 1,
                Ok(false) => {
                    let _ = store.transition_status(
                        &record.id,
                        &[KnowledgeStatus::Rejected],
                        record.status,
                        ts,
                    )?;
                }
                Err(error) => {
                    let _ = store.transition_status(
                        &record.id,
                        &[KnowledgeStatus::Rejected],
                        record.status,
                        ts,
                    );
                    return Err(error);
                }
            }
        }
    }
    Ok((evaluated, rolled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeEvidence, KnowledgeRecord, KnowledgeScope};
    use serde_json::json;
    use tempfile::tempdir;
    fn strategy(status: KnowledgeStatus) -> KnowledgeRecord {
        KnowledgeRecord {
            id: String::new(),
            schema_version: 1,
            scope: KnowledgeScope::Workspace,
            target: KnowledgeArtifactTarget::ToolStrategy,
            status,
            trigger: json!({"tool":"search_text"}).as_object().unwrap().clone(),
            hypothesis: "fast tail".into(),
            recommended_action:
                json!({"semantic_preserving_implementation":"search_exact_total_fast_tail"})
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
                total_duration_ms: 10000,
                total_request_bytes: 0,
                total_response_bytes: 0,
                first_seen_at_ms: 1,
                last_seen_at_ms: 2,
                task_context_ids: None,
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
    #[test]
    fn canary_decision_is_deterministic_and_promoted_is_always_applied() {
        let d = tempdir().unwrap();
        let store = KnowledgeStore::new(d.path());
        let stored = store.upsert(strategy(KnowledgeStatus::Validated)).unwrap();
        let impact = CanaryImpactStore::new(d.path());
        let engine = CanaryStrategyEngine::new(store.clone(), impact);
        let a = engine
            .decide(
                "search_text",
                &json!({"query":"x","calculate_total":true}),
                "session-a",
            )
            .unwrap()
            .unwrap();
        let b = engine
            .decide(
                "search_text",
                &json!({"query":"x","calculate_total":true}),
                "session-a",
            )
            .unwrap()
            .unwrap();
        assert_eq!(a, b);
        store
            .transition_status(
                &stored.id,
                &[KnowledgeStatus::Validated],
                KnowledgeStatus::Promoted,
                5,
            )
            .unwrap();
        let promoted = engine
            .decide(
                "search_text",
                &json!({"query":"x","calculate_total":true}),
                "session-a",
            )
            .unwrap()
            .unwrap();
        assert!(promoted.applied);
        assert_eq!(promoted.stage, CanaryStage::Promoted)
    }
    #[test]
    fn promotion_marker_write_failure_restores_validated_status() {
        let d = tempdir().unwrap();
        let store = KnowledgeStore::new(d.path());
        let stored = store.upsert(strategy(KnowledgeStatus::Validated)).unwrap();
        let impact = CanaryImpactStore::new(d.path());

        let intervention = KnowledgeCanaryDecision {
            knowledge_id: stored.id.clone(),
            implementation: ToolStrategyImplementationId::SearchExactTotalFastTail,
            eligible: true,
            stage: CanaryStage::Canary,
            applied: true,
            bucket: 1,
        };
        let control = KnowledgeCanaryDecision {
            applied: false,
            bucket: 50,
            ..intervention.clone()
        };
        let mut observations = Vec::new();
        for index in 0..20 {
            observations.push(CanaryObservation {
                event_id: format!("intervention-{index}"),
                task_id: (index < 3).then(|| format!("task-{index}")),
                outcome: ExperienceOutcome::Success,
                duration_ms: 50,
                timestamp_ms: 10 + index,
                decision: intervention.clone(),
            });
        }
        for index in 0..10 {
            observations.push(CanaryObservation {
                event_id: format!("control-{index}"),
                task_id: None,
                outcome: ExperienceOutcome::Success,
                duration_ms: 100,
                timestamp_ms: 40 + index,
                decision: control.clone(),
            });
        }
        impact.record_observations(&observations).unwrap();
        for index in 0..3 {
            impact
                .credit_task_outcome(
                    &format!("task-{index}"),
                    KnowledgeTaskOutcome::VerifiedSuccess,
                    60 + index,
                )
                .unwrap();
        }

        std::fs::create_dir(d.path().join("canary-impact.json.tmp")).unwrap();
        assert!(evaluate_canary_promotions(&store, &impact, 100).is_err());
        assert_eq!(
            store.get(&stored.id).unwrap().unwrap().status,
            KnowledgeStatus::Validated
        );
    }

    #[test]
    fn rollback_marker_write_failure_restores_original_status() {
        let d = tempdir().unwrap();
        let store = KnowledgeStore::new(d.path());
        let stored = store.upsert(strategy(KnowledgeStatus::Validated)).unwrap();
        let shadow = KnowledgeImpactStore::new(d.path());
        let impact = CanaryImpactStore::new(d.path());
        let decision = KnowledgeCanaryDecision {
            knowledge_id: stored.id.clone(),
            implementation: ToolStrategyImplementationId::SearchExactTotalFastTail,
            eligible: true,
            stage: CanaryStage::Canary,
            applied: true,
            bucket: 1,
        };
        impact
            .record_observations(&[CanaryObservation {
                event_id: "rollback-event".into(),
                task_id: Some("rollback-task".into()),
                outcome: ExperienceOutcome::Success,
                duration_ms: 100,
                timestamp_ms: 10,
                decision,
            }])
            .unwrap();
        impact
            .credit_task_outcome("rollback-task", KnowledgeTaskOutcome::RolledBack, 11)
            .unwrap();

        std::fs::create_dir(d.path().join("canary-impact.json.tmp")).unwrap();
        assert!(evaluate_canary_rollbacks(&store, &shadow, &impact, 12).is_err());
        assert_eq!(
            store.get(&stored.id).unwrap().unwrap().status,
            KnowledgeStatus::Validated
        );
    }

    #[test]
    fn canary_impact_replay_and_task_credit_are_idempotent() {
        let d = tempdir().unwrap();
        let impact = CanaryImpactStore::new(d.path());
        let decision = KnowledgeCanaryDecision {
            knowledge_id: "k".into(),
            implementation: ToolStrategyImplementationId::SearchExactTotalFastTail,
            eligible: true,
            stage: CanaryStage::Canary,
            applied: true,
            bucket: 1,
        };
        let obs = CanaryObservation {
            event_id: "e".into(),
            task_id: Some("t".into()),
            outcome: ExperienceOutcome::Success,
            duration_ms: 100,
            timestamp_ms: 1,
            decision,
        };
        assert_eq!(
            impact
                .record_observations(std::slice::from_ref(&obs))
                .unwrap(),
            1
        );
        assert_eq!(impact.record_observations(&[obs]).unwrap(), 0);
        assert_eq!(
            impact
                .credit_task_outcome("t", KnowledgeTaskOutcome::VerifiedSuccess, 2)
                .unwrap(),
            1
        );
        assert_eq!(
            impact
                .credit_task_outcome("t", KnowledgeTaskOutcome::Failure, 3)
                .unwrap(),
            0
        )
    }
}
