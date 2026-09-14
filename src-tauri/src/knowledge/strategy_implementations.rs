use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::model::{KnowledgeArtifactTarget, KnowledgeRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStrategyImplementationId {
    SearchExactTotalFastTail,
    EvolvedSkillAppendGuidanceV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStrategyImplementationSafety {
    SemanticsPreserving,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolStrategyImplementationSpec {
    pub id: ToolStrategyImplementationId,
    pub tool: &'static str,
    pub safety: ToolStrategyImplementationSafety,
}

const SEARCH_EXACT_TOTAL_FAST_TAIL: ToolStrategyImplementationSpec =
    ToolStrategyImplementationSpec {
        id: ToolStrategyImplementationId::SearchExactTotalFastTail,
        tool: "search_text",
        safety: ToolStrategyImplementationSafety::SemanticsPreserving,
    };

pub fn resolve_tool_strategy_implementation(
    record: &KnowledgeRecord,
    tool: &str,
    args: &Value,
) -> Option<ToolStrategyImplementationSpec> {
    if record.target != KnowledgeArtifactTarget::ToolStrategy || tool != "search_text" {
        return None;
    }
    let declared = record
        .recommended_action
        .get("semantic_preserving_implementation")
        .and_then(Value::as_str);
    let legacy = record
        .recommended_action
        .get("recommendation")
        .and_then(Value::as_str)
        == Some("prefer_bounded_search");
    let trigger_tool = record.trigger.get("tool").and_then(Value::as_str);
    let calculate_total = args.get("calculate_total").and_then(Value::as_bool) == Some(true);
    let count_only = args.get("count_only").and_then(Value::as_bool) == Some(true);
    let has_query = args
        .get("query")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        || args
            .get("queries")
            .and_then(Value::as_array)
            .is_some_and(|values| !values.is_empty());
    (trigger_tool == Some("search_text")
        && (declared == Some("search_exact_total_fast_tail") || legacy)
        && calculate_total
        && !count_only
        && has_query)
        .then_some(SEARCH_EXACT_TOTAL_FAST_TAIL)
}

pub fn tool_strategy_implementation_snapshot() -> Value {
    let implementations = vec![SEARCH_EXACT_TOTAL_FAST_TAIL];
    let encoded = serde_json::to_vec(&implementations).unwrap_or_default();
    serde_json::json!({
        "count": implementations.len(),
        "revision": format!("{:x}", Sha256::digest(encoded)),
        "implementations": implementations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeEvidence, KnowledgeScope, KnowledgeStatus};
    use serde_json::json;

    fn strategy() -> KnowledgeRecord {
        KnowledgeRecord {
            id: "strategy".into(),
            schema_version: 1,
            scope: KnowledgeScope::Workspace,
            target: KnowledgeArtifactTarget::ToolStrategy,
            status: KnowledgeStatus::Validated,
            trigger: json!({"tool":"search_text","calculate_total":true})
                .as_object()
                .unwrap()
                .clone(),
            hypothesis: "exact total tail".into(),
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
                total_duration_ms: 10_000,
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
    fn resolver_only_exposes_registered_semantics_preserving_search_strategy() {
        assert_eq!(
            resolve_tool_strategy_implementation(
                &strategy(),
                "search_text",
                &json!({"query":"x","calculate_total":true})
            )
            .unwrap()
            .id,
            ToolStrategyImplementationId::SearchExactTotalFastTail
        );
        assert!(resolve_tool_strategy_implementation(
            &strategy(),
            "search_text",
            &json!({"query":"x","calculate_total":true,"count_only":true})
        )
        .is_none());
        assert!(resolve_tool_strategy_implementation(
            &strategy(),
            "read_file",
            &json!({"path":"x"})
        )
        .is_none());
    }
}
