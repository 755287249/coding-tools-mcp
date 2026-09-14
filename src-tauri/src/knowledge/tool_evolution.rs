use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::model::{KnowledgeArtifactTarget, KnowledgeRecord, KnowledgeStatus};
use super::persistence::write_private_atomic;
use super::store::{KnowledgeStore, KnowledgeStoreError};

const MAX_SAMPLE_DIGESTS_PER_ARM: usize = 2_048;
const MIN_BENCHMARK_SAMPLES: u64 = 5;
const MIN_TASK_OUTCOMES: u64 = 3;
const REQUIRED_IMPROVEMENT_RATIO: f64 = 0.9;
const MAX_EFFICIENCY_REGRESSION_RATIO: f64 = 1.05;
const MAX_PROPOSAL_ACTION_BYTES: usize = 8_192;

#[derive(Debug, Error)]
pub enum ToolEvolutionError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Knowledge(#[from] KnowledgeStoreError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvolutionProposalType {
    Schema,
    Primitive,
    Recovery,
    Performance,
    Result,
}

impl Default for ToolEvolutionProposalType {
    fn default() -> Self {
        Self::Performance
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvolutionChangeProposalStatus {
    Proposed,
    Materialized,
    Superseded,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionChangeProposalRecord {
    pub schema_version: u8,
    pub proposal_id: String,
    pub knowledge_id: String,
    pub tool: String,
    pub proposal_type: ToolEvolutionProposalType,
    pub base_revision: String,
    pub action: Map<String, Value>,
    pub status: ToolEvolutionChangeProposalStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvolutionExperimentStatus {
    Planned,
    Running,
    Validated,
    Rejected,
    Promoted,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvolutionTaskOutcome {
    VerifiedSuccess,
    UnverifiedSuccess,
    Failure,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionBenchmarkSample {
    pub sample_id: String,
    pub source_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_outcome: Option<ToolEvolutionTaskOutcome>,
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub duration_ms: f64,
    pub response_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionSampleDigest {
    pub sample_id: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionBenchmarkAggregate {
    pub samples: u64,
    pub task_outcomes: u64,
    pub verified_task_successes: u64,
    pub unverified_task_successes: u64,
    pub task_failures: u64,
    pub task_rollbacks: u64,
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub duration_ms: f64,
    pub response_bytes: u64,
    pub sample_digests: Vec<ToolEvolutionSampleDigest>,
}

impl Default for ToolEvolutionBenchmarkAggregate {
    fn default() -> Self {
        Self {
            samples: 0,
            task_outcomes: 0,
            verified_task_successes: 0,
            unverified_task_successes: 0,
            task_failures: 0,
            task_rollbacks: 0,
            tool_calls: 0,
            tool_failures: 0,
            duration_ms: 0.0,
            response_bytes: 0,
            sample_digests: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionExperimentPlan {
    pub knowledge_id: String,
    pub tool: String,
    pub proposal_type: ToolEvolutionProposalType,
    pub base_revision: String,
    pub candidate_revision: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionExperimentRecord {
    pub schema_version: u8,
    pub experiment_id: String,
    pub knowledge_id: String,
    pub tool: String,
    pub proposal_type: ToolEvolutionProposalType,
    pub base_revision: String,
    pub candidate_revision: String,
    pub created_at_ms: u64,
    pub status: ToolEvolutionExperimentStatus,
    pub baseline: ToolEvolutionBenchmarkAggregate,
    pub candidate: ToolEvolutionBenchmarkAggregate,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvolutionRuntimeSource {
    pub revision: String,
    pub trusted: bool,
    pub proposal_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolEvolutionCandidateClaimResult {
    pub enabled: bool,
    pub considered: usize,
    pub materialized: usize,
    pub replayed: usize,
    pub ignored: usize,
    pub failed: usize,
}

#[derive(Debug, Clone)]
pub struct ToolEvolutionCandidateClaimer {
    knowledge_store: KnowledgeStore,
    proposal_store: ToolEvolutionProposalStore,
    experiment_store: ToolEvolutionExperimentStore,
    source: ToolEvolutionRuntimeSource,
}

impl ToolEvolutionCandidateClaimer {
    pub fn new(
        knowledge_store: KnowledgeStore,
        proposal_store: ToolEvolutionProposalStore,
        experiment_store: ToolEvolutionExperimentStore,
        source: ToolEvolutionRuntimeSource,
    ) -> Self {
        Self {
            knowledge_store,
            proposal_store,
            experiment_store,
            source,
        }
    }

    pub fn sync(&self, timestamp_ms: u64) -> ToolEvolutionCandidateClaimResult {
        let revision = self.source.revision.trim().to_ascii_lowercase();
        let proposal_ids = self
            .source
            .proposal_ids
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| {
                value.len() == 64 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(16)
            .collect::<Vec<_>>();
        if !self.source.trusted
            || revision.len() != 40
            || !revision
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return ToolEvolutionCandidateClaimResult {
                enabled: false,
                considered: 0,
                materialized: 0,
                replayed: 0,
                ignored: 0,
                failed: 0,
            };
        }
        let mut result = ToolEvolutionCandidateClaimResult {
            enabled: true,
            considered: proposal_ids.len(),
            materialized: 0,
            replayed: 0,
            ignored: 0,
            failed: 0,
        };
        for proposal_id in proposal_ids {
            let attempt = (|| -> Result<&'static str, ToolEvolutionError> {
                let Some(proposal) = self.proposal_store.get(&proposal_id)? else {
                    return Ok("ignored");
                };
                if proposal.status == ToolEvolutionChangeProposalStatus::Materialized {
                    return Ok(
                        if proposal.candidate_revision.as_deref() == Some(revision.as_str())
                            && proposal.experiment_id.is_some()
                        {
                            "replayed"
                        } else {
                            "ignored"
                        },
                    );
                }
                if proposal.status != ToolEvolutionChangeProposalStatus::Proposed
                    || proposal.base_revision == revision
                {
                    return Ok("ignored");
                }
                let (materialized, _, _) = materialize_tool_evolution_proposal(
                    &self.knowledge_store,
                    &self.proposal_store,
                    &self.experiment_store,
                    &proposal_id,
                    &proposal.base_revision,
                    &revision,
                    timestamp_ms,
                )?;
                Ok(if materialized {
                    "materialized"
                } else {
                    "ignored"
                })
            })();
            match attempt {
                Ok("materialized") => result.materialized += 1,
                Ok("replayed") => result.replayed += 1,
                Ok(_) => result.ignored += 1,
                Err(_) => result.failed += 1,
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolEvolutionProposalSyncResult {
    pub enabled: bool,
    pub considered: usize,
    pub planned: usize,
    pub superseded: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProposalDocument {
    schema_version: u8,
    records: Vec<ToolEvolutionChangeProposalRecord>,
}

impl Default for ProposalDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExperimentDocument {
    schema_version: u8,
    records: Vec<ToolEvolutionExperimentRecord>,
}

impl Default for ExperimentDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ToolEvolutionLifecycleTransactionState {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolEvolutionLifecycleTransactionDocument {
    schema_version: u8,
    state: ToolEvolutionLifecycleTransactionState,
    proposal_before_exists: bool,
    experiment_before_exists: bool,
    proposal_before: ProposalDocument,
    proposal_after: ProposalDocument,
    experiment_before: ExperimentDocument,
    experiment_after: ExperimentDocument,
}

#[derive(Default)]
struct ToolEvolutionLifecycleCoordinator {
    recovery_checked: BTreeSet<PathBuf>,
}

static TOOL_EVOLUTION_LIFECYCLE_COORDINATOR: OnceLock<Mutex<ToolEvolutionLifecycleCoordinator>> =
    OnceLock::new();

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

fn stable_json<T: Serialize>(value: &T) -> Result<String, ToolEvolutionError> {
    let json = serde_json::to_value(value)?;
    Ok(serde_json::to_string(&stable_value(&json))?)
}

fn sha256<T: Serialize>(value: &T) -> Result<String, ToolEvolutionError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(stable_json(value)?.as_bytes())
    ))
}

fn bounded_text(value: &str, label: &str, maximum: usize) -> Result<String, ToolEvolutionError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.len() > maximum
        || normalized.contains(['\r', '\n', '\0'])
    {
        return Err(ToolEvolutionError::Validation(format!(
            "{label} must be non-empty bounded single-line text."
        )));
    }
    Ok(normalized.to_string())
}

fn proposal_type(record: &KnowledgeRecord) -> ToolEvolutionProposalType {
    match record
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("proposalType"))
        .and_then(Value::as_str)
    {
        Some("schema") => ToolEvolutionProposalType::Schema,
        Some("primitive") => ToolEvolutionProposalType::Primitive,
        Some("recovery") => ToolEvolutionProposalType::Recovery,
        Some("result") => ToolEvolutionProposalType::Result,
        _ => ToolEvolutionProposalType::Performance,
    }
}

fn tool_name(record: &KnowledgeRecord) -> String {
    record
        .trigger
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn write_document<T: Serialize>(
    root: &Path,
    path: &Path,
    document: &T,
) -> Result<(), ToolEvolutionError> {
    let mut content = serde_json::to_string_pretty(document)?;
    content.push('\n');
    write_private_atomic(root, path, content.as_bytes())?;
    Ok(())
}

fn proposal_file_path(root: &Path) -> PathBuf {
    root.join("tool-evolution-proposals.json")
}

fn experiment_file_path(root: &Path) -> PathBuf {
    root.join("tool-evolution-experiments.json")
}

fn lifecycle_transaction_file_path(root: &Path) -> PathBuf {
    root.join("tool-evolution-lifecycle-transaction.json")
}

fn read_proposal_document_raw(root: &Path) -> Result<(bool, ProposalDocument), ToolEvolutionError> {
    let path = proposal_file_path(root);
    match fs::read_to_string(path) {
        Ok(content) => {
            let document: ProposalDocument = serde_json::from_str(&content)?;
            if document.schema_version != 1 {
                return Err(ToolEvolutionError::Validation(
                    "Unsupported tool evolution proposal schema.".into(),
                ));
            }
            Ok((true, document))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok((false, ProposalDocument::default()))
        }
        Err(error) => Err(error.into()),
    }
}

fn read_experiment_document_raw(
    root: &Path,
) -> Result<(bool, ExperimentDocument), ToolEvolutionError> {
    let path = experiment_file_path(root);
    match fs::read_to_string(path) {
        Ok(content) => {
            let document: ExperimentDocument = serde_json::from_str(&content)?;
            if document.schema_version != 1 {
                return Err(ToolEvolutionError::Validation(
                    "Unsupported tool evolution experiment schema.".into(),
                ));
            }
            Ok((true, document))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok((false, ExperimentDocument::default()))
        }
        Err(error) => Err(error.into()),
    }
}

fn read_lifecycle_transaction_raw(
    root: &Path,
) -> Result<Option<ToolEvolutionLifecycleTransactionDocument>, ToolEvolutionError> {
    match fs::read_to_string(lifecycle_transaction_file_path(root)) {
        Ok(content) => {
            let transaction: ToolEvolutionLifecycleTransactionDocument =
                serde_json::from_str(&content)?;
            if transaction.schema_version != 1
                || transaction.proposal_before.schema_version != 1
                || transaction.proposal_after.schema_version != 1
                || transaction.experiment_before.schema_version != 1
                || transaction.experiment_after.schema_version != 1
            {
                return Err(ToolEvolutionError::Validation(
                    "Unsupported tool evolution lifecycle transaction schema.".into(),
                ));
            }
            Ok(Some(transaction))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), ToolEvolutionError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn restore_proposal_document(
    root: &Path,
    existed: bool,
    document: &ProposalDocument,
) -> Result<(), ToolEvolutionError> {
    let path = proposal_file_path(root);
    if !existed {
        return remove_file_if_exists(&path);
    }
    let (current_exists, current) = read_proposal_document_raw(root)?;
    if current_exists && current == *document {
        return Ok(());
    }
    write_document(root, &path, document)
}

fn restore_experiment_document(
    root: &Path,
    existed: bool,
    document: &ExperimentDocument,
) -> Result<(), ToolEvolutionError> {
    let path = experiment_file_path(root);
    if !existed {
        return remove_file_if_exists(&path);
    }
    let (current_exists, current) = read_experiment_document_raw(root)?;
    if current_exists && current == *document {
        return Ok(());
    }
    write_document(root, &path, document)
}

fn recover_tool_evolution_lifecycle_transaction_raw(root: &Path) -> Result<(), ToolEvolutionError> {
    let Some(transaction) = read_lifecycle_transaction_raw(root)? else {
        return Ok(());
    };
    match transaction.state {
        ToolEvolutionLifecycleTransactionState::Prepared => {
            restore_experiment_document(
                root,
                transaction.experiment_before_exists,
                &transaction.experiment_before,
            )?;
            restore_proposal_document(
                root,
                transaction.proposal_before_exists,
                &transaction.proposal_before,
            )?;
        }
        ToolEvolutionLifecycleTransactionState::Committed => {
            restore_experiment_document(root, true, &transaction.experiment_after)?;
            restore_proposal_document(root, true, &transaction.proposal_after)?;
        }
    }
    remove_file_if_exists(&lifecycle_transaction_file_path(root))
}

fn lifecycle_coordinator() -> &'static Mutex<ToolEvolutionLifecycleCoordinator> {
    TOOL_EVOLUTION_LIFECYCLE_COORDINATOR
        .get_or_init(|| Mutex::new(ToolEvolutionLifecycleCoordinator::default()))
}

fn lock_lifecycle_coordinator(
) -> Result<std::sync::MutexGuard<'static, ToolEvolutionLifecycleCoordinator>, ToolEvolutionError> {
    lifecycle_coordinator().lock().map_err(|_| {
        ToolEvolutionError::Validation("Tool evolution lifecycle coordinator is poisoned.".into())
    })
}

fn ensure_tool_evolution_lifecycle_recovered_locked(
    root: &Path,
    coordinator: &mut ToolEvolutionLifecycleCoordinator,
) -> Result<(), ToolEvolutionError> {
    let key = root.to_path_buf();
    if coordinator.recovery_checked.contains(&key) {
        return Ok(());
    }
    recover_tool_evolution_lifecycle_transaction_raw(root)?;
    coordinator.recovery_checked.insert(key);
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ToolEvolutionProposalStore {
    root_dir: PathBuf,
    file_path: PathBuf,
}

impl ToolEvolutionProposalStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        let file_path = proposal_file_path(&root_dir);
        Self {
            root_dir,
            file_path,
        }
    }

    fn read_document(&self) -> Result<ProposalDocument, ToolEvolutionError> {
        let mut coordinator = lock_lifecycle_coordinator()?;
        ensure_tool_evolution_lifecycle_recovered_locked(&self.root_dir, &mut coordinator)?;
        Ok(read_proposal_document_raw(&self.root_dir)?.1)
    }

    fn write_document(&self, document: &ProposalDocument) -> Result<(), ToolEvolutionError> {
        write_document(&self.root_dir, &self.file_path, document)
    }

    pub fn list(&self) -> Result<Vec<ToolEvolutionChangeProposalRecord>, ToolEvolutionError> {
        Ok(self.read_document()?.records)
    }

    pub fn get(
        &self,
        proposal_id: &str,
    ) -> Result<Option<ToolEvolutionChangeProposalRecord>, ToolEvolutionError> {
        Ok(self
            .list()?
            .into_iter()
            .find(|record| record.proposal_id == proposal_id))
    }

    pub fn revision(&self) -> Result<String, ToolEvolutionError> {
        sha256(&self.list()?)
    }

    pub fn plan(
        &self,
        knowledge: &KnowledgeRecord,
        base_revision: &str,
        timestamp_ms: u64,
    ) -> Result<ToolEvolutionChangeProposalRecord, ToolEvolutionError> {
        if knowledge.target != KnowledgeArtifactTarget::ToolEvolution {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution knowledge required: {}",
                knowledge.id
            )));
        }
        if !matches!(
            knowledge.status,
            KnowledgeStatus::Candidate | KnowledgeStatus::Shadow | KnowledgeStatus::Validated
        ) {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution knowledge {} is not eligible for a proposal in status {:?}.",
                knowledge.id, knowledge.status
            )));
        }
        let tool = bounded_text(&tool_name(knowledge), "tool", 128)?;
        let base_revision = bounded_text(base_revision, "baseRevision", 128)?;
        let action = knowledge.recommended_action.clone();
        if stable_json(&action)?.len() > MAX_PROPOSAL_ACTION_BYTES {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution proposal action exceeds {MAX_PROPOSAL_ACTION_BYTES} bytes."
            )));
        }
        let proposal_type = proposal_type(knowledge);
        let proposal_id =
            tool_evolution_proposal_id(&knowledge.id, &tool, proposal_type, &base_revision)?;
        let mut document = self.read_document()?;
        if let Some(existing) = document
            .records
            .iter()
            .find(|record| record.proposal_id == proposal_id)
        {
            return Ok(existing.clone());
        }
        let record = ToolEvolutionChangeProposalRecord {
            schema_version: 1,
            proposal_id,
            knowledge_id: knowledge.id.clone(),
            tool,
            proposal_type,
            base_revision,
            action,
            status: ToolEvolutionChangeProposalStatus::Proposed,
            created_at_ms: timestamp_ms,
            updated_at_ms: timestamp_ms,
            candidate_revision: None,
            experiment_id: None,
        };
        document.records.push(record.clone());
        document
            .records
            .sort_by(|left, right| left.proposal_id.cmp(&right.proposal_id));
        self.write_document(&document)?;
        Ok(record)
    }

    pub fn supersede(
        &self,
        proposal_id: &str,
        timestamp_ms: u64,
    ) -> Result<Option<ToolEvolutionChangeProposalRecord>, ToolEvolutionError> {
        let mut document = self.read_document()?;
        let Some(index) = document
            .records
            .iter()
            .position(|record| record.proposal_id == proposal_id)
        else {
            return Ok(None);
        };
        if document.records[index].status == ToolEvolutionChangeProposalStatus::Proposed {
            document.records[index].status = ToolEvolutionChangeProposalStatus::Superseded;
            document.records[index].updated_at_ms =
                document.records[index].updated_at_ms.max(timestamp_ms);
            self.write_document(&document)?;
        }
        Ok(Some(document.records[index].clone()))
    }

    pub fn materialize(
        &self,
        proposal_id: &str,
        candidate_revision: &str,
        experiment_id: &str,
        timestamp_ms: u64,
    ) -> Result<ToolEvolutionChangeProposalRecord, ToolEvolutionError> {
        let candidate_revision = bounded_text(candidate_revision, "candidateRevision", 128)?;
        let experiment_id = bounded_text(experiment_id, "experimentId", 128)?;
        let mut document = self.read_document()?;
        let Some(index) = document
            .records
            .iter()
            .position(|record| record.proposal_id == proposal_id)
        else {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution proposal not found: {proposal_id}"
            )));
        };
        let existing = document.records[index].clone();
        if existing.status == ToolEvolutionChangeProposalStatus::Materialized {
            if existing.candidate_revision.as_deref() != Some(candidate_revision.as_str())
                || existing.experiment_id.as_deref() != Some(experiment_id.as_str())
            {
                return Err(ToolEvolutionError::Validation(format!(
                    "Tool evolution proposal {proposal_id} was already materialized with a different candidate."
                )));
            }
            return Ok(existing);
        }
        if existing.status != ToolEvolutionChangeProposalStatus::Proposed {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution proposal {proposal_id} cannot materialize from status {:?}.",
                existing.status
            )));
        }
        document.records[index].status = ToolEvolutionChangeProposalStatus::Materialized;
        document.records[index].candidate_revision = Some(candidate_revision);
        document.records[index].experiment_id = Some(experiment_id);
        document.records[index].updated_at_ms = existing.updated_at_ms.max(timestamp_ms);
        self.write_document(&document)?;
        Ok(document.records[index].clone())
    }
}

pub fn tool_evolution_proposal_id(
    knowledge_id: &str,
    tool: &str,
    proposal_type: ToolEvolutionProposalType,
    base_revision: &str,
) -> Result<String, ToolEvolutionError> {
    sha256(&serde_json::json!({
        "knowledgeId": knowledge_id,
        "tool": tool,
        "proposalType": proposal_type,
        "baseRevision": base_revision,
    }))
}

#[derive(Debug, Clone)]
pub struct ToolEvolutionPlanner {
    knowledge_store: KnowledgeStore,
    proposal_store: ToolEvolutionProposalStore,
    experiment_store: Option<ToolEvolutionExperimentStore>,
    source: ToolEvolutionRuntimeSource,
}

impl ToolEvolutionPlanner {
    pub fn new(
        knowledge_store: KnowledgeStore,
        proposal_store: ToolEvolutionProposalStore,
        experiment_store: Option<ToolEvolutionExperimentStore>,
        source: ToolEvolutionRuntimeSource,
    ) -> Self {
        Self {
            knowledge_store,
            proposal_store,
            experiment_store,
            source,
        }
    }

    pub fn sync(
        &self,
        timestamp_ms: u64,
    ) -> Result<ToolEvolutionProposalSyncResult, ToolEvolutionError> {
        let revision = self.source.revision.trim();
        if !self.source.trusted || revision.is_empty() {
            return Ok(ToolEvolutionProposalSyncResult {
                enabled: false,
                considered: 0,
                planned: 0,
                superseded: 0,
            });
        }
        let revision = bounded_text(revision, "baseRevision", 128)?;
        let knowledge = self
            .knowledge_store
            .list()?
            .into_iter()
            .filter(|record| {
                record.target == KnowledgeArtifactTarget::ToolEvolution
                    && matches!(
                        record.status,
                        KnowledgeStatus::Candidate
                            | KnowledgeStatus::Shadow
                            | KnowledgeStatus::Validated
                    )
            })
            .collect::<Vec<_>>();
        let eligible_ids = knowledge
            .iter()
            .map(|record| record.id.clone())
            .collect::<BTreeSet<_>>();
        let existing = self.proposal_store.list()?;
        let active_experiments = match &self.experiment_store {
            Some(store) => store
                .list()?
                .into_iter()
                .filter(|record| {
                    matches!(
                        record.status,
                        ToolEvolutionExperimentStatus::Planned
                            | ToolEvolutionExperimentStatus::Running
                            | ToolEvolutionExperimentStatus::Validated
                            | ToolEvolutionExperimentStatus::Promoted
                    )
                })
                .map(|record| record.experiment_id)
                .collect::<BTreeSet<_>>(),
            None => BTreeSet::new(),
        };
        let blocked_knowledge_ids = existing
            .iter()
            .filter(|proposal| {
                proposal.status == ToolEvolutionChangeProposalStatus::Materialized
                    && (self.experiment_store.is_none()
                        || proposal
                            .experiment_id
                            .as_ref()
                            .is_some_and(|id| active_experiments.contains(id)))
            })
            .map(|proposal| proposal.knowledge_id.clone())
            .collect::<BTreeSet<_>>();
        let mut superseded = 0;
        for proposal in &existing {
            if proposal.status != ToolEvolutionChangeProposalStatus::Proposed {
                continue;
            }
            if proposal.base_revision == revision && eligible_ids.contains(&proposal.knowledge_id) {
                continue;
            }
            if self
                .proposal_store
                .supersede(&proposal.proposal_id, timestamp_ms)?
                .is_some_and(|updated| {
                    updated.status == ToolEvolutionChangeProposalStatus::Superseded
                })
            {
                superseded += 1;
            }
        }
        let mut known_ids = existing
            .iter()
            .map(|record| record.proposal_id.clone())
            .collect::<BTreeSet<_>>();
        let mut planned = 0;
        for record in &knowledge {
            if blocked_knowledge_ids.contains(&record.id) {
                continue;
            }
            let tool = tool_name(record);
            let proposal_type = proposal_type(record);
            let proposal_id =
                tool_evolution_proposal_id(&record.id, &tool, proposal_type, &revision)?;
            if known_ids.contains(&proposal_id) {
                continue;
            }
            self.proposal_store.plan(record, &revision, timestamp_ms)?;
            known_ids.insert(proposal_id);
            planned += 1;
        }
        Ok(ToolEvolutionProposalSyncResult {
            enabled: true,
            considered: knowledge.len(),
            planned,
            superseded,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ToolEvolutionExperimentStore {
    root_dir: PathBuf,
    file_path: PathBuf,
}

impl ToolEvolutionExperimentStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        let file_path = experiment_file_path(&root_dir);
        Self {
            root_dir,
            file_path,
        }
    }

    fn read_document(&self) -> Result<ExperimentDocument, ToolEvolutionError> {
        let mut coordinator = lock_lifecycle_coordinator()?;
        ensure_tool_evolution_lifecycle_recovered_locked(&self.root_dir, &mut coordinator)?;
        Ok(read_experiment_document_raw(&self.root_dir)?.1)
    }

    fn write_document(&self, document: &ExperimentDocument) -> Result<(), ToolEvolutionError> {
        write_document(&self.root_dir, &self.file_path, document)
    }

    pub fn list(&self) -> Result<Vec<ToolEvolutionExperimentRecord>, ToolEvolutionError> {
        Ok(self.read_document()?.records)
    }

    pub fn get(
        &self,
        experiment_id: &str,
    ) -> Result<Option<ToolEvolutionExperimentRecord>, ToolEvolutionError> {
        Ok(self
            .list()?
            .into_iter()
            .find(|record| record.experiment_id == experiment_id))
    }

    pub fn revision(&self) -> Result<String, ToolEvolutionError> {
        sha256(&self.list()?)
    }

    pub fn plan(
        &self,
        plan: ToolEvolutionExperimentPlan,
    ) -> Result<ToolEvolutionExperimentRecord, ToolEvolutionError> {
        let record = planned_tool_evolution_experiment(plan)?;
        let mut document = self.read_document()?;
        if let Some(existing) = document
            .records
            .iter()
            .find(|existing| existing.experiment_id == record.experiment_id)
        {
            return Ok(existing.clone());
        }
        document.records.push(record.clone());
        document
            .records
            .sort_by(|left, right| left.experiment_id.cmp(&right.experiment_id));
        self.write_document(&document)?;
        Ok(record)
    }

    pub fn record_benchmark(
        &self,
        experiment_id: &str,
        arm: ToolEvolutionBenchmarkArm,
        sample: ToolEvolutionBenchmarkSample,
        timestamp_ms: u64,
    ) -> Result<ToolEvolutionExperimentRecord, ToolEvolutionError> {
        let mut document = self.read_document()?;
        let Some(index) = document
            .records
            .iter()
            .position(|record| record.experiment_id == experiment_id)
        else {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution experiment not found: {experiment_id}"
            )));
        };
        let record = document.records[index].clone();
        if !matches!(
            record.status,
            ToolEvolutionExperimentStatus::Planned | ToolEvolutionExperimentStatus::Running
        ) {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution experiment {experiment_id} is not accepting benchmark samples in status {:?}.", record.status
            )));
        }
        let expected_revision = match arm {
            ToolEvolutionBenchmarkArm::Baseline => &record.base_revision,
            ToolEvolutionBenchmarkArm::Candidate => &record.candidate_revision,
        };
        if sample.source_revision.trim() != expected_revision {
            return Err(ToolEvolutionError::Validation(
                "Benchmark sourceRevision must match the experiment arm revision.".into(),
            ));
        }
        let aggregate = match arm {
            ToolEvolutionBenchmarkArm::Baseline => &record.baseline,
            ToolEvolutionBenchmarkArm::Candidate => &record.candidate,
        };
        let (updated, added) = add_sample(aggregate, &sample)?;
        if !added {
            return Ok(record);
        }
        document.records[index].status = ToolEvolutionExperimentStatus::Running;
        match arm {
            ToolEvolutionBenchmarkArm::Baseline => document.records[index].baseline = updated,
            ToolEvolutionBenchmarkArm::Candidate => document.records[index].candidate = updated,
        }
        document.records[index].updated_at_ms = record.updated_at_ms.max(timestamp_ms);
        document.records[index].decision_reason = None;
        self.write_document(&document)?;
        Ok(document.records[index].clone())
    }

    pub fn evaluate(
        &self,
        experiment_id: &str,
        current_base_revision: &str,
        timestamp_ms: u64,
    ) -> Result<ToolEvolutionExperimentRecord, ToolEvolutionError> {
        let mut document = self.read_document()?;
        let Some(index) = document
            .records
            .iter()
            .position(|record| record.experiment_id == experiment_id)
        else {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution experiment not found: {experiment_id}"
            )));
        };
        let record = document.records[index].clone();
        if matches!(
            record.status,
            ToolEvolutionExperimentStatus::Promoted
                | ToolEvolutionExperimentStatus::Rejected
                | ToolEvolutionExperimentStatus::Stale
        ) {
            return Ok(record);
        }
        let current_base = bounded_text(current_base_revision, "currentBaseRevision", 128)?;
        if current_base != record.base_revision {
            document.records[index].status = ToolEvolutionExperimentStatus::Stale;
            document.records[index].decision_reason = Some("base_revision_changed".into());
            document.records[index].updated_at_ms = record.updated_at_ms.max(timestamp_ms);
            self.write_document(&document)?;
            return Ok(document.records[index].clone());
        }
        let baseline = &record.baseline;
        let candidate = &record.candidate;
        if baseline.samples < MIN_BENCHMARK_SAMPLES
            || candidate.samples < MIN_BENCHMARK_SAMPLES
            || baseline.task_outcomes < MIN_TASK_OUTCOMES
            || candidate.task_outcomes < MIN_TASK_OUTCOMES
        {
            document.records[index].status = ToolEvolutionExperimentStatus::Running;
            document.records[index].decision_reason =
                Some("insufficient_benchmark_evidence".into());
            document.records[index].updated_at_ms = record.updated_at_ms.max(timestamp_ms);
            self.write_document(&document)?;
            return Ok(document.records[index].clone());
        }

        let mut reason = None;
        let baseline_verified_rate =
            ratio(baseline.verified_task_successes, baseline.task_outcomes);
        let candidate_verified_rate =
            ratio(candidate.verified_task_successes, candidate.task_outcomes);
        let baseline_task_failure_rate = ratio(baseline.task_failures, baseline.task_outcomes);
        let candidate_task_failure_rate = ratio(candidate.task_failures, candidate.task_outcomes);
        let baseline_tool_failure_rate = ratio(baseline.tool_failures, baseline.tool_calls);
        let candidate_tool_failure_rate = ratio(candidate.tool_failures, candidate.tool_calls);
        if candidate.task_rollbacks > 0 {
            reason = Some("candidate_task_rollback");
        } else if candidate_verified_rate < baseline_verified_rate {
            reason = Some("verified_task_success_regression");
        } else if candidate_task_failure_rate > baseline_task_failure_rate {
            reason = Some("task_failure_rate_regression");
        } else if candidate_tool_failure_rate > baseline_tool_failure_rate {
            reason = Some("tool_failure_rate_regression");
        }
        let baseline_efficiency = (
            calls_per_verified_success(baseline),
            average_duration(baseline),
            response_bytes_per_call(baseline),
        );
        let candidate_efficiency = (
            calls_per_verified_success(candidate),
            average_duration(candidate),
            response_bytes_per_call(candidate),
        );
        if reason.is_none()
            && (!no_regression(candidate_efficiency.0, baseline_efficiency.0)
                || !no_regression(candidate_efficiency.1, baseline_efficiency.1)
                || !no_regression(candidate_efficiency.2, baseline_efficiency.2))
        {
            reason = Some("efficiency_regression");
        }
        if reason.is_none()
            && !(material_improvement(candidate_efficiency.0, baseline_efficiency.0)
                || material_improvement(candidate_efficiency.1, baseline_efficiency.1)
                || material_improvement(candidate_efficiency.2, baseline_efficiency.2))
        {
            reason = Some("no_material_efficiency_improvement");
        }
        document.records[index].status = if reason.is_some() {
            ToolEvolutionExperimentStatus::Rejected
        } else {
            ToolEvolutionExperimentStatus::Validated
        };
        document.records[index].decision_reason = Some(
            reason
                .unwrap_or("quality_preserved_with_material_efficiency_improvement")
                .into(),
        );
        document.records[index].updated_at_ms = record.updated_at_ms.max(timestamp_ms);
        self.write_document(&document)?;
        Ok(document.records[index].clone())
    }
    pub fn promote(
        &self,
        experiment_id: &str,
        current_base_revision: &str,
        current_candidate_revision: &str,
        timestamp_ms: u64,
    ) -> Result<ToolEvolutionExperimentRecord, ToolEvolutionError> {
        let current = self.evaluate(experiment_id, current_base_revision, timestamp_ms)?;
        if current.status == ToolEvolutionExperimentStatus::Promoted {
            return Ok(current);
        }
        if current.status != ToolEvolutionExperimentStatus::Validated {
            return Ok(current);
        }
        let current_candidate =
            bounded_text(current_candidate_revision, "currentCandidateRevision", 128)?;
        if current_candidate != current.candidate_revision {
            let mut document = self.read_document()?;
            let Some(index) = document
                .records
                .iter()
                .position(|record| record.experiment_id == experiment_id)
            else {
                return Err(ToolEvolutionError::Validation(format!(
                    "Tool evolution experiment not found: {experiment_id}"
                )));
            };
            document.records[index].status = ToolEvolutionExperimentStatus::Stale;
            document.records[index].decision_reason = Some("candidate_revision_changed".into());
            document.records[index].updated_at_ms =
                document.records[index].updated_at_ms.max(timestamp_ms);
            self.write_document(&document)?;
            return Ok(document.records[index].clone());
        }
        let mut document = self.read_document()?;
        let Some(index) = document
            .records
            .iter()
            .position(|record| record.experiment_id == experiment_id)
        else {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution experiment not found: {experiment_id}"
            )));
        };
        document.records[index].status = ToolEvolutionExperimentStatus::Promoted;
        document.records[index].decision_reason = Some("validated_candidate_promoted".into());
        document.records[index].updated_at_ms =
            document.records[index].updated_at_ms.max(timestamp_ms);
        self.write_document(&document)?;
        Ok(document.records[index].clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEvolutionBenchmarkArm {
    Baseline,
    Candidate,
}

pub fn tool_evolution_experiment_id(
    plan: &ToolEvolutionExperimentPlan,
) -> Result<String, ToolEvolutionError> {
    sha256(&serde_json::json!({
        "knowledgeId": plan.knowledge_id,
        "tool": plan.tool,
        "proposalType": plan.proposal_type,
        "baseRevision": plan.base_revision,
        "candidateRevision": plan.candidate_revision,
    }))
}

fn planned_tool_evolution_experiment(
    plan: ToolEvolutionExperimentPlan,
) -> Result<ToolEvolutionExperimentRecord, ToolEvolutionError> {
    let plan = ToolEvolutionExperimentPlan {
        knowledge_id: bounded_text(&plan.knowledge_id, "knowledgeId", 128)?,
        tool: bounded_text(&plan.tool, "tool", 128)?,
        proposal_type: plan.proposal_type,
        base_revision: bounded_text(&plan.base_revision, "baseRevision", 128)?,
        candidate_revision: bounded_text(&plan.candidate_revision, "candidateRevision", 128)?,
        created_at_ms: plan.created_at_ms,
    };
    if plan.base_revision == plan.candidate_revision {
        return Err(ToolEvolutionError::Validation(
            "Tool evolution candidateRevision must differ from baseRevision.".into(),
        ));
    }
    Ok(ToolEvolutionExperimentRecord {
        schema_version: 1,
        experiment_id: tool_evolution_experiment_id(&plan)?,
        knowledge_id: plan.knowledge_id,
        tool: plan.tool,
        proposal_type: plan.proposal_type,
        base_revision: plan.base_revision,
        candidate_revision: plan.candidate_revision,
        created_at_ms: plan.created_at_ms,
        status: ToolEvolutionExperimentStatus::Planned,
        baseline: ToolEvolutionBenchmarkAggregate::default(),
        candidate: ToolEvolutionBenchmarkAggregate::default(),
        updated_at_ms: plan.created_at_ms,
        decision_reason: None,
    })
}

fn materialize_tool_evolution_lifecycle_atomic(
    root: &Path,
    proposal_id: &str,
    plan: ToolEvolutionExperimentPlan,
    timestamp_ms: u64,
) -> Result<
    (
        ToolEvolutionChangeProposalRecord,
        ToolEvolutionExperimentRecord,
    ),
    ToolEvolutionError,
> {
    let mut coordinator = lock_lifecycle_coordinator()?;
    ensure_tool_evolution_lifecycle_recovered_locked(root, &mut coordinator)?;
    let (proposal_before_exists, proposal_before) = read_proposal_document_raw(root)?;
    let (experiment_before_exists, experiment_before) = read_experiment_document_raw(root)?;
    let mut proposal_after = proposal_before.clone();
    let mut experiment_after = experiment_before.clone();
    let Some(proposal_index) = proposal_after
        .records
        .iter()
        .position(|record| record.proposal_id == proposal_id)
    else {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution proposal not found: {proposal_id}"
        )));
    };
    let existing_proposal = proposal_after.records[proposal_index].clone();
    let planned_experiment = planned_tool_evolution_experiment(plan)?;
    if existing_proposal.status == ToolEvolutionChangeProposalStatus::Materialized {
        if existing_proposal.candidate_revision.as_deref()
            != Some(planned_experiment.candidate_revision.as_str())
            || existing_proposal.experiment_id.as_deref()
                != Some(planned_experiment.experiment_id.as_str())
        {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution proposal {proposal_id} was already materialized with a different candidate."
            )));
        }
        let experiment = experiment_after
            .records
            .iter()
            .find(|record| record.experiment_id == planned_experiment.experiment_id)
            .cloned()
            .ok_or_else(|| {
                ToolEvolutionError::Validation(format!(
                    "Materialized tool evolution experiment not found: {}",
                    planned_experiment.experiment_id
                ))
            })?;
        return Ok((existing_proposal, experiment));
    }
    if existing_proposal.status != ToolEvolutionChangeProposalStatus::Proposed {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution proposal {proposal_id} cannot materialize from status {:?}.",
            existing_proposal.status
        )));
    }
    if existing_proposal.knowledge_id != planned_experiment.knowledge_id
        || existing_proposal.tool != planned_experiment.tool
        || existing_proposal.proposal_type != planned_experiment.proposal_type
        || existing_proposal.base_revision != planned_experiment.base_revision
    {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution proposal {proposal_id} does not match the experiment plan."
        )));
    }
    let experiment = if let Some(existing) = experiment_after
        .records
        .iter()
        .find(|record| record.experiment_id == planned_experiment.experiment_id)
        .cloned()
    {
        if existing.knowledge_id != planned_experiment.knowledge_id
            || existing.tool != planned_experiment.tool
            || existing.proposal_type != planned_experiment.proposal_type
            || existing.base_revision != planned_experiment.base_revision
            || existing.candidate_revision != planned_experiment.candidate_revision
        {
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution experiment {} conflicts with the deterministic experiment plan.",
                planned_experiment.experiment_id
            )));
        }
        existing
    } else {
        experiment_after.records.push(planned_experiment.clone());
        experiment_after
            .records
            .sort_by(|left, right| left.experiment_id.cmp(&right.experiment_id));
        planned_experiment.clone()
    };
    let mut materialized = existing_proposal;
    materialized.status = ToolEvolutionChangeProposalStatus::Materialized;
    materialized.candidate_revision = Some(planned_experiment.candidate_revision.clone());
    materialized.experiment_id = Some(planned_experiment.experiment_id.clone());
    materialized.updated_at_ms = materialized.updated_at_ms.max(timestamp_ms);
    proposal_after.records[proposal_index] = materialized.clone();

    let transaction = ToolEvolutionLifecycleTransactionDocument {
        schema_version: 1,
        state: ToolEvolutionLifecycleTransactionState::Prepared,
        proposal_before_exists,
        experiment_before_exists,
        proposal_before,
        proposal_after: proposal_after.clone(),
        experiment_before,
        experiment_after: experiment_after.clone(),
    };
    let journal_path = lifecycle_transaction_file_path(root);
    write_document(root, &journal_path, &transaction)?;
    let commit_result = (|| -> Result<(), ToolEvolutionError> {
        write_document(root, &experiment_file_path(root), &experiment_after)?;
        write_document(root, &proposal_file_path(root), &proposal_after)?;
        let mut committed = transaction.clone();
        committed.state = ToolEvolutionLifecycleTransactionState::Committed;
        write_document(root, &journal_path, &committed)?;
        Ok(())
    })();
    if let Err(error) = commit_result {
        if let Err(recovery_error) = recover_tool_evolution_lifecycle_transaction_raw(root) {
            coordinator.recovery_checked.remove(&root.to_path_buf());
            return Err(ToolEvolutionError::Validation(format!(
                "Tool evolution lifecycle transaction failed ({error}) and rollback could not complete ({recovery_error})."
            )));
        }
        return Err(error);
    }
    if remove_file_if_exists(&journal_path).is_err() {
        coordinator.recovery_checked.remove(&root.to_path_buf());
    }
    Ok((materialized, experiment))
}

pub fn create_tool_evolution_experiment(
    knowledge_store: &KnowledgeStore,
    experiment_store: &ToolEvolutionExperimentStore,
    knowledge_id: &str,
    base_revision: &str,
    candidate_revision: &str,
    timestamp_ms: u64,
) -> Result<ToolEvolutionExperimentRecord, ToolEvolutionError> {
    let Some(knowledge) = knowledge_store.get(knowledge_id)? else {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution knowledge not found: {knowledge_id}"
        )));
    };
    if knowledge.target != KnowledgeArtifactTarget::ToolEvolution
        || !matches!(
            knowledge.status,
            KnowledgeStatus::Candidate | KnowledgeStatus::Shadow | KnowledgeStatus::Validated
        )
    {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution knowledge {knowledge_id} is not eligible for a new experiment."
        )));
    }
    let tool = bounded_text(&tool_name(&knowledge), "tool", 128)?;
    let proposal_type = proposal_type(&knowledge);
    experiment_store.plan(ToolEvolutionExperimentPlan {
        knowledge_id: knowledge.id,
        tool,
        proposal_type,
        base_revision: base_revision.to_string(),
        candidate_revision: candidate_revision.to_string(),
        created_at_ms: timestamp_ms,
    })
}

pub fn materialize_tool_evolution_proposal(
    knowledge_store: &KnowledgeStore,
    proposal_store: &ToolEvolutionProposalStore,
    experiment_store: &ToolEvolutionExperimentStore,
    proposal_id: &str,
    current_base_revision: &str,
    candidate_revision: &str,
    timestamp_ms: u64,
) -> Result<
    (
        bool,
        ToolEvolutionChangeProposalRecord,
        Option<ToolEvolutionExperimentRecord>,
    ),
    ToolEvolutionError,
> {
    let Some(proposal) = proposal_store.get(proposal_id)? else {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution proposal not found: {proposal_id}"
        )));
    };
    let candidate_revision = bounded_text(candidate_revision, "candidateRevision", 128)?;
    if proposal.status == ToolEvolutionChangeProposalStatus::Materialized {
        if proposal.candidate_revision.as_deref() != Some(candidate_revision.as_str()) {
            return Err(ToolEvolutionError::Validation(format!("Tool evolution proposal {proposal_id} was already materialized with a different candidate.")));
        }
        let experiment_id = proposal.experiment_id.as_deref().ok_or_else(|| {
            ToolEvolutionError::Validation("Materialized proposal is missing experimentId.".into())
        })?;
        let experiment = experiment_store.get(experiment_id)?.ok_or_else(|| {
            ToolEvolutionError::Validation(format!(
                "Materialized tool evolution experiment not found: {experiment_id}"
            ))
        })?;
        return Ok((true, proposal, Some(experiment)));
    }
    let current_base_revision = bounded_text(current_base_revision, "currentBaseRevision", 128)?;
    if proposal.base_revision != current_base_revision {
        let stale = proposal_store
            .supersede(proposal_id, timestamp_ms)?
            .unwrap_or(proposal);
        return Ok((false, stale, None));
    }
    if candidate_revision == current_base_revision {
        return Err(ToolEvolutionError::Validation(
            "Tool evolution candidateRevision must differ from baseRevision.".into(),
        ));
    }
    if proposal.status != ToolEvolutionChangeProposalStatus::Proposed {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution proposal {proposal_id} cannot materialize from status {:?}.",
            proposal.status
        )));
    }
    if proposal_store.root_dir != experiment_store.root_dir {
        return Err(ToolEvolutionError::Validation(
            "Tool evolution proposal and experiment stores must share the same rootDir for atomic materialization."
                .into(),
        ));
    }
    let Some(knowledge) = knowledge_store.get(&proposal.knowledge_id)? else {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution knowledge not found: {}",
            proposal.knowledge_id
        )));
    };
    if knowledge.target != KnowledgeArtifactTarget::ToolEvolution
        || !matches!(
            knowledge.status,
            KnowledgeStatus::Candidate | KnowledgeStatus::Shadow | KnowledgeStatus::Validated
        )
    {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution knowledge {} is not eligible for a new experiment.",
            proposal.knowledge_id
        )));
    }
    let tool = bounded_text(&tool_name(&knowledge), "tool", 128)?;
    let plan = ToolEvolutionExperimentPlan {
        knowledge_id: proposal.knowledge_id.clone(),
        tool,
        proposal_type: proposal_type(&knowledge),
        base_revision: proposal.base_revision.clone(),
        candidate_revision,
        created_at_ms: timestamp_ms,
    };
    let (materialized, experiment) = materialize_tool_evolution_lifecycle_atomic(
        &proposal_store.root_dir,
        proposal_id,
        plan,
        timestamp_ms,
    )?;
    Ok((true, materialized, Some(experiment)))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolEvolutionPromotionResult {
    pub promoted: bool,
    pub experiment: ToolEvolutionExperimentRecord,
    pub knowledge: Option<KnowledgeRecord>,
}

pub fn promote_tool_evolution_experiment(
    knowledge_store: &KnowledgeStore,
    experiment_store: &ToolEvolutionExperimentStore,
    experiment_id: &str,
    current_base_revision: &str,
    current_candidate_revision: &str,
    timestamp_ms: u64,
) -> Result<ToolEvolutionPromotionResult, ToolEvolutionError> {
    let Some(experiment) = experiment_store.get(experiment_id)? else {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution experiment not found: {experiment_id}"
        )));
    };
    let Some(knowledge) = knowledge_store.get(&experiment.knowledge_id)? else {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution knowledge not found: {}",
            experiment.knowledge_id
        )));
    };
    if knowledge.target != KnowledgeArtifactTarget::ToolEvolution {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution knowledge not found: {}",
            experiment.knowledge_id
        )));
    }
    if experiment.status == ToolEvolutionExperimentStatus::Promoted
        && knowledge.status == KnowledgeStatus::Promoted
    {
        return Ok(ToolEvolutionPromotionResult {
            promoted: true,
            experiment,
            knowledge: Some(knowledge),
        });
    }
    if !matches!(
        knowledge.status,
        KnowledgeStatus::Candidate | KnowledgeStatus::Shadow | KnowledgeStatus::Validated
    ) {
        return Ok(ToolEvolutionPromotionResult {
            promoted: false,
            experiment,
            knowledge: Some(knowledge),
        });
    }
    let original_status = knowledge.status;
    let transitioned = knowledge_store.transition_status(
        &knowledge.id,
        &[original_status],
        KnowledgeStatus::Promoted,
        timestamp_ms,
    )?;
    if transitioned.is_none() {
        return Ok(ToolEvolutionPromotionResult {
            promoted: false,
            experiment,
            knowledge: Some(knowledge),
        });
    }

    match experiment_store.promote(
        experiment_id,
        current_base_revision,
        current_candidate_revision,
        timestamp_ms,
    ) {
        Ok(promoted_experiment)
            if promoted_experiment.status == ToolEvolutionExperimentStatus::Promoted =>
        {
            Ok(ToolEvolutionPromotionResult {
                promoted: true,
                experiment: promoted_experiment,
                knowledge: knowledge_store.get(&knowledge.id)?,
            })
        }
        Ok(experiment) => {
            let restored = knowledge_store.transition_status(
                &knowledge.id,
                &[KnowledgeStatus::Promoted],
                original_status,
                timestamp_ms,
            )?;
            Ok(ToolEvolutionPromotionResult {
                promoted: false,
                experiment,
                knowledge: restored.or(Some(knowledge)),
            })
        }
        Err(error) => {
            let _ = knowledge_store.transition_status(
                &knowledge.id,
                &[KnowledgeStatus::Promoted],
                original_status,
                timestamp_ms,
            );
            Err(error)
        }
    }
}

fn normalize_sample(
    sample: &ToolEvolutionBenchmarkSample,
) -> Result<ToolEvolutionBenchmarkSample, ToolEvolutionError> {
    if !sample.duration_ms.is_finite()
        || sample.duration_ms < 0.0
        || sample.duration_ms > 86_400_000.0
    {
        return Err(ToolEvolutionError::Validation(
            "durationMs must be a non-negative bounded number.".into(),
        ));
    }
    if sample.tool_failures > sample.tool_calls {
        return Err(ToolEvolutionError::Validation(
            "toolFailures cannot exceed toolCalls.".into(),
        ));
    }
    Ok(ToolEvolutionBenchmarkSample {
        sample_id: bounded_text(&sample.sample_id, "sampleId", 128)?,
        source_revision: bounded_text(&sample.source_revision, "sourceRevision", 128)?,
        task_outcome: sample.task_outcome,
        tool_calls: sample.tool_calls,
        tool_failures: sample.tool_failures,
        duration_ms: sample.duration_ms,
        response_bytes: sample.response_bytes,
    })
}

fn add_sample(
    aggregate: &ToolEvolutionBenchmarkAggregate,
    sample: &ToolEvolutionBenchmarkSample,
) -> Result<(ToolEvolutionBenchmarkAggregate, bool), ToolEvolutionError> {
    let sample = normalize_sample(sample)?;
    let digest = sha256(&sample)?;
    if let Some(existing) = aggregate
        .sample_digests
        .iter()
        .find(|item| item.sample_id == sample.sample_id)
    {
        if existing.digest != digest {
            return Err(ToolEvolutionError::Validation(format!(
                "Benchmark sample {} was replayed with different metrics.",
                sample.sample_id
            )));
        }
        return Ok((aggregate.clone(), false));
    }
    if aggregate.sample_digests.len() >= MAX_SAMPLE_DIGESTS_PER_ARM {
        return Err(ToolEvolutionError::Validation(format!(
            "Tool evolution benchmark arm is capped at {MAX_SAMPLE_DIGESTS_PER_ARM} samples."
        )));
    }
    let mut next = aggregate.clone();
    next.samples += 1;
    next.tool_calls = next.tool_calls.saturating_add(sample.tool_calls);
    next.tool_failures = next.tool_failures.saturating_add(sample.tool_failures);
    next.duration_ms += sample.duration_ms;
    next.response_bytes = next.response_bytes.saturating_add(sample.response_bytes);
    if let Some(outcome) = sample.task_outcome {
        next.task_outcomes += 1;
        match outcome {
            ToolEvolutionTaskOutcome::VerifiedSuccess => next.verified_task_successes += 1,
            ToolEvolutionTaskOutcome::UnverifiedSuccess => next.unverified_task_successes += 1,
            ToolEvolutionTaskOutcome::Failure => next.task_failures += 1,
            ToolEvolutionTaskOutcome::RolledBack => next.task_rollbacks += 1,
        }
    }
    next.sample_digests.push(ToolEvolutionSampleDigest {
        sample_id: sample.sample_id,
        digest,
    });
    next.sample_digests
        .sort_by(|left, right| left.sample_id.cmp(&right.sample_id));
    Ok((next, true))
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator > 0 {
        numerator as f64 / denominator as f64
    } else {
        0.0
    }
}

fn calls_per_verified_success(metrics: &ToolEvolutionBenchmarkAggregate) -> f64 {
    if metrics.verified_task_successes > 0 {
        metrics.tool_calls as f64 / metrics.verified_task_successes as f64
    } else {
        f64::INFINITY
    }
}

fn average_duration(metrics: &ToolEvolutionBenchmarkAggregate) -> f64 {
    if metrics.samples > 0 {
        metrics.duration_ms / metrics.samples as f64
    } else {
        f64::INFINITY
    }
}

fn response_bytes_per_call(metrics: &ToolEvolutionBenchmarkAggregate) -> f64 {
    if metrics.tool_calls > 0 {
        metrics.response_bytes as f64 / metrics.tool_calls as f64
    } else {
        0.0
    }
}

fn no_regression(candidate: f64, baseline: f64) -> bool {
    if !baseline.is_finite() {
        return candidate.is_finite() || candidate == baseline;
    }
    if baseline == 0.0 {
        return candidate == 0.0;
    }
    candidate <= baseline * MAX_EFFICIENCY_REGRESSION_RATIO
}

fn material_improvement(candidate: f64, baseline: f64) -> bool {
    candidate.is_finite()
        && baseline.is_finite()
        && baseline > 0.0
        && candidate <= baseline * REQUIRED_IMPROVEMENT_RATIO
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::knowledge::{ExperienceCompiler, ExperienceObservation, ExperienceOutcome};

    fn candidate_knowledge(store: &KnowledgeStore) -> KnowledgeRecord {
        let observations = (0..5)
            .map(|index| ExperienceObservation {
                event_id: format!("event-{index}"),
                tool: "project_state".into(),
                outcome: ExperienceOutcome::Success,
                duration_ms: 2_000,
                request_bytes: None,
                response_bytes: None,
                error_code: None,
                features: json!({"phase_baseline_capture_ms": 1_500})
                    .as_object()
                    .cloned(),
                verification_outcome: None,
                recovery_outcome: None,
                canary: None,
                operation_id: None,
                task_id: Some(format!("task-{index}")),
                conversation_context_id: None,
                skill: None,
                runtime_boot_id: None,
                timestamp_ms: 1_000 + index,
            })
            .collect::<Vec<_>>();
        let record = ExperienceCompiler.compile(&observations).unwrap().remove(0);
        store.upsert(record).unwrap()
    }

    fn sample(id: &str, revision: &str, duration_ms: f64) -> ToolEvolutionBenchmarkSample {
        ToolEvolutionBenchmarkSample {
            sample_id: id.into(),
            source_revision: revision.into(),
            task_outcome: Some(ToolEvolutionTaskOutcome::VerifiedSuccess),
            tool_calls: 10,
            tool_failures: 0,
            duration_ms,
            response_bytes: 1_000,
        }
    }

    #[test]
    fn planner_creates_idempotent_proposal_and_supersedes_old_revision() {
        let dir = tempdir().unwrap();
        let knowledge_store = KnowledgeStore::new(dir.path());
        let knowledge = candidate_knowledge(&knowledge_store);
        assert_eq!(knowledge.status, KnowledgeStatus::Candidate);
        let proposal_store = ToolEvolutionProposalStore::new(dir.path());
        let planner = ToolEvolutionPlanner::new(
            knowledge_store.clone(),
            proposal_store.clone(),
            None,
            ToolEvolutionRuntimeSource {
                revision: "base-a".into(),
                trusted: true,
                proposal_ids: vec![],
            },
        );
        let first = planner.sync(10).unwrap();
        assert_eq!(first.planned, 1);
        assert_eq!(planner.sync(11).unwrap().planned, 0);
        let next = ToolEvolutionPlanner::new(
            knowledge_store,
            proposal_store.clone(),
            None,
            ToolEvolutionRuntimeSource {
                revision: "base-b".into(),
                trusted: true,
                proposal_ids: vec![],
            },
        );
        let result = next.sync(12).unwrap();
        assert_eq!(result.planned, 1);
        assert_eq!(result.superseded, 1);
        let records = proposal_store.list().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records
                .iter()
                .filter(|record| record.status == ToolEvolutionChangeProposalStatus::Proposed)
                .count(),
            1
        );
    }

    #[test]
    fn trusted_candidate_claimer_materializes_only_explicit_proposal_ids() {
        let dir = tempdir().unwrap();
        let knowledge_store = KnowledgeStore::new(dir.path());
        let proposal_store = ToolEvolutionProposalStore::new(dir.path());
        let experiment_store = ToolEvolutionExperimentStore::new(dir.path());
        let _knowledge = candidate_knowledge(&knowledge_store);
        ToolEvolutionPlanner::new(
            knowledge_store.clone(),
            proposal_store.clone(),
            Some(experiment_store.clone()),
            ToolEvolutionRuntimeSource {
                revision: "a".repeat(40),
                trusted: true,
                proposal_ids: vec![],
            },
        )
        .sync(10)
        .unwrap();
        let proposal = proposal_store.list().unwrap().remove(0);
        let claimer = ToolEvolutionCandidateClaimer::new(
            knowledge_store.clone(),
            proposal_store.clone(),
            experiment_store.clone(),
            ToolEvolutionRuntimeSource {
                revision: "b".repeat(40),
                trusted: true,
                proposal_ids: vec![proposal.proposal_id.clone()],
            },
        );
        let result = claimer.sync(20);
        assert_eq!(result.materialized, 1);
        assert_eq!(result.failed, 0);
        let materialized = proposal_store.get(&proposal.proposal_id).unwrap().unwrap();
        assert_eq!(
            materialized.status,
            ToolEvolutionChangeProposalStatus::Materialized
        );
        assert_eq!(
            materialized.candidate_revision.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(claimer.sync(21).replayed, 1);

        let disabled = ToolEvolutionCandidateClaimer::new(
            knowledge_store,
            proposal_store,
            experiment_store,
            ToolEvolutionRuntimeSource {
                revision: "c".repeat(40),
                trusted: false,
                proposal_ids: vec![proposal.proposal_id],
            },
        )
        .sync(30);
        assert!(!disabled.enabled);
        assert_eq!(disabled.considered, 0);
    }

    #[test]
    fn proposal_materialization_creates_single_experiment() {
        let dir = tempdir().unwrap();
        let knowledge_store = KnowledgeStore::new(dir.path());
        candidate_knowledge(&knowledge_store);
        let proposal_store = ToolEvolutionProposalStore::new(dir.path());
        let experiment_store = ToolEvolutionExperimentStore::new(dir.path());
        let planner = ToolEvolutionPlanner::new(
            knowledge_store.clone(),
            proposal_store.clone(),
            Some(experiment_store.clone()),
            ToolEvolutionRuntimeSource {
                revision: "base-a".into(),
                trusted: true,
                proposal_ids: vec![],
            },
        );
        planner.sync(10).unwrap();
        let proposal = proposal_store.list().unwrap().remove(0);
        let first = materialize_tool_evolution_proposal(
            &knowledge_store,
            &proposal_store,
            &experiment_store,
            &proposal.proposal_id,
            "base-a",
            "candidate-b",
            20,
        )
        .unwrap();
        assert!(first.0);
        assert_eq!(
            first.1.status,
            ToolEvolutionChangeProposalStatus::Materialized
        );
        let second = materialize_tool_evolution_proposal(
            &knowledge_store,
            &proposal_store,
            &experiment_store,
            &proposal.proposal_id,
            "base-a",
            "candidate-b",
            21,
        )
        .unwrap();
        assert!(second.0);
        assert_eq!(experiment_store.list().unwrap().len(), 1);
        assert_eq!(
            first.2.unwrap().experiment_id,
            second.2.unwrap().experiment_id
        );
    }

    #[test]
    fn proposal_materialization_transaction_rolls_back_failed_proposal_write() {
        let dir = tempdir().unwrap();
        let knowledge_store = KnowledgeStore::new(dir.path());
        candidate_knowledge(&knowledge_store);
        let proposal_store = ToolEvolutionProposalStore::new(dir.path());
        let experiment_store = ToolEvolutionExperimentStore::new(dir.path());
        ToolEvolutionPlanner::new(
            knowledge_store.clone(),
            proposal_store.clone(),
            Some(experiment_store.clone()),
            ToolEvolutionRuntimeSource {
                revision: "base-atomic".into(),
                trusted: true,
                proposal_ids: vec![],
            },
        )
        .sync(10)
        .unwrap();
        let proposal = proposal_store.list().unwrap().remove(0);
        let blocking_temp = dir.path().join("tool-evolution-proposals.json.tmp");
        fs::create_dir(&blocking_temp).unwrap();

        assert!(materialize_tool_evolution_proposal(
            &knowledge_store,
            &proposal_store,
            &experiment_store,
            &proposal.proposal_id,
            "base-atomic",
            "candidate-atomic",
            20,
        )
        .is_err());
        assert_eq!(
            proposal_store
                .get(&proposal.proposal_id)
                .unwrap()
                .unwrap()
                .status,
            ToolEvolutionChangeProposalStatus::Proposed
        );
        assert!(experiment_store.list().unwrap().is_empty());
        assert!(!lifecycle_transaction_file_path(dir.path()).exists());

        fs::remove_dir(&blocking_temp).unwrap();
        let retry = materialize_tool_evolution_proposal(
            &knowledge_store,
            &proposal_store,
            &experiment_store,
            &proposal.proposal_id,
            "base-atomic",
            "candidate-atomic",
            21,
        )
        .unwrap();
        assert!(retry.0);
        assert_eq!(
            retry.1.status,
            ToolEvolutionChangeProposalStatus::Materialized
        );
        assert_eq!(experiment_store.list().unwrap().len(), 1);
    }

    #[test]
    fn experiment_requires_quality_and_material_efficiency_improvement() {
        let dir = tempdir().unwrap();
        let store = ToolEvolutionExperimentStore::new(dir.path());
        let experiment = store
            .plan(ToolEvolutionExperimentPlan {
                knowledge_id: "knowledge".into(),
                tool: "project_state".into(),
                proposal_type: ToolEvolutionProposalType::Performance,
                base_revision: "base-a".into(),
                candidate_revision: "candidate-b".into(),
                created_at_ms: 1,
            })
            .unwrap();
        for index in 0..5 {
            store
                .record_benchmark(
                    &experiment.experiment_id,
                    ToolEvolutionBenchmarkArm::Baseline,
                    sample(&format!("b-{index}"), "base-a", 100.0),
                    10 + index,
                )
                .unwrap();
            store
                .record_benchmark(
                    &experiment.experiment_id,
                    ToolEvolutionBenchmarkArm::Candidate,
                    sample(&format!("c-{index}"), "candidate-b", 80.0),
                    10 + index,
                )
                .unwrap();
        }
        let evaluated = store
            .evaluate(&experiment.experiment_id, "base-a", 30)
            .unwrap();
        assert_eq!(evaluated.status, ToolEvolutionExperimentStatus::Validated);
        assert_eq!(
            evaluated.decision_reason.as_deref(),
            Some("quality_preserved_with_material_efficiency_improvement")
        );
    }

    #[test]
    fn promotion_updates_knowledge_only_when_revisions_still_match() {
        let dir = tempdir().unwrap();
        let knowledge_store = KnowledgeStore::new(dir.path());
        let knowledge = candidate_knowledge(&knowledge_store);
        let experiment_store = ToolEvolutionExperimentStore::new(dir.path());
        let experiment = create_tool_evolution_experiment(
            &knowledge_store,
            &experiment_store,
            &knowledge.id,
            "base-a",
            "candidate-b",
            1,
        )
        .unwrap();
        for index in 0..5 {
            experiment_store
                .record_benchmark(
                    &experiment.experiment_id,
                    ToolEvolutionBenchmarkArm::Baseline,
                    sample(&format!("promote-b-{index}"), "base-a", 100.0),
                    10 + index,
                )
                .unwrap();
            experiment_store
                .record_benchmark(
                    &experiment.experiment_id,
                    ToolEvolutionBenchmarkArm::Candidate,
                    sample(&format!("promote-c-{index}"), "candidate-b", 80.0),
                    10 + index,
                )
                .unwrap();
        }
        let promoted = promote_tool_evolution_experiment(
            &knowledge_store,
            &experiment_store,
            &experiment.experiment_id,
            "base-a",
            "candidate-b",
            30,
        )
        .unwrap();
        assert!(promoted.promoted);
        assert_eq!(
            promoted.experiment.status,
            ToolEvolutionExperimentStatus::Promoted
        );
        assert_eq!(
            promoted.knowledge.as_ref().map(|record| record.status),
            Some(KnowledgeStatus::Promoted)
        );
        let replay = promote_tool_evolution_experiment(
            &knowledge_store,
            &experiment_store,
            &experiment.experiment_id,
            "base-a",
            "candidate-b",
            31,
        )
        .unwrap();
        assert!(replay.promoted);
    }

    #[test]
    fn promotion_rolls_back_knowledge_when_candidate_revision_changed() {
        let dir = tempdir().unwrap();
        let knowledge_store = KnowledgeStore::new(dir.path());
        let knowledge = candidate_knowledge(&knowledge_store);
        let experiment_store = ToolEvolutionExperimentStore::new(dir.path());
        let experiment = create_tool_evolution_experiment(
            &knowledge_store,
            &experiment_store,
            &knowledge.id,
            "base-a",
            "candidate-b",
            1,
        )
        .unwrap();
        for index in 0..5 {
            experiment_store
                .record_benchmark(
                    &experiment.experiment_id,
                    ToolEvolutionBenchmarkArm::Baseline,
                    sample(&format!("stale-b-{index}"), "base-a", 100.0),
                    10 + index,
                )
                .unwrap();
            experiment_store
                .record_benchmark(
                    &experiment.experiment_id,
                    ToolEvolutionBenchmarkArm::Candidate,
                    sample(&format!("stale-c-{index}"), "candidate-b", 80.0),
                    10 + index,
                )
                .unwrap();
        }
        let result = promote_tool_evolution_experiment(
            &knowledge_store,
            &experiment_store,
            &experiment.experiment_id,
            "base-a",
            "candidate-changed",
            30,
        )
        .unwrap();
        assert!(!result.promoted);
        assert_eq!(
            result.experiment.status,
            ToolEvolutionExperimentStatus::Stale
        );
        assert_eq!(
            result.experiment.decision_reason.as_deref(),
            Some("candidate_revision_changed")
        );
        assert_eq!(
            knowledge_store.get(&knowledge.id).unwrap().unwrap().status,
            KnowledgeStatus::Candidate
        );
    }

    #[test]
    fn experiment_rejects_replayed_sample_with_changed_metrics_and_marks_stale_base() {
        let dir = tempdir().unwrap();
        let store = ToolEvolutionExperimentStore::new(dir.path());
        let experiment = store
            .plan(ToolEvolutionExperimentPlan {
                knowledge_id: "knowledge".into(),
                tool: "project_state".into(),
                proposal_type: ToolEvolutionProposalType::Performance,
                base_revision: "base-a".into(),
                candidate_revision: "candidate-b".into(),
                created_at_ms: 1,
            })
            .unwrap();
        let original = sample("same", "base-a", 100.0);
        store
            .record_benchmark(
                &experiment.experiment_id,
                ToolEvolutionBenchmarkArm::Baseline,
                original.clone(),
                2,
            )
            .unwrap();
        store
            .record_benchmark(
                &experiment.experiment_id,
                ToolEvolutionBenchmarkArm::Baseline,
                original,
                3,
            )
            .unwrap();
        assert_eq!(
            store
                .get(&experiment.experiment_id)
                .unwrap()
                .unwrap()
                .baseline
                .samples,
            1
        );
        let error = store
            .record_benchmark(
                &experiment.experiment_id,
                ToolEvolutionBenchmarkArm::Baseline,
                sample("same", "base-a", 99.0),
                4,
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("replayed with different metrics"));
        let stale = store
            .evaluate(&experiment.experiment_id, "base-changed", 5)
            .unwrap();
        assert_eq!(stale.status, ToolEvolutionExperimentStatus::Stale);
        assert_eq!(
            stale.decision_reason.as_deref(),
            Some("base_revision_changed")
        );
    }
}
