use serde_json::{Map, Value};

use super::model::{
    EvidenceOutcome, ExperienceCanary, ExperienceObservation, ExperienceOutcome,
    ExperienceSkillAttribution,
};

const TELEMETRY_FEATURES: &[&str] = &[
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
    "graph_action",
    "detached",
    "reattached",
    "graph_yield_ms",
    "graph_wait_ms",
    "parallel_conflict",
    "parallel_serialized",
    "exact_total_fast_tail",
    "exact_total_fast_tail_skipped_match_details",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExperienceAttribution {
    pub operation_id: Option<String>,
    pub task_id: Option<String>,
    pub conversation_context_id: Option<String>,
    pub skill: Option<ExperienceSkillAttribution>,
    pub canary: Option<ExperienceCanary>,
}

fn bounded_number(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value.as_f64().and_then(|number| {
                (number.is_finite() && number >= 0.0).then_some(number.min(u64::MAX as f64) as u64)
            })
        })
}

fn scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::String(_) | Value::Number(_) | Value::Bool(_)
    )
}

fn workspace_id(record: &Value) -> Option<&str> {
    record
        .get("selected_workspace_id")
        .or_else(|| record.get("workspace_id"))
        .and_then(Value::as_str)
}

pub fn tool_usage_record_to_observation(
    record: &Value,
    expected_workspace_id: Option<&str>,
    attribution: &ExperienceAttribution,
) -> Option<ExperienceObservation> {
    if record.get("event_type").and_then(Value::as_str) != Some("tool_call") {
        return None;
    }
    let tool = record.get("tool").and_then(Value::as_str)?.trim();
    let event_id = record
        .get("diagnostic_event_id")
        .and_then(Value::as_str)?
        .trim();
    if tool.is_empty() || event_id.is_empty() {
        return None;
    }
    if expected_workspace_id.is_some_and(|expected| workspace_id(record) != Some(expected)) {
        return None;
    }

    let mut features = Map::new();
    for key in TELEMETRY_FEATURES {
        if let Some(value) = record.get(*key).filter(|value| scalar(value)) {
            features.insert((*key).into(), value.clone());
        }
    }
    if record.get("recovery_attempt").and_then(Value::as_bool) == Some(true) {
        features.insert("retry_attempt".into(), Value::Bool(true));
    }

    let verification_outcome = match record.get("verification_ok").and_then(Value::as_bool) {
        Some(true) => Some(EvidenceOutcome::Passed),
        Some(false) => Some(EvidenceOutcome::Failed),
        None => None,
    };
    let recovery_outcome = if record.get("recovery_attempt").and_then(Value::as_bool) == Some(true)
    {
        Some(
            if record.get("recovery_succeeded").and_then(Value::as_bool) == Some(true) {
                EvidenceOutcome::Passed
            } else {
                EvidenceOutcome::Failed
            },
        )
    } else {
        None
    };
    let success = record.get("outcome").and_then(Value::as_str) == Some("success")
        && record.get("verification_ok").and_then(Value::as_bool) != Some(false);

    let record_canary = match (
        record.get("knowledge_canary_id").and_then(Value::as_str),
        record
            .get("knowledge_canary_implementation")
            .and_then(Value::as_str),
        record.get("knowledge_canary_stage").and_then(Value::as_str),
    ) {
        (Some(knowledge_id), Some(implementation), Some(stage))
            if !knowledge_id.is_empty()
                && knowledge_id.len() <= 128
                && !implementation.is_empty()
                && implementation.len() <= 64
                && matches!(stage, "canary" | "promoted") =>
        {
            Some(ExperienceCanary {
                knowledge_id: knowledge_id.to_string(),
                implementation: implementation.to_string(),
                eligible: record
                    .get("knowledge_canary_eligible")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                stage: stage.to_string(),
                selected: record
                    .get("knowledge_canary_selected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                applied: record
                    .get("knowledge_canary_applied")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                bucket: bounded_number(record.get("knowledge_canary_bucket"))
                    .unwrap_or(100)
                    .min(100) as u32,
            })
        }
        _ => None,
    };
    let canary = record_canary.or_else(|| {
        (tool == "edit")
            .then(|| attribution.canary.clone())
            .flatten()
    });

    Some(ExperienceObservation {
        event_id: event_id.into(),
        tool: tool.into(),
        outcome: if success {
            ExperienceOutcome::Success
        } else {
            ExperienceOutcome::Failure
        },
        duration_ms: bounded_number(record.get("duration_ms")).unwrap_or(0),
        request_bytes: bounded_number(record.get("request_json_bytes")),
        response_bytes: bounded_number(record.get("response_json_bytes")),
        error_code: record
            .get("error_code")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        features: (!features.is_empty()).then_some(features),
        verification_outcome,
        recovery_outcome,
        canary,
        operation_id: attribution.operation_id.clone(),
        task_id: attribution.task_id.clone(),
        conversation_context_id: attribution.conversation_context_id.clone(),
        skill: attribution.skill.clone(),
        runtime_boot_id: record
            .get("runtime_boot_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        timestamp_ms: bounded_number(record.get("completed_ts_ms"))
            .or_else(|| bounded_number(record.get("timestamp_ms")))
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn canonical_rust_tool_usage_converts_to_sanitized_observation() {
        let record = json!({
            "event_type": "tool_call",
            "diagnostic_event_id": "event-1",
            "workspace_id": "workspace-a",
            "tool": "search_text",
            "outcome": "success",
            "duration_ms": 1_250,
            "request_json_bytes": 100,
            "response_json_bytes": 200,
            "runtime_boot_id": "boot-a",
            "verification_ok": true,
            "calculate_total": true,
            "files_considered": 800,
            "exact_total_fast_tail": true,
            "exact_total_fast_tail_skipped_match_details": 77,
            "knowledge_canary_id": "knowledge-abc",
            "knowledge_canary_implementation": "search_exact_total_fast_tail",
            "knowledge_canary_eligible": true,
            "knowledge_canary_stage": "canary",
            "knowledge_canary_selected": true,
            "knowledge_canary_applied": false,
            "knowledge_canary_bucket": 42,
            "argument_path": "must-not-leak",
            "command_preview": "must-not-leak"
        });
        let observation = tool_usage_record_to_observation(
            &record,
            Some("workspace-a"),
            &ExperienceAttribution {
                task_id: Some("task-a".into()),
                conversation_context_id: Some("conversation-a".into()),
                skill: Some(ExperienceSkillAttribution {
                    source: "project".into(),
                    name: "demo".into(),
                    content_sha256: "a".repeat(64),
                    generation: 2,
                }),
                ..ExperienceAttribution::default()
            },
        )
        .expect("observation");
        assert_eq!(observation.outcome, ExperienceOutcome::Success);
        assert_eq!(
            observation.verification_outcome,
            Some(EvidenceOutcome::Passed)
        );
        assert_eq!(
            observation.features.as_ref().unwrap()["files_considered"],
            800
        );
        assert_eq!(
            observation.features.as_ref().unwrap()["exact_total_fast_tail_skipped_match_details"],
            77
        );
        assert_eq!(observation.skill.as_ref().unwrap().name, "demo");
        assert_eq!(observation.skill.as_ref().unwrap().generation, 2);
        let canary = observation.canary.as_ref().expect("canary metadata");
        assert_eq!(canary.knowledge_id, "knowledge-abc");
        assert_eq!(canary.implementation, "search_exact_total_fast_tail");
        assert!(canary.eligible && canary.selected && !canary.applied);
        assert_eq!(canary.stage, "canary");
        assert_eq!(canary.bucket, 42);
        let serialized = serde_json::to_string(&observation).unwrap();
        assert!(!serialized.contains("must-not-leak"));
    }

    #[test]
    fn adapter_rejects_transport_events_and_other_workspaces() {
        let transport = json!({
            "event_type": "transport_event",
            "diagnostic_event_id": "event-1",
            "workspace_id": "workspace-a",
            "tool": "server_info"
        });
        assert!(tool_usage_record_to_observation(
            &transport,
            None,
            &ExperienceAttribution::default()
        )
        .is_none());

        let tool_call = json!({
            "event_type": "tool_call",
            "diagnostic_event_id": "event-2",
            "workspace_id": "workspace-b",
            "tool": "read_file"
        });
        assert!(tool_usage_record_to_observation(
            &tool_call,
            Some("workspace-a"),
            &ExperienceAttribution::default()
        )
        .is_none());
    }
}
