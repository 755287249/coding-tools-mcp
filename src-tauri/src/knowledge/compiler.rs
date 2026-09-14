use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::model::{
    EvidenceOutcome, ExperienceObservation, ExperienceOutcome, ExperienceSkillAttribution,
    KnowledgeArtifactTarget, KnowledgeEvidence, KnowledgeRecord, KnowledgeScope, KnowledgeStatus,
};
use super::store::{knowledge_evidence_candidate_ready, knowledge_record_id, KnowledgeStoreError};

const MAX_CONTEXT_IDS: usize = 256;
const SAFE_FEATURES: &[&str] = &[
    "calculate_total",
    "files_considered",
    "scanned_files",
    "scan_excluded_worktree_container_count",
    "returned_count",
    "phase_baseline_capture_ms",
    "phase_baseline_refresh_ms",
    "phase_harness_begin_ms",
    "phase_harness_finish_ms",
    "baseline_refresh_mode",
    "baseline_refresh_file_count",
    "recovery_succeeded",
    "error_direct_recovery_action_count",
    "retry_attempt",
    "graph_action",
    "detached",
    "reattached",
    "graph_yield_ms",
    "graph_wait_ms",
    "parallel_conflict",
    "parallel_serialized",
];
const EVOLVED_SKILL_SOURCES: &[&str] =
    &["project", "agents", "claude", "codex-user", "claude-user"];
const STALE_EDIT_GUIDANCE: &str = "When a guarded edit reports stale state, re-read the current file and rebuild the guarded edit before retrying.";
const IDENTITY_FEATURES: &[&str] = &[
    "calculate_total",
    "graph_action",
    "detached",
    "reattached",
    "parallel_conflict",
    "parallel_serialized",
];

struct Classification {
    target: KnowledgeArtifactTarget,
    hypothesis: &'static str,
    action: Map<String, Value>,
    metadata: Option<Map<String, Value>>,
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::String(_) | Value::Number(_) | Value::Bool(_)
    )
}

fn filtered_features(observation: &ExperienceObservation, allowed: &[&str]) -> Map<String, Value> {
    let mut sorted = BTreeMap::new();
    if let Some(features) = &observation.features {
        for (key, value) in features {
            if allowed.contains(&key.as_str()) && scalar(value) {
                sorted.insert(key.clone(), value.clone());
            }
        }
    }
    sorted.into_iter().collect()
}

fn identity_features(observation: &ExperienceObservation) -> Map<String, Value> {
    filtered_features(observation, IDENTITY_FEATURES)
}

fn number(features: &Map<String, Value>, key: &str) -> u64 {
    features.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn boolean(features: &Map<String, Value>, key: &str) -> Option<bool> {
    features.get(key).and_then(Value::as_bool)
}

fn string<'a>(features: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    features.get(key).and_then(Value::as_str)
}

fn classify(observations: &[ExperienceObservation]) -> Classification {
    let first = &observations[0];
    let features = observations
        .iter()
        .map(|item| filtered_features(item, SAFE_FEATURES))
        .collect::<Vec<_>>();
    let errors = observations
        .iter()
        .filter_map(|item| item.error_code.as_deref())
        .collect::<BTreeSet<_>>();

    if first.tool == "search_text"
        && observations.iter().zip(&features).any(|(item, f)| {
            boolean(f, "calculate_total") == Some(true)
                && (number(f, "files_considered") >= 500 || item.duration_ms >= 1_000)
        })
    {
        return Classification {
            target: KnowledgeArtifactTarget::ToolStrategy,
            hypothesis: "Exact total counting can dominate search cost when callers only need bounded matches.",
            action: object(json!({
                "recommendation": "prefer_bounded_search",
                "caller_intent_required": "bounded_results_only",
                "semantic_preserving_implementation": "search_exact_total_fast_tail"
            })),
            metadata: None,
        };
    }
    if first.tool == "search_text"
        && observations.iter().zip(&features).any(|(item, f)| {
            boolean(f, "calculate_total") != Some(true)
                && number(f, "files_considered") >= 5_000
                && item.duration_ms >= 5_000
        })
    {
        return Classification {
            target: KnowledgeArtifactTarget::ToolEvolution,
            hypothesis: "Broad workspace discovery can spend most of its time scanning files outside the active worktree when managed worktree containers are mixed into the workspace root.",
            action: object(json!({"proposal": "exclude_untracked_worktree_management_containers"})),
            metadata: Some(object(json!({"proposalType": "performance"}))),
        };
    }
    if first.tool == "project_state"
        && features
            .iter()
            .any(|f| number(f, "phase_baseline_capture_ms") >= 1_000)
    {
        return Classification {
            target: KnowledgeArtifactTarget::ToolEvolution,
            hypothesis: "Repeated full baseline capture is a structural tool cost and should be reduced inside the MCP implementation.",
            action: object(json!({"proposal": "incremental_or_cached_baseline"})),
            metadata: Some(object(json!({"proposalType": "performance"}))),
        };
    }
    if first.tool == "edit"
        && errors.is_empty()
        && observations.iter().zip(&features).any(|(item, f)| {
            item.outcome == ExperienceOutcome::Success
                && number(f, "phase_harness_begin_ms")
                    .saturating_add(number(f, "phase_harness_finish_ms"))
                    >= 1_000
        })
    {
        return Classification {
            target: KnowledgeArtifactTarget::ToolEvolution,
            hypothesis: "Successful guarded edits can spend more time refreshing whole-workspace Harness state than applying the verified file mutation.",
            action: object(json!({"proposal": "incremental_post_edit_baseline_refresh"})),
            metadata: Some(object(json!({"proposalType": "performance"}))),
        };
    }
    if first.tool == "exec_many"
        && observations.iter().zip(&features).any(|(item, f)| {
            let yield_ms = number(f, "graph_yield_ms");
            let wait_ms = number(f, "graph_wait_ms");
            item.outcome == ExperienceOutcome::Success
                && string(f, "graph_action") == Some("run")
                && boolean(f, "detached") == Some(true)
                && boolean(f, "reattached") == Some(true)
                && yield_ms >= 25_000
                && wait_ms >= 25_000_u64.max(yield_ms.saturating_mul(9) / 10)
        })
    {
        return Classification {
            target: KnowledgeArtifactTarget::ToolEvolution,
            hypothesis: "Retained exec_many reattachments that repeatedly exhaust the graph wait window force avoidable MCP and model round-trips while child work is still healthy.",
            action: object(json!({
                "proposal": "adaptive_retained_graph_reattach_wait",
                "preserve_initial_responsiveness": true
            })),
            metadata: Some(object(json!({"proposalType": "performance"}))),
        };
    }
    if first.tool == "edit"
        && (errors.contains("FILE_VERSION_MISMATCH")
            || errors.contains("EDIT_MATCH_COUNT_MISMATCH"))
    {
        return Classification {
            target: KnowledgeArtifactTarget::ToolStrategy,
            hypothesis: "Stale edit state should be refreshed before retrying the same mutation.",
            action: object(json!({
                "recovery": ["use_direct_recovery_action_if_available", "read_file", "retry_edit"]
            })),
            metadata: None,
        };
    }
    Classification {
        target: KnowledgeArtifactTarget::Ignore,
        hypothesis: "No promotable tool-learning pattern matched this observation group.",
        action: Map::new(),
        metadata: None,
    }
}

fn support_confidence(observations: usize) -> f64 {
    if observations == 0 {
        0.0
    } else {
        let value = observations as f64 / (observations as f64 + 5.0);
        (value * 10_000.0).round() / 10_000.0
    }
}

fn context_digest(kind: &str, value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(format!(
        "{:x}",
        Sha256::digest(format!("{kind}\0{value}").as_bytes())
    ))
}

fn context_ids<'a>(
    observations: &'a [ExperienceObservation],
    kind: &str,
    value: impl Fn(&'a ExperienceObservation) -> Option<&'a str>,
) -> Option<Vec<String>> {
    let values = observations
        .iter()
        .filter_map(|item| context_digest(kind, value(item)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_CONTEXT_IDS)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn evidence_for(observations: &[ExperienceObservation]) -> KnowledgeEvidence {
    let first = &observations[0];
    let last = observations.last().expect("non-empty observations");
    let successes = observations
        .iter()
        .filter(|item| item.outcome == ExperienceOutcome::Success)
        .count() as u64;
    KnowledgeEvidence {
        observations: observations.len() as u64,
        successes,
        failures: observations.len() as u64 - successes,
        verified_successes: Some(
            observations
                .iter()
                .filter(|item| item.verification_outcome == Some(EvidenceOutcome::Passed))
                .count() as u64,
        ),
        verification_failures: Some(
            observations
                .iter()
                .filter(|item| item.verification_outcome == Some(EvidenceOutcome::Failed))
                .count() as u64,
        ),
        recovery_successes: Some(
            observations
                .iter()
                .filter(|item| item.recovery_outcome == Some(EvidenceOutcome::Passed))
                .count() as u64,
        ),
        recovery_failures: Some(
            observations
                .iter()
                .filter(|item| item.recovery_outcome == Some(EvidenceOutcome::Failed))
                .count() as u64,
        ),
        total_duration_ms: observations
            .iter()
            .fold(0_u64, |total, item| total.saturating_add(item.duration_ms)),
        total_request_bytes: observations.iter().fold(0_u64, |total, item| {
            total.saturating_add(item.request_bytes.unwrap_or(0))
        }),
        total_response_bytes: observations.iter().fold(0_u64, |total, item| {
            total.saturating_add(item.response_bytes.unwrap_or(0))
        }),
        first_seen_at_ms: first.timestamp_ms,
        last_seen_at_ms: last.timestamp_ms,
        task_context_ids: context_ids(observations, "task", |item| item.task_id.as_deref()),
        conversation_context_ids: context_ids(observations, "conversation", |item| {
            item.conversation_context_id.as_deref()
        }),
        runtime_boot_context_ids: context_ids(observations, "runtime-boot", |item| {
            item.runtime_boot_id.as_deref()
        }),
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn evolved_skill_group_key(
    skill: &ExperienceSkillAttribution,
) -> Result<String, KnowledgeStoreError> {
    Ok(serde_json::to_string(&json!({
        "source": skill.source,
        "name": skill.name,
        "contentSha256": skill.content_sha256,
        "generation": skill.generation,
        "pattern": "stale_guarded_edit_recovery"
    }))?)
}

fn compile_evolved_skill_records(
    observations: &[ExperienceObservation],
) -> Result<Vec<KnowledgeRecord>, KnowledgeStoreError> {
    let mut groups = BTreeMap::<String, Vec<ExperienceObservation>>::new();
    for observation in observations {
        let Some(skill) = observation.skill.as_ref() else {
            continue;
        };
        if observation.tool != "edit"
            || !matches!(
                observation.error_code.as_deref(),
                Some("FILE_VERSION_MISMATCH" | "EDIT_MATCH_COUNT_MISMATCH")
            )
            || !EVOLVED_SKILL_SOURCES.contains(&skill.source.as_str())
            || skill.name.trim().is_empty()
            || skill.name.len() > 128
            || !valid_sha256(&skill.content_sha256)
            || skill.generation > 10_000
        {
            continue;
        }
        groups
            .entry(evolved_skill_group_key(skill)?)
            .or_default()
            .push(observation.clone());
    }

    let mut records = Vec::new();
    for mut grouped in groups.into_values() {
        grouped.sort_by(|left, right| {
            left.timestamp_ms
                .cmp(&right.timestamp_ms)
                .then_with(|| left.event_id.cmp(&right.event_id))
        });
        let skill = grouped[0]
            .skill
            .as_ref()
            .expect("grouped Skill attribution");
        let evidence = evidence_for(&grouped);
        let mut trigger = Map::new();
        trigger.insert("tool".into(), Value::String("edit".into()));
        trigger.insert(
            "pattern".into(),
            Value::String("stale_guarded_edit_recovery".into()),
        );
        trigger.insert("skill_source".into(), Value::String(skill.source.clone()));
        trigger.insert(
            "skill_name_sha256".into(),
            Value::String(format!("{:x}", Sha256::digest(skill.name.as_bytes()))),
        );
        trigger.insert(
            "base_content_sha256".into(),
            Value::String(skill.content_sha256.clone()),
        );
        let mut record = KnowledgeRecord {
            id: String::new(),
            schema_version: 1,
            scope: KnowledgeScope::Workspace,
            target: KnowledgeArtifactTarget::EvolvedSkill,
            status: if knowledge_evidence_candidate_ready(
                KnowledgeArtifactTarget::EvolvedSkill,
                &evidence,
            ) {
                KnowledgeStatus::Candidate
            } else {
                KnowledgeStatus::Observed
            },
            trigger,
            hypothesis: "A loaded Skill can incorporate deterministic stale-edit recovery guidance after the same friction recurs across independent contexts.".into(),
            recommended_action: object(json!({ "append_guidance": [STALE_EDIT_GUIDANCE] })),
            evidence,
            confidence: support_confidence(grouped.len()),
            compatibility: None,
            source_event_ids: grouped.iter().map(|item| item.event_id.clone()).collect(),
            counterexample_event_ids: Vec::new(),
            supersedes: None,
            superseded_by: None,
            created_at_ms: grouped[0].timestamp_ms,
            updated_at_ms: grouped.last().expect("non-empty group").timestamp_ms,
            metadata: Some(object(json!({
                "name": skill.name,
                "base": {
                    "source": skill.source,
                    "name": skill.name,
                    "contentSha256": skill.content_sha256
                },
                "generation": skill.generation.saturating_add(1),
                "pattern": "stale_guarded_edit_recovery"
            }))),
        };
        record.source_event_ids.sort();
        record.source_event_ids.dedup();
        record.id = knowledge_record_id(&record)?;
        records.push(record);
    }
    Ok(records)
}

fn group_key(observation: &ExperienceObservation) -> Result<String, KnowledgeStoreError> {
    let mut key = Map::new();
    key.insert("tool".into(), Value::String(observation.tool.clone()));
    key.insert(
        "errorCode".into(),
        observation
            .error_code
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    key.insert(
        "features".into(),
        Value::Object(identity_features(observation)),
    );
    Ok(serde_json::to_string(&Value::Object(key))?)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExperienceCompiler;

impl ExperienceCompiler {
    pub fn compile(
        &self,
        observations: &[ExperienceObservation],
    ) -> Result<Vec<KnowledgeRecord>, KnowledgeStoreError> {
        let mut groups = BTreeMap::<String, Vec<ExperienceObservation>>::new();
        for observation in observations {
            groups
                .entry(group_key(observation)?)
                .or_default()
                .push(observation.clone());
        }

        let mut records = Vec::new();
        for mut grouped in groups.into_values() {
            grouped.sort_by(|left, right| {
                left.timestamp_ms
                    .cmp(&right.timestamp_ms)
                    .then_with(|| left.event_id.cmp(&right.event_id))
            });
            let classification = classify(&grouped);
            if classification.target == KnowledgeArtifactTarget::Ignore {
                continue;
            }

            let successes = grouped
                .iter()
                .filter(|item| item.outcome == ExperienceOutcome::Success)
                .count() as u64;
            let verified_successes = grouped
                .iter()
                .filter(|item| item.verification_outcome == Some(EvidenceOutcome::Passed))
                .count() as u64;
            let verification_failures = grouped
                .iter()
                .filter(|item| item.verification_outcome == Some(EvidenceOutcome::Failed))
                .count() as u64;
            let recovery_successes = grouped
                .iter()
                .filter(|item| item.recovery_outcome == Some(EvidenceOutcome::Passed))
                .count() as u64;
            let recovery_failures = grouped
                .iter()
                .filter(|item| item.recovery_outcome == Some(EvidenceOutcome::Failed))
                .count() as u64;
            let first = &grouped[0];
            let last = grouped.last().expect("non-empty group");

            let mut trigger = Map::new();
            trigger.insert("tool".into(), Value::String(first.tool.clone()));
            if let Some(error_code) = &first.error_code {
                trigger.insert("errorCode".into(), Value::String(error_code.clone()));
            }
            trigger.extend(identity_features(first));

            let evidence = KnowledgeEvidence {
                observations: grouped.len() as u64,
                successes,
                failures: grouped.len() as u64 - successes,
                verified_successes: Some(verified_successes),
                verification_failures: Some(verification_failures),
                recovery_successes: Some(recovery_successes),
                recovery_failures: Some(recovery_failures),
                total_duration_ms: grouped
                    .iter()
                    .fold(0_u64, |total, item| total.saturating_add(item.duration_ms)),
                total_request_bytes: grouped.iter().fold(0_u64, |total, item| {
                    total.saturating_add(item.request_bytes.unwrap_or(0))
                }),
                total_response_bytes: grouped.iter().fold(0_u64, |total, item| {
                    total.saturating_add(item.response_bytes.unwrap_or(0))
                }),
                first_seen_at_ms: first.timestamp_ms,
                last_seen_at_ms: last.timestamp_ms,
                task_context_ids: context_ids(&grouped, "task", |item| item.task_id.as_deref()),
                conversation_context_ids: context_ids(&grouped, "conversation", |item| {
                    item.conversation_context_id.as_deref()
                }),
                runtime_boot_context_ids: context_ids(&grouped, "runtime-boot", |item| {
                    item.runtime_boot_id.as_deref()
                }),
            };
            let status = if knowledge_evidence_candidate_ready(classification.target, &evidence) {
                KnowledgeStatus::Candidate
            } else {
                KnowledgeStatus::Observed
            };
            let mut source_event_ids = grouped
                .iter()
                .map(|item| item.event_id.clone())
                .collect::<Vec<_>>();
            source_event_ids.sort();
            source_event_ids.dedup();
            let mut record = KnowledgeRecord {
                id: String::new(),
                schema_version: 1,
                scope: KnowledgeScope::Runtime,
                target: classification.target,
                status,
                trigger,
                hypothesis: classification.hypothesis.into(),
                recommended_action: classification.action,
                evidence,
                confidence: support_confidence(grouped.len()),
                compatibility: None,
                source_event_ids,
                counterexample_event_ids: Vec::new(),
                supersedes: None,
                superseded_by: None,
                created_at_ms: first.timestamp_ms,
                updated_at_ms: last.timestamp_ms,
                metadata: classification.metadata,
            };
            record.id = knowledge_record_id(&record)?;
            records.push(record);
        }
        records.extend(compile_evolved_skill_records(observations)?);
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(event_id: &str, timestamp_ms: u64) -> ExperienceObservation {
        ExperienceObservation {
            event_id: event_id.into(),
            tool: "search_text".into(),
            outcome: ExperienceOutcome::Success,
            duration_ms: 2_000,
            request_bytes: Some(10),
            response_bytes: Some(20),
            error_code: None,
            features: Some(object(
                json!({"calculate_total": true, "files_considered": 800, "returned_count": 10}),
            )),
            verification_outcome: Some(EvidenceOutcome::Passed),
            recovery_outcome: None,
            canary: None,
            operation_id: None,
            task_id: Some(format!("task-{event_id}")),
            conversation_context_id: Some(format!("conversation-{event_id}")),
            skill: None,
            runtime_boot_id: Some("boot-a".into()),
            timestamp_ms,
        }
    }

    #[test]
    fn metric_values_do_not_fragment_pattern_identity() {
        let mut second = observation("event-2", 2_000);
        second
            .features
            .as_mut()
            .unwrap()
            .insert("files_considered".into(), json!(9_000));
        let records = ExperienceCompiler
            .compile(&[observation("event-1", 1_000), second])
            .expect("compile");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].target, KnowledgeArtifactTarget::ToolStrategy);
        assert_eq!(records[0].trigger["calculate_total"], true);
        assert!(!records[0].trigger.contains_key("files_considered"));
        assert_eq!(records[0].evidence.observations, 2);
    }

    #[test]
    fn evolved_skill_candidate_requires_explicit_attribution_and_independent_contexts() {
        let skill = ExperienceSkillAttribution {
            source: "project".into(),
            name: "release-helper".into(),
            content_sha256: "a".repeat(64),
            generation: 0,
        };
        let make = |index: u64, conversation: &str| ExperienceObservation {
            event_id: format!("skill-edit-{index}"),
            tool: "edit".into(),
            outcome: ExperienceOutcome::Failure,
            duration_ms: 25,
            request_bytes: None,
            response_bytes: None,
            error_code: Some("FILE_VERSION_MISMATCH".into()),
            features: None,
            verification_outcome: None,
            recovery_outcome: None,
            canary: None,
            operation_id: None,
            task_id: None,
            conversation_context_id: Some(conversation.into()),
            skill: Some(skill.clone()),
            runtime_boot_id: None,
            timestamp_ms: 2_500 + index,
        };
        let same_context = (0..5)
            .map(|index| make(index, "conversation-a"))
            .collect::<Vec<_>>();
        let observed = ExperienceCompiler
            .compile(&same_context)
            .expect("compile")
            .into_iter()
            .find(|record| record.target == KnowledgeArtifactTarget::EvolvedSkill)
            .expect("evolved Skill observation");
        assert_eq!(observed.status, KnowledgeStatus::Observed);
        assert_eq!(observed.scope, KnowledgeScope::Workspace);
        assert_eq!(
            observed.metadata.as_ref().unwrap()["name"],
            "release-helper"
        );
        assert_eq!(observed.metadata.as_ref().unwrap()["generation"], 1);
        assert_eq!(
            observed.recommended_action["append_guidance"][0],
            STALE_EDIT_GUIDANCE
        );

        let diversified = (0..5)
            .map(|index| {
                make(
                    index,
                    if index < 3 {
                        "conversation-a"
                    } else {
                        "conversation-b"
                    },
                )
            })
            .collect::<Vec<_>>();
        let candidate = ExperienceCompiler
            .compile(&diversified)
            .expect("compile")
            .into_iter()
            .find(|record| record.target == KnowledgeArtifactTarget::EvolvedSkill)
            .expect("evolved Skill candidate");
        assert_eq!(candidate.status, KnowledgeStatus::Candidate);
        assert_eq!(
            candidate
                .evidence
                .conversation_context_ids
                .as_ref()
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(candidate.trigger["skill_source"], "project");
        assert_eq!(candidate.trigger["base_content_sha256"], "a".repeat(64));

        let without_skill = diversified
            .into_iter()
            .map(|mut item| {
                item.skill = None;
                item
            })
            .collect::<Vec<_>>();
        assert!(!ExperienceCompiler
            .compile(&without_skill)
            .expect("compile")
            .iter()
            .any(|record| record.target == KnowledgeArtifactTarget::EvolvedSkill));
    }

    #[test]
    fn tool_evolution_candidate_requires_independent_contexts() {
        let observations = (0..5)
            .map(|index| ExperienceObservation {
                event_id: format!("event-{index}"),
                tool: "project_state".into(),
                outcome: ExperienceOutcome::Success,
                duration_ms: 2_000,
                request_bytes: None,
                response_bytes: None,
                error_code: None,
                features: Some(object(json!({"phase_baseline_capture_ms": 1_500}))),
                verification_outcome: None,
                recovery_outcome: None,
                canary: None,
                operation_id: None,
                task_id: None,
                conversation_context_id: Some(
                    if index < 3 {
                        "conversation-a"
                    } else {
                        "conversation-b"
                    }
                    .into(),
                ),
                skill: None,
                runtime_boot_id: None,
                timestamp_ms: 1_000 + index,
            })
            .collect::<Vec<_>>();
        let record = ExperienceCompiler
            .compile(&observations)
            .expect("compile")
            .remove(0);
        assert_eq!(record.target, KnowledgeArtifactTarget::ToolEvolution);
        assert_eq!(record.status, KnowledgeStatus::Candidate);
        assert_eq!(record.evidence.conversation_context_ids.unwrap().len(), 2);
    }
}
