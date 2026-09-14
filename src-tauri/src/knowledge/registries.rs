use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::model::{KnowledgeArtifactTarget, KnowledgeEvidence, KnowledgeStatus};
use super::store::{KnowledgeStore, KnowledgeStoreError};
use super::tool_evolution::{
    ToolEvolutionChangeProposalStatus, ToolEvolutionExperimentStatus, ToolEvolutionExperimentStore,
    ToolEvolutionProposalStore,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStrategyRecord {
    pub knowledge_id: String,
    pub tool: String,
    pub status: KnowledgeStatus,
    pub confidence: f64,
    pub trigger: Map<String, Value>,
    pub action: Map<String, Value>,
    pub evidence: KnowledgeEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStrategySnapshot {
    pub revision: String,
    pub strategies: Vec<ToolStrategyRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionChangeProposalSummary {
    pub proposal_id: String,
    pub status: ToolEvolutionChangeProposalStatus,
    pub base_revision: String,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionExperimentSummary {
    pub experiment_id: String,
    pub status: ToolEvolutionExperimentStatus,
    pub base_revision: String,
    pub candidate_revision: String,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionRecord {
    pub knowledge_id: String,
    pub tool: String,
    pub status: KnowledgeStatus,
    pub confidence: f64,
    pub proposal_type: String,
    pub problem: String,
    pub proposal: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_proposal: Option<ToolEvolutionChangeProposalSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_experiment: Option<ToolEvolutionExperimentSummary>,
    pub evidence: KnowledgeEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvolutionSnapshot {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_revision: Option<String>,
    pub candidates: Vec<ToolEvolutionRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolvedSkillBase {
    pub source: String,
    pub name: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolvedSkillRecord {
    pub knowledge_id: String,
    pub name: String,
    pub status: KnowledgeStatus,
    pub confidence: f64,
    pub base: EvolvedSkillBase,
    pub generation: u64,
    pub overlay: Map<String, Value>,
    pub evidence: KnowledgeEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvolvedSkillSnapshot {
    pub revision: String,
    pub skills: Vec<EvolvedSkillRecord>,
}

fn tool_name(trigger: &Map<String, Value>) -> String {
    trigger
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn metadata_string(metadata: Option<&Map<String, Value>>, key: &str) -> String {
    metadata
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[derive(Debug, Clone)]
pub struct ToolStrategyRegistry {
    store: KnowledgeStore,
}

impl ToolStrategyRegistry {
    pub fn new(store: KnowledgeStore) -> Self {
        Self { store }
    }

    pub fn snapshot(&self) -> Result<ToolStrategySnapshot, KnowledgeStoreError> {
        let revision = self.store.revision()?;
        let mut strategies = self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.target == KnowledgeArtifactTarget::ToolStrategy)
            .map(|record| ToolStrategyRecord {
                knowledge_id: record.id,
                tool: tool_name(&record.trigger),
                status: record.status,
                confidence: record.confidence,
                trigger: record.trigger,
                action: record.recommended_action,
                evidence: record.evidence,
            })
            .collect::<Vec<_>>();
        strategies.sort_by(|left, right| left.knowledge_id.cmp(&right.knowledge_id));
        Ok(ToolStrategySnapshot {
            revision,
            strategies,
        })
    }
}

fn proposal_status_rank(status: ToolEvolutionChangeProposalStatus) -> u8 {
    match status {
        ToolEvolutionChangeProposalStatus::Materialized => 3,
        ToolEvolutionChangeProposalStatus::Proposed => 2,
        ToolEvolutionChangeProposalStatus::Superseded => 1,
        ToolEvolutionChangeProposalStatus::Dismissed => 0,
    }
}

#[derive(Debug, Clone)]
pub struct ToolEvolutionRegistry {
    store: KnowledgeStore,
    experiment_store: Option<ToolEvolutionExperimentStore>,
    proposal_store: Option<ToolEvolutionProposalStore>,
}

impl ToolEvolutionRegistry {
    pub fn new(store: KnowledgeStore) -> Self {
        Self {
            store,
            experiment_store: None,
            proposal_store: None,
        }
    }

    pub fn with_lifecycle_stores(
        store: KnowledgeStore,
        experiment_store: ToolEvolutionExperimentStore,
        proposal_store: ToolEvolutionProposalStore,
    ) -> Self {
        Self {
            store,
            experiment_store: Some(experiment_store),
            proposal_store: Some(proposal_store),
        }
    }

    pub fn snapshot(&self) -> Result<ToolEvolutionSnapshot, KnowledgeStoreError> {
        let revision = self.store.revision()?;
        let (proposals, proposal_revision) = if let Some(store) = &self.proposal_store {
            (
                store
                    .list()
                    .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?,
                Some(
                    store
                        .revision()
                        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?,
                ),
            )
        } else {
            (Vec::new(), None)
        };
        let (experiments, experiment_revision) = if let Some(store) = &self.experiment_store {
            (
                store
                    .list()
                    .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?,
                Some(
                    store
                        .revision()
                        .map_err(|error| KnowledgeStoreError::Validation(error.to_string()))?,
                ),
            )
        } else {
            (Vec::new(), None)
        };
        let mut latest_proposals = BTreeMap::<String, ToolEvolutionChangeProposalSummary>::new();
        for proposal in proposals {
            let summary = ToolEvolutionChangeProposalSummary {
                proposal_id: proposal.proposal_id,
                status: proposal.status,
                base_revision: proposal.base_revision,
                updated_at_ms: proposal.updated_at_ms,
                candidate_revision: proposal.candidate_revision,
                experiment_id: proposal.experiment_id,
            };
            let replace = latest_proposals
                .get(&proposal.knowledge_id)
                .is_none_or(|current| {
                    summary.updated_at_ms > current.updated_at_ms
                        || (summary.updated_at_ms == current.updated_at_ms
                            && proposal_status_rank(summary.status)
                                > proposal_status_rank(current.status))
                        || (summary.updated_at_ms == current.updated_at_ms
                            && proposal_status_rank(summary.status)
                                == proposal_status_rank(current.status)
                            && summary.proposal_id > current.proposal_id)
                });
            if replace {
                latest_proposals.insert(proposal.knowledge_id, summary);
            }
        }
        let mut latest_experiments = BTreeMap::<String, ToolEvolutionExperimentSummary>::new();
        for experiment in experiments {
            let summary = ToolEvolutionExperimentSummary {
                experiment_id: experiment.experiment_id,
                status: experiment.status,
                base_revision: experiment.base_revision,
                candidate_revision: experiment.candidate_revision,
                updated_at_ms: experiment.updated_at_ms,
                decision_reason: experiment.decision_reason,
            };
            let replace = latest_experiments
                .get(&experiment.knowledge_id)
                .is_none_or(|current| {
                    summary.updated_at_ms > current.updated_at_ms
                        || (summary.updated_at_ms == current.updated_at_ms
                            && summary.experiment_id > current.experiment_id)
                });
            if replace {
                latest_experiments.insert(experiment.knowledge_id, summary);
            }
        }
        let mut candidates = self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.target == KnowledgeArtifactTarget::ToolEvolution)
            .map(|record| {
                let proposal_type = metadata_string(record.metadata.as_ref(), "proposalType");
                ToolEvolutionRecord {
                    knowledge_id: record.id.clone(),
                    tool: tool_name(&record.trigger),
                    status: record.status,
                    confidence: record.confidence,
                    proposal_type: if proposal_type.is_empty() {
                        "performance".into()
                    } else {
                        proposal_type
                    },
                    problem: record.hypothesis,
                    proposal: record.recommended_action,
                    evidence: record.evidence,
                    latest_proposal: latest_proposals.remove(&record.id),
                    latest_experiment: latest_experiments.remove(&record.id),
                }
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.knowledge_id.cmp(&right.knowledge_id));
        Ok(ToolEvolutionSnapshot {
            revision,
            proposal_revision,
            experiment_revision,
            candidates,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EvolvedSkillRegistry {
    store: KnowledgeStore,
}

impl EvolvedSkillRegistry {
    pub fn new(store: KnowledgeStore) -> Self {
        Self { store }
    }

    pub fn snapshot(
        &self,
        include_non_promoted: bool,
    ) -> Result<EvolvedSkillSnapshot, KnowledgeStoreError> {
        let revision = self.store.revision()?;
        let mut skills = self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.target == KnowledgeArtifactTarget::EvolvedSkill)
            .filter(|record| include_non_promoted || record.status == KnowledgeStatus::Promoted)
            .map(|record| {
                let metadata = record.metadata.as_ref();
                let base = metadata
                    .and_then(|value| value.get("base"))
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                let generation = metadata
                    .and_then(|value| value.get("generation"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                EvolvedSkillRecord {
                    knowledge_id: record.id,
                    name: metadata_string(metadata, "name"),
                    status: record.status,
                    confidence: record.confidence,
                    base,
                    generation,
                    overlay: record.recommended_action,
                    evidence: record.evidence,
                }
            })
            .collect::<Vec<_>>();
        skills.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.knowledge_id.cmp(&right.knowledge_id))
        });
        Ok(EvolvedSkillSnapshot { revision, skills })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::knowledge::{ExperienceCompiler, ExperienceObservation, ExperienceOutcome};

    fn observation(tool: &str, event_id: &str, features: Value) -> ExperienceObservation {
        ExperienceObservation {
            event_id: event_id.into(),
            tool: tool.into(),
            outcome: ExperienceOutcome::Success,
            duration_ms: 2_000,
            request_bytes: None,
            response_bytes: None,
            error_code: None,
            features: features.as_object().cloned(),
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
    fn tool_evolution_registry_exposes_latest_lifecycle_summaries() {
        let dir = tempdir().unwrap();
        let store = KnowledgeStore::new(dir.path());
        let mut record = ExperienceCompiler
            .compile(&[observation(
                "project_state",
                "lifecycle-event",
                json!({"phase_baseline_capture_ms": 1_500}),
            )])
            .unwrap()
            .remove(0);
        record.status = KnowledgeStatus::Candidate;
        store.upsert(record.clone()).unwrap();

        let proposal_store = ToolEvolutionProposalStore::new(dir.path());
        let proposal = proposal_store.plan(&record, &"a".repeat(40), 10).unwrap();
        let experiment_store = ToolEvolutionExperimentStore::new(dir.path());
        let experiment = experiment_store
            .plan(crate::knowledge::ToolEvolutionExperimentPlan {
                knowledge_id: record.id.clone(),
                tool: "project_state".into(),
                proposal_type: Default::default(),
                base_revision: "a".repeat(40),
                candidate_revision: "b".repeat(40),
                created_at_ms: 20,
            })
            .unwrap();

        let snapshot =
            ToolEvolutionRegistry::with_lifecycle_stores(store, experiment_store, proposal_store)
                .snapshot()
                .unwrap();
        assert!(snapshot.proposal_revision.is_some());
        assert!(snapshot.experiment_revision.is_some());
        let candidate = snapshot
            .candidates
            .iter()
            .find(|candidate| candidate.knowledge_id == record.id)
            .unwrap();
        assert_eq!(
            candidate.latest_proposal.as_ref().unwrap().proposal_id,
            proposal.proposal_id
        );
        assert_eq!(
            candidate.latest_experiment.as_ref().unwrap().experiment_id,
            experiment.experiment_id
        );
    }

    #[test]
    fn registries_expose_target_specific_read_only_views() {
        let dir = tempdir().unwrap();
        let store = KnowledgeStore::new(dir.path());
        for record in ExperienceCompiler
            .compile(&[
                observation(
                    "search_text",
                    "search-event",
                    json!({"calculate_total": true, "files_considered": 800}),
                ),
                observation(
                    "project_state",
                    "project-event",
                    json!({"phase_baseline_capture_ms": 1_500}),
                ),
            ])
            .unwrap()
        {
            store.upsert(record).unwrap();
        }

        let strategies = ToolStrategyRegistry::new(store.clone()).snapshot().unwrap();
        let evolution = ToolEvolutionRegistry::new(store.clone())
            .snapshot()
            .unwrap();
        let skills = EvolvedSkillRegistry::new(store).snapshot(false).unwrap();
        assert_eq!(strategies.strategies.len(), 1);
        assert_eq!(strategies.strategies[0].tool, "search_text");
        assert_eq!(evolution.candidates.len(), 1);
        assert_eq!(evolution.candidates[0].tool, "project_state");
        assert!(skills.skills.is_empty());
        assert_eq!(strategies.revision, evolution.revision);
    }
}
