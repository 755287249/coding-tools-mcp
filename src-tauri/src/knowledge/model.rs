use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type ExperienceFeatures = Map<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStatus {
    Observed,
    Candidate,
    Shadow,
    Validated,
    Promoted,
    Rejected,
    Deprecated,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeArtifactTarget {
    ToolStrategy,
    RuntimePolicy,
    ToolEvolution,
    EvolvedSkill,
    ProductOptimization,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeScope {
    Runtime,
    Workspace,
    User,
    Global,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeCompatibility {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_capability: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEvidence {
    pub observations: u64,
    pub successes: u64,
    pub failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_successes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_failures: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_successes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_failures: Option<u64>,
    pub total_duration_ms: u64,
    pub total_request_bytes: u64,
    pub total_response_bytes: u64,
    pub first_seen_at_ms: u64,
    pub last_seen_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_context_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_context_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_boot_context_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeRecord {
    pub id: String,
    pub schema_version: u8,
    pub scope: KnowledgeScope,
    pub target: KnowledgeArtifactTarget,
    pub status: KnowledgeStatus,
    pub trigger: Map<String, Value>,
    pub hypothesis: String,
    pub recommended_action: Map<String, Value>,
    pub evidence: KnowledgeEvidence,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<KnowledgeCompatibility>,
    pub source_event_ids: Vec<String>,
    pub counterexample_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceOutcome {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOutcome {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeTaskOutcome {
    VerifiedSuccess,
    UnverifiedSuccess,
    Failure,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceCanary {
    pub knowledge_id: String,
    pub implementation: String,
    pub eligible: bool,
    pub stage: String,
    pub selected: bool,
    pub applied: bool,
    pub bucket: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceSkillAttribution {
    pub source: String,
    pub name: String,
    pub content_sha256: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceObservation {
    pub event_id: String,
    pub tool: String,
    pub outcome: ExperienceOutcome,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<ExperienceFeatures>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_outcome: Option<EvidenceOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_outcome: Option<EvidenceOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary: Option<ExperienceCanary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_context_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<ExperienceSkillAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_boot_id: Option<String>,
    pub timestamp_ms: u64,
}
