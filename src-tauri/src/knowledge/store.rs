use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::model::{KnowledgeArtifactTarget, KnowledgeEvidence, KnowledgeRecord, KnowledgeStatus};
use super::persistence::write_private_atomic;

const MAX_EVIDENCE_CONTEXT_IDS: usize = 256;

#[derive(Debug, Error)]
pub enum KnowledgeStoreError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgeDocument {
    schema_version: u8,
    records: Vec<KnowledgeRecord>,
}

impl Default for KnowledgeDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KnowledgeStore {
    root_dir: PathBuf,
    file_path: PathBuf,
}

fn stable_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(stable_value).collect()),
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, child)| (key.clone(), stable_value(child)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        _ => value.clone(),
    }
}

fn stable_json(value: &Value) -> Result<String, KnowledgeStoreError> {
    Ok(serde_json::to_string(&stable_value(value))?)
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn valid_context_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn context_ids(
    values: Option<&[String]>,
    label: &str,
) -> Result<Option<Vec<String>>, KnowledgeStoreError> {
    let Some(values) = values else {
        return Ok(None);
    };
    if values.iter().any(|value| !valid_context_id(value)) {
        return Err(KnowledgeStoreError::Validation(format!(
            "{label} must contain only SHA-256 context identifiers."
        )));
    }
    let normalized = unique(values.iter().cloned())
        .into_iter()
        .take(MAX_EVIDENCE_CONTEXT_IDS)
        .collect::<Vec<_>>();
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn normalized_evidence(
    evidence: &KnowledgeEvidence,
) -> Result<KnowledgeEvidence, KnowledgeStoreError> {
    let mut normalized = evidence.clone();
    normalized.task_context_ids =
        context_ids(evidence.task_context_ids.as_deref(), "taskContextIds")?;
    normalized.conversation_context_ids = context_ids(
        evidence.conversation_context_ids.as_deref(),
        "conversationContextIds",
    )?;
    normalized.runtime_boot_context_ids = context_ids(
        evidence.runtime_boot_context_ids.as_deref(),
        "runtimeBootContextIds",
    )?;
    Ok(normalized)
}

pub fn knowledge_evidence_candidate_ready(
    target: KnowledgeArtifactTarget,
    evidence: &KnowledgeEvidence,
) -> bool {
    if evidence.observations < 5 {
        return false;
    }
    if !matches!(
        target,
        KnowledgeArtifactTarget::ToolEvolution | KnowledgeArtifactTarget::EvolvedSkill
    ) {
        return true;
    }
    evidence.task_context_ids.as_ref().map_or(0, Vec::len) >= 3
        || evidence
            .conversation_context_ids
            .as_ref()
            .map_or(0, Vec::len)
            >= 2
        || evidence
            .runtime_boot_context_ids
            .as_ref()
            .map_or(0, Vec::len)
            >= 2
}

fn merge_evidence(
    left: &KnowledgeEvidence,
    right: &KnowledgeEvidence,
) -> Result<KnowledgeEvidence, KnowledgeStoreError> {
    let left = normalized_evidence(left)?;
    let right = normalized_evidence(right)?;
    Ok(KnowledgeEvidence {
        observations: left.observations.saturating_add(right.observations),
        successes: left.successes.saturating_add(right.successes),
        failures: left.failures.saturating_add(right.failures),
        verified_successes: Some(
            left.verified_successes
                .unwrap_or(0)
                .saturating_add(right.verified_successes.unwrap_or(0)),
        ),
        verification_failures: Some(
            left.verification_failures
                .unwrap_or(0)
                .saturating_add(right.verification_failures.unwrap_or(0)),
        ),
        recovery_successes: Some(
            left.recovery_successes
                .unwrap_or(0)
                .saturating_add(right.recovery_successes.unwrap_or(0)),
        ),
        recovery_failures: Some(
            left.recovery_failures
                .unwrap_or(0)
                .saturating_add(right.recovery_failures.unwrap_or(0)),
        ),
        total_duration_ms: left
            .total_duration_ms
            .saturating_add(right.total_duration_ms),
        total_request_bytes: left
            .total_request_bytes
            .saturating_add(right.total_request_bytes),
        total_response_bytes: left
            .total_response_bytes
            .saturating_add(right.total_response_bytes),
        first_seen_at_ms: left.first_seen_at_ms.min(right.first_seen_at_ms),
        last_seen_at_ms: left.last_seen_at_ms.max(right.last_seen_at_ms),
        task_context_ids: context_ids(
            Some(
                &left
                    .task_context_ids
                    .unwrap_or_default()
                    .into_iter()
                    .chain(right.task_context_ids.unwrap_or_default())
                    .collect::<Vec<_>>(),
            ),
            "taskContextIds",
        )?,
        conversation_context_ids: context_ids(
            Some(
                &left
                    .conversation_context_ids
                    .unwrap_or_default()
                    .into_iter()
                    .chain(right.conversation_context_ids.unwrap_or_default())
                    .collect::<Vec<_>>(),
            ),
            "conversationContextIds",
        )?,
        runtime_boot_context_ids: context_ids(
            Some(
                &left
                    .runtime_boot_context_ids
                    .unwrap_or_default()
                    .into_iter()
                    .chain(right.runtime_boot_context_ids.unwrap_or_default())
                    .collect::<Vec<_>>(),
            ),
            "runtimeBootContextIds",
        )?,
    })
}

fn automatic_status(status: KnowledgeStatus) -> bool {
    matches!(
        status,
        KnowledgeStatus::Observed | KnowledgeStatus::Candidate
    )
}

fn evidence_tracking_status(status: KnowledgeStatus) -> bool {
    automatic_status(status) || status == KnowledgeStatus::Shadow
}

fn support_confidence(observations: u64) -> f64 {
    if observations == 0 {
        return 0.0;
    }
    let value = observations as f64 / (observations as f64 + 5.0);
    (value * 10_000.0).round() / 10_000.0
}

fn record_identity_value(record: &KnowledgeRecord) -> Value {
    serde_json::json!({
        "scope": record.scope,
        "target": record.target,
        "trigger": record.trigger,
        "hypothesis": record.hypothesis,
        "recommendedAction": record.recommended_action,
    })
}

pub fn knowledge_record_id(record: &KnowledgeRecord) -> Result<String, KnowledgeStoreError> {
    let canonical = stable_json(&record_identity_value(record))?;
    Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
}

impl KnowledgeStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        let file_path = root_dir.join("knowledge.json");
        Self {
            root_dir,
            file_path,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root_dir
    }

    pub fn list(&self) -> Result<Vec<KnowledgeRecord>, KnowledgeStoreError> {
        Ok(self.read_document()?.records)
    }

    pub fn get(&self, id: &str) -> Result<Option<KnowledgeRecord>, KnowledgeStoreError> {
        Ok(self.list()?.into_iter().find(|record| record.id == id))
    }

    pub fn upsert(
        &self,
        mut record: KnowledgeRecord,
    ) -> Result<KnowledgeRecord, KnowledgeStoreError> {
        if record.schema_version != 1 {
            return Err(KnowledgeStoreError::Validation(
                "Unsupported knowledge record schema.".into(),
            ));
        }
        let canonical_id = knowledge_record_id(&record)?;
        if !record.id.is_empty() && record.id != canonical_id {
            return Err(KnowledgeStoreError::Validation(format!(
                "Knowledge record id mismatch: expected {canonical_id}"
            )));
        }
        record.id = canonical_id.clone();
        record.evidence = normalized_evidence(&record.evidence)?;
        let mut document = self.read_document()?;
        if let Some(index) = document
            .records
            .iter()
            .position(|existing| existing.id == canonical_id)
        {
            let existing = document.records[index].clone();
            let existing_events = existing
                .source_event_ids
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let incoming_events = record
                .source_event_ids
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let overlapping = incoming_events
                .intersection(&existing_events)
                .cloned()
                .collect::<Vec<_>>();
            let new_events = incoming_events
                .difference(&existing_events)
                .cloned()
                .collect::<Vec<_>>();
            let incoming_is_replay = !incoming_events.is_empty() && new_events.is_empty();
            if !overlapping.is_empty() && !new_events.is_empty() {
                return Err(KnowledgeStoreError::Validation(
                    "Knowledge evidence batches must be disjoint or exact replays; partial overlap would double-count evidence.".into(),
                ));
            }
            let evidence = if incoming_is_replay {
                normalized_evidence(&existing.evidence)?
            } else {
                merge_evidence(&existing.evidence, &record.evidence)?
            };
            let incoming_automatic = automatic_status(record.status);
            let existing_automatic = automatic_status(existing.status);
            let status = if !incoming_automatic {
                record.status
            } else if existing.status == KnowledgeStatus::Shadow {
                KnowledgeStatus::Shadow
            } else if !existing_automatic {
                existing.status
            } else if knowledge_evidence_candidate_ready(record.target, &evidence) {
                KnowledgeStatus::Candidate
            } else {
                KnowledgeStatus::Observed
            };
            let confidence = if incoming_automatic && evidence_tracking_status(existing.status) {
                support_confidence(evidence.observations)
            } else if !incoming_automatic {
                record.confidence
            } else {
                existing.confidence
            };
            let supersedes = unique(
                existing
                    .supersedes
                    .unwrap_or_default()
                    .into_iter()
                    .chain(record.supersedes.unwrap_or_default()),
            );
            document.records[index] = KnowledgeRecord {
                id: canonical_id.clone(),
                schema_version: record.schema_version,
                scope: record.scope,
                target: record.target,
                status,
                trigger: record.trigger,
                hypothesis: record.hypothesis,
                recommended_action: record.recommended_action,
                evidence,
                confidence,
                compatibility: record.compatibility.or(existing.compatibility),
                source_event_ids: unique(
                    existing
                        .source_event_ids
                        .into_iter()
                        .chain(record.source_event_ids),
                ),
                counterexample_event_ids: unique(
                    existing
                        .counterexample_event_ids
                        .into_iter()
                        .chain(record.counterexample_event_ids),
                ),
                supersedes: (!supersedes.is_empty()).then_some(supersedes),
                superseded_by: record.superseded_by.or(existing.superseded_by),
                created_at_ms: existing.created_at_ms.min(record.created_at_ms),
                updated_at_ms: existing.updated_at_ms.max(record.updated_at_ms),
                metadata: record.metadata.or(existing.metadata),
            };
        } else {
            if record.target == KnowledgeArtifactTarget::ToolEvolution
                && automatic_status(record.status)
            {
                record.status =
                    if knowledge_evidence_candidate_ready(record.target, &record.evidence) {
                        KnowledgeStatus::Candidate
                    } else {
                        KnowledgeStatus::Observed
                    };
            }
            document.records.push(record);
        }
        document
            .records
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.write_document(&document)?;
        Ok(document
            .records
            .into_iter()
            .find(|record| record.id == canonical_id)
            .expect("upserted knowledge record must exist"))
    }

    pub fn transition_status(
        &self,
        id: &str,
        allowed_from: &[KnowledgeStatus],
        next: KnowledgeStatus,
        updated_at_ms: u64,
    ) -> Result<Option<KnowledgeRecord>, KnowledgeStoreError> {
        let mut document = self.read_document()?;
        let Some(index) = document.records.iter().position(|record| record.id == id) else {
            return Ok(None);
        };
        if !allowed_from.contains(&document.records[index].status) {
            return Ok(None);
        }
        document.records[index].status = next;
        document.records[index].updated_at_ms =
            document.records[index].updated_at_ms.max(updated_at_ms);
        let updated = document.records[index].clone();
        self.write_document(&document)?;
        Ok(Some(updated))
    }

    pub fn revision(&self) -> Result<String, KnowledgeStoreError> {
        let records = serde_json::to_value(self.list()?)?;
        let canonical = stable_json(&records)?;
        Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
    }

    fn read_document(&self) -> Result<KnowledgeDocument, KnowledgeStoreError> {
        match fs::read_to_string(&self.file_path) {
            Ok(contents) => {
                let document = serde_json::from_str::<KnowledgeDocument>(&contents)?;
                if document.schema_version != 1 {
                    return Err(KnowledgeStoreError::Validation(
                        "Unsupported knowledge store schema.".into(),
                    ));
                }
                Ok(document)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(KnowledgeDocument::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn write_document(&self, document: &KnowledgeDocument) -> Result<(), KnowledgeStoreError> {
        let contents = format!("{}\n", serde_json::to_string_pretty(document)?);
        write_private_atomic(&self.root_dir, &self.file_path, contents.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::model::{KnowledgeScope, KnowledgeStatus};
    use serde_json::{json, Map};
    use tempfile::tempdir;

    fn context_id(seed: &str) -> String {
        format!("{:x}", Sha256::digest(seed.as_bytes()))
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("object fixture")
    }

    fn record() -> KnowledgeRecord {
        KnowledgeRecord {
            id: String::new(),
            schema_version: 1,
            scope: KnowledgeScope::Runtime,
            target: KnowledgeArtifactTarget::ToolEvolution,
            status: KnowledgeStatus::Candidate,
            trigger: object(json!({"tool": "project_state", "variant": "rust-parity"})),
            hypothesis: "Repeated baseline capture should evolve the MCP implementation.".into(),
            recommended_action: object(json!({"proposal": "incremental_or_cached_baseline"})),
            evidence: KnowledgeEvidence {
                observations: 5,
                successes: 5,
                failures: 0,
                verified_successes: None,
                verification_failures: None,
                recovery_successes: None,
                recovery_failures: None,
                total_duration_ms: 50_000,
                total_request_bytes: 500,
                total_response_bytes: 1_000,
                first_seen_at_ms: 1_000,
                last_seen_at_ms: 1_004,
                task_context_ids: None,
                conversation_context_ids: Some(vec![context_id("conversation-a")]),
                runtime_boot_context_ids: None,
            },
            confidence: 0.5,
            compatibility: None,
            source_event_ids: (1..=5).map(|index| format!("event-{index}")).collect(),
            counterexample_event_ids: Vec::new(),
            supersedes: None,
            superseded_by: None,
            created_at_ms: 1_000,
            updated_at_ms: 1_004,
            metadata: Some(object(json!({"proposalType": "performance"}))),
        }
    }

    #[test]
    fn deterministic_id_matches_node_canonical_fixture() {
        let record = record();
        assert_eq!(
            knowledge_record_id(&record).expect("id"),
            "deaebbbbe7bc87ac301c80d55f27928cdf56c5a479818aeb116803cb059dc8be"
        );
    }

    #[test]
    fn tool_evolution_requires_independent_contexts_before_candidate() {
        let dir = tempdir().expect("tempdir");
        let store = KnowledgeStore::new(dir.path());
        let first = store.upsert(record()).expect("first");
        assert_eq!(first.status, KnowledgeStatus::Observed);

        let mut second = record();
        second.source_event_ids = vec!["event-6".into(), "event-7".into()];
        second.evidence.observations = 2;
        second.evidence.successes = 2;
        second.evidence.total_duration_ms = 20_000;
        second.evidence.total_request_bytes = 200;
        second.evidence.total_response_bytes = 400;
        second.evidence.first_seen_at_ms = 2_000;
        second.evidence.last_seen_at_ms = 2_001;
        second.evidence.conversation_context_ids = Some(vec![context_id("conversation-b")]);
        let merged = store.upsert(second).expect("merged");
        assert_eq!(merged.status, KnowledgeStatus::Candidate);
        assert_eq!(merged.evidence.observations, 7);
        assert_eq!(merged.evidence.conversation_context_ids.unwrap().len(), 2);
    }

    #[test]
    fn exact_replay_is_idempotent_and_partial_overlap_is_rejected() {
        let dir = tempdir().expect("tempdir");
        let store = KnowledgeStore::new(dir.path());
        let fixture = record();
        let first = store.upsert(fixture.clone()).expect("first");
        let replayed = store.upsert(fixture.clone()).expect("replayed");
        assert_eq!(first.evidence, replayed.evidence);

        let mut overlap = fixture;
        overlap.source_event_ids = vec!["event-5".into(), "event-new".into()];
        assert!(matches!(
            store.upsert(overlap),
            Err(KnowledgeStoreError::Validation(message))
                if message.contains("partial overlap")
        ));
    }

    #[test]
    fn context_ids_are_hashed_and_bounded() {
        let dir = tempdir().expect("tempdir");
        let store = KnowledgeStore::new(dir.path());
        let mut invalid = record();
        invalid.evidence.conversation_context_ids = Some(vec!["raw-context".into()]);
        assert!(matches!(
            store.upsert(invalid),
            Err(KnowledgeStoreError::Validation(message))
                if message.contains("SHA-256 context identifiers")
        ));

        let mut bounded = record();
        bounded.evidence.task_context_ids = Some(
            (0..300)
                .map(|index| context_id(&format!("task-{index}")))
                .collect(),
        );
        bounded.evidence.conversation_context_ids = Some(vec![
            context_id("conversation-a"),
            context_id("conversation-b"),
        ]);
        let stored = store.upsert(bounded).expect("bounded");
        assert_eq!(stored.evidence.task_context_ids.unwrap().len(), 256);
    }
}
