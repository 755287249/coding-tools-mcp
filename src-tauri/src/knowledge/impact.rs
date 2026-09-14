use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::model::{KnowledgeArtifactTarget, KnowledgeTaskOutcome};
use super::persistence::write_private_atomic;

#[derive(Debug, Error)]
pub enum KnowledgeImpactError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeImpactRecord {
    pub schema_version: u8,
    pub knowledge_id: String,
    pub target: KnowledgeArtifactTarget,
    pub tool: String,
    pub source_event_ids: Vec<String>,
    pub matched_task_ids: Vec<String>,
    pub credited_task_ids: Vec<String>,
    pub considered: u64,
    pub matched: u64,
    pub actual_successes: u64,
    pub actual_failures: u64,
    pub verified_successes: u64,
    pub verification_failures: u64,
    pub recovery_successes: u64,
    pub recovery_failures: u64,
    pub task_verified_successes: u64,
    pub task_unverified_successes: u64,
    pub task_failures: u64,
    pub task_rollbacks: u64,
    pub total_observed_duration_ms: u64,
    pub total_observed_request_bytes: u64,
    pub total_observed_response_bytes: u64,
    pub first_seen_at_ms: u64,
    pub last_seen_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgeImpactDocument {
    schema_version: u8,
    records: Vec<KnowledgeImpactRecord>,
}

impl Default for KnowledgeImpactDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: Vec::new(),
        }
    }
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

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize(mut record: KnowledgeImpactRecord) -> KnowledgeImpactRecord {
    record.source_event_ids = unique(record.source_event_ids);
    record.matched_task_ids = unique(record.matched_task_ids);
    record.credited_task_ids = unique(record.credited_task_ids);
    record
}

fn merge(left: &KnowledgeImpactRecord, right: &KnowledgeImpactRecord) -> KnowledgeImpactRecord {
    KnowledgeImpactRecord {
        schema_version: 1,
        knowledge_id: right.knowledge_id.clone(),
        target: right.target,
        tool: right.tool.clone(),
        source_event_ids: unique(
            left.source_event_ids
                .iter()
                .chain(&right.source_event_ids)
                .cloned(),
        ),
        matched_task_ids: unique(
            left.matched_task_ids
                .iter()
                .chain(&right.matched_task_ids)
                .cloned(),
        ),
        credited_task_ids: unique(
            left.credited_task_ids
                .iter()
                .chain(&right.credited_task_ids)
                .cloned(),
        ),
        considered: left.considered.saturating_add(right.considered),
        matched: left.matched.saturating_add(right.matched),
        actual_successes: left.actual_successes.saturating_add(right.actual_successes),
        actual_failures: left.actual_failures.saturating_add(right.actual_failures),
        verified_successes: left
            .verified_successes
            .saturating_add(right.verified_successes),
        verification_failures: left
            .verification_failures
            .saturating_add(right.verification_failures),
        recovery_successes: left
            .recovery_successes
            .saturating_add(right.recovery_successes),
        recovery_failures: left
            .recovery_failures
            .saturating_add(right.recovery_failures),
        task_verified_successes: left
            .task_verified_successes
            .saturating_add(right.task_verified_successes),
        task_unverified_successes: left
            .task_unverified_successes
            .saturating_add(right.task_unverified_successes),
        task_failures: left.task_failures.saturating_add(right.task_failures),
        task_rollbacks: left.task_rollbacks.saturating_add(right.task_rollbacks),
        total_observed_duration_ms: left
            .total_observed_duration_ms
            .saturating_add(right.total_observed_duration_ms),
        total_observed_request_bytes: left
            .total_observed_request_bytes
            .saturating_add(right.total_observed_request_bytes),
        total_observed_response_bytes: left
            .total_observed_response_bytes
            .saturating_add(right.total_observed_response_bytes),
        first_seen_at_ms: left.first_seen_at_ms.min(right.first_seen_at_ms),
        last_seen_at_ms: left.last_seen_at_ms.max(right.last_seen_at_ms),
    }
}

#[derive(Debug, Clone)]
pub struct KnowledgeImpactStore {
    root_dir: PathBuf,
    file_path: PathBuf,
}

impl KnowledgeImpactStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        Self {
            file_path: root_dir.join("impact.json"),
            root_dir,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root_dir
    }

    pub fn list(&self) -> Result<Vec<KnowledgeImpactRecord>, KnowledgeImpactError> {
        Ok(self
            .read_document()?
            .records
            .into_iter()
            .map(normalize)
            .collect())
    }

    pub fn record(
        &self,
        delta: KnowledgeImpactRecord,
    ) -> Result<KnowledgeImpactRecord, KnowledgeImpactError> {
        let mut document = self.read_document()?;
        let delta = normalize(delta);
        if let Some(index) = document
            .records
            .iter()
            .position(|item| item.knowledge_id == delta.knowledge_id)
        {
            let existing = normalize(document.records[index].clone());
            let existing_events = existing
                .source_event_ids
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let incoming_events = delta
                .source_event_ids
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let overlapping = incoming_events.intersection(&existing_events).count();
            let novel = incoming_events.difference(&existing_events).count();
            if !incoming_events.is_empty() && novel == 0 {
                return Ok(existing);
            }
            if overlapping > 0 {
                return Err(KnowledgeImpactError::Validation(
                    "Shadow impact batches must be disjoint or exact replays; partial overlap would double-count impact.".into(),
                ));
            }
            document.records[index] = merge(&existing, &delta);
        } else {
            document.records.push(delta.clone());
        }
        document
            .records
            .sort_by(|left, right| left.knowledge_id.cmp(&right.knowledge_id));
        self.write_document(&document)?;
        Ok(document
            .records
            .iter()
            .find(|item| item.knowledge_id == delta.knowledge_id)
            .cloned()
            .expect("impact was stored"))
    }

    pub fn credit_task_outcome(
        &self,
        task_id: &str,
        outcome: KnowledgeTaskOutcome,
        timestamp_ms: u64,
    ) -> Result<usize, KnowledgeImpactError> {
        let mut document = self.read_document()?;
        let mut credited = 0;
        for record in &mut document.records {
            if !record.matched_task_ids.iter().any(|item| item == task_id)
                || record.credited_task_ids.iter().any(|item| item == task_id)
            {
                continue;
            }
            record.credited_task_ids = unique(
                record
                    .credited_task_ids
                    .iter()
                    .cloned()
                    .chain(std::iter::once(task_id.to_string())),
            );
            match outcome {
                KnowledgeTaskOutcome::VerifiedSuccess => record.task_verified_successes += 1,
                KnowledgeTaskOutcome::UnverifiedSuccess => record.task_unverified_successes += 1,
                KnowledgeTaskOutcome::Failure => record.task_failures += 1,
                KnowledgeTaskOutcome::RolledBack => record.task_rollbacks += 1,
            }
            record.last_seen_at_ms = record.last_seen_at_ms.max(timestamp_ms);
            credited += 1;
        }
        if credited > 0 {
            self.write_document(&document)?;
        }
        Ok(credited)
    }

    pub fn revision(&self) -> Result<String, KnowledgeImpactError> {
        let value = serde_json::to_value(self.list()?)?;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&stable_value(&value))?)
        ))
    }

    fn read_document(&self) -> Result<KnowledgeImpactDocument, KnowledgeImpactError> {
        match fs::read_to_string(&self.file_path) {
            Ok(content) => {
                let document: KnowledgeImpactDocument = serde_json::from_str(&content)?;
                if document.schema_version != 1 {
                    return Err(KnowledgeImpactError::Validation(
                        "Unsupported knowledge impact schema.".into(),
                    ));
                }
                Ok(document)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(KnowledgeImpactDocument::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn write_document(
        &self,
        document: &KnowledgeImpactDocument,
    ) -> Result<(), KnowledgeImpactError> {
        let contents = format!("{}\n", serde_json::to_string_pretty(document)?);
        write_private_atomic(&self.root_dir, &self.file_path, contents.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn impact(events: &[&str]) -> KnowledgeImpactRecord {
        KnowledgeImpactRecord {
            schema_version: 1,
            knowledge_id: "knowledge-a".into(),
            target: KnowledgeArtifactTarget::ToolStrategy,
            tool: "search_text".into(),
            source_event_ids: events.iter().map(|item| item.to_string()).collect(),
            matched_task_ids: vec!["task-a".into()],
            credited_task_ids: vec![],
            considered: events.len() as u64,
            matched: events.len() as u64,
            actual_successes: events.len() as u64,
            actual_failures: 0,
            verified_successes: 0,
            verification_failures: 0,
            recovery_successes: 0,
            recovery_failures: 0,
            task_verified_successes: 0,
            task_unverified_successes: 0,
            task_failures: 0,
            task_rollbacks: 0,
            total_observed_duration_ms: 1_000,
            total_observed_request_bytes: 10,
            total_observed_response_bytes: 20,
            first_seen_at_ms: 1,
            last_seen_at_ms: 2,
        }
    }

    #[test]
    fn impact_replay_is_idempotent_and_partial_overlap_is_rejected() {
        let root = tempdir().unwrap();
        let store = KnowledgeImpactStore::new(root.path());
        store.record(impact(&["a", "b"])).unwrap();
        assert_eq!(store.record(impact(&["a", "b"])).unwrap().matched, 2);
        assert!(store.record(impact(&["b", "c"])).is_err());
        assert_eq!(store.record(impact(&["c", "d"])).unwrap().matched, 4);
    }

    #[test]
    fn task_outcomes_are_credited_once() {
        let root = tempdir().unwrap();
        let store = KnowledgeImpactStore::new(root.path());
        store.record(impact(&["a"])).unwrap();
        assert_eq!(
            store
                .credit_task_outcome("task-a", KnowledgeTaskOutcome::VerifiedSuccess, 5)
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .credit_task_outcome("task-a", KnowledgeTaskOutcome::Failure, 6)
                .unwrap(),
            0
        );
        assert_eq!(store.list().unwrap()[0].task_verified_successes, 1);
    }
}
