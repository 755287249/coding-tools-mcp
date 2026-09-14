use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub use super::registry_metadata::{
    ALLOWED_TOOLS, CORE_READ_ONLY_TOOLS, CORE_TOOLS, GUARDED_CORE_TOOLS, P0_TOOLS, READ_ONLY_TOOLS,
};

pub fn is_allowed_tool(name: &str) -> bool {
    ALLOWED_TOOLS.contains(&name)
}

pub fn is_mcp_only_tool(name: &str) -> bool {
    matches!(name, "switch_workspace_folder" | "conversation_bootstrap")
}

pub fn canonical_tool_name(name: &str) -> &str {
    match name {
        "edit_file" | "edit_many" => "edit",
        _ => name,
    }
}

pub fn normalize_tool_profile(profile: &str) -> &'static str {
    match profile {
        "advanced" => "advanced",
        "read-only" => "read-only",
        "compat-readonly-all" => "compat-readonly-all",
        "guarded-core" => "guarded-core",
        "trusted-core" | "core" => "trusted-core",
        _ => "trusted-core",
    }
}

pub fn resolve_tool_profile(profile: &str, permission_mode: &str) -> &'static str {
    let normalized = normalize_tool_profile(profile);
    if normalized == "trusted-core"
        && permission_mode != "trusted"
        && permission_mode != "dangerous"
    {
        "guarded-core"
    } else {
        normalized
    }
}

pub fn exposed_tool_names(tool_profile: &str) -> Vec<&'static str> {
    match normalize_tool_profile(tool_profile) {
        "read-only" => CORE_READ_ONLY_TOOLS.to_vec(),
        "guarded-core" => GUARDED_CORE_TOOLS.to_vec(),
        "advanced" | "compat-readonly-all" => P0_TOOLS.iter().map(|(name, ..)| *name).collect(),
        _ => CORE_TOOLS.to_vec(),
    }
}

pub fn list_tools() -> Vec<Value> {
    list_tools_for_profile("full")
}

pub fn list_tools_for_profile(tool_profile: &str) -> Vec<Value> {
    let compat = tool_profile == "compat-readonly-all";
    exposed_tool_names(tool_profile)
        .into_iter()
        .filter_map(|name| {
            P0_TOOLS.iter().find(|(n, ..)| *n == name).map(|entry| {
                let (name, title, description, declared_read_only, destructive, open_world) =
                    *entry;
                let contract_read_only =
                    coding_tools_tunnel_protocol::is_retry_safe_tool_name(name);
                debug_assert_eq!(
                    declared_read_only, contract_read_only,
                    "tool read-only contract drifted for {name}"
                );
                let (read_only, destructive, open_world) = if compat {
                    (true, false, false)
                } else {
                    (contract_read_only, destructive, open_world)
                };
                json!({
                    "name": name,
                    "title": title,
                    "description": description,
                    "inputSchema": input_schema(name),
                    "annotations": {
                        "title": title,
                        "readOnlyHint": read_only,
                        "destructiveHint": destructive,
                        "idempotentHint": read_only,
                        "openWorldHint": open_world
                    }
                })
            })
        })
        .collect()
}

pub fn toolset_revision(tool_profile: &str) -> String {
    let encoded = serde_json::to_vec(&list_tools_for_profile(tool_profile)).unwrap_or_default();
    let digest = format!("{:x}", Sha256::digest(encoded));
    digest[..16].to_string()
}

pub fn input_schema(name: &str) -> Value {
    let mut schema = super::registry_schemas::input_schema(name).unwrap_or_else(|| {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    });
    if super::tool_runtime::descriptor(name).workspace_selector {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert(
                "workspace_folder_id".into(),
                json!({
                    "type": "string",
                    "minLength": 1,
                    "x-mcp-header": "Workspace",
                    "description": "Optional one-call workspace selector. Routes only this tool call and does not change the conversation's selected folder."
                }),
            );
        }
    }
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert(
            "retry_of_call_sequence".into(),
            json!({
                "type": "integer",
                "minimum": 1,
                "description": "Optional telemetry correlation to the failed tool call_sequence being retried. Removed before tool execution and does not change tool semantics or dedupe identity."
            }),
        );
        properties.insert(
            "recovery_of_operation_id".into(),
            json!({
                "type": "string",
                "minLength": 1,
                "maxLength": 128,
                "description": "Optional recovery-chain correlation to a prior operation_id. The runtime hashes this identifier in telemetry and removes it before tool execution."
            }),
        );
        properties.insert(
            "recovery_action_id".into(),
            json!({
                "type": "string",
                "minLength": 1,
                "maxLength": 128,
                "description": "Optional stable recovery action identifier selected from a previous error response. Removed before tool execution."
            }),
        );
    }
    schema
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::{input_schema, list_tools_for_profile, P0_TOOLS};
    use crate::tools::execution_timeout::ABSOLUTE_JOB_TIMEOUT_MAX_MS;
    use crate::tools::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS;

    #[test]
    fn read_only_annotations_match_the_tunnel_retry_contract() {
        for (name, _, _, declared_read_only, _, _) in P0_TOOLS {
            assert_eq!(
                *declared_read_only,
                coding_tools_tunnel_protocol::is_retry_safe_tool_name(name),
                "read-only retry contract drifted for {name}"
            );
        }
        assert!(!coding_tools_tunnel_protocol::is_retry_safe_tool_name(
            "set_default_cwd"
        ));
        assert!(!coding_tools_tunnel_protocol::is_retry_safe_tool_name(
            "apply_patch"
        ));
    }

    #[test]
    fn workspace_selector_is_exposed_only_on_one_call_routable_tools() {
        assert_eq!(
            input_schema("read_file")["properties"]["workspace_folder_id"]["type"],
            "string"
        );
        assert_eq!(
            input_schema("read_file")["properties"]["workspace_folder_id"]["x-mcp-header"],
            "Workspace"
        );
        assert_eq!(
            input_schema("git_status")["properties"]["workspace_folder_id"]["type"],
            "string"
        );
        assert_eq!(
            input_schema("exec_command")["properties"]["workspace_folder_id"]["type"],
            "string"
        );
        assert!(input_schema("history_session_checkpoint")["properties"]
            .get("workspace_folder_id")
            .is_none());
        assert!(input_schema("conversation_bootstrap")["properties"]
            .get("workspace_folder_id")
            .is_none());
    }

    #[test]
    fn recovery_correlation_fields_are_available_without_changing_tool_contracts() {
        let schema = input_schema("edit");
        assert_eq!(
            schema["properties"]["retry_of_call_sequence"]["type"],
            "integer"
        );
        assert_eq!(
            schema["properties"]["recovery_of_operation_id"]["type"],
            "string"
        );
        assert_eq!(schema["properties"]["recovery_action_id"]["type"], "string");
    }

    #[test]
    fn trusted_core_catalog_exposes_non_overlapping_tools() {
        let tools = list_tools_for_profile("core");
        let names: Vec<_> = tools
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect();
        let unique: HashSet<_> = names.iter().copied().collect();

        assert_eq!(tools.len(), 37);
        assert_eq!(unique.len(), tools.len());
        assert!(names.contains(&"history_session_bootstrap"));
        assert!(names.contains(&"history_session_checkpoint"));
        assert!(names.contains(&"list_workspace_folders"));
        assert!(names.contains(&"conversation_bootstrap"));
        assert!(names.contains(&"switch_workspace_folder"));
        assert!(names.contains(&"read_many"));
        assert!(names.contains(&"edit"));
        assert!(!names.contains(&"edit_file"));
        assert!(!names.contains(&"edit_many"));
        assert!(names.contains(&"format_files"));
        assert!(names.contains(&"wait_command"));
        assert!(names.contains(&"resolve_operation"));
        assert!(names.contains(&"list_sessions"));
        assert!(names.contains(&"git_push"));
        assert!(names.contains(&"send_input"));
        for removed in [
            "history_session_validate",
            "check_exec_environment",
            "get_default_cwd",
            "list_dir",
            "grep_text",
            "request_permissions",
        ] {
            assert!(!names.contains(&removed), "{removed} should not be exposed");
        }

        for name in names {
            let schema = input_schema(name);
            assert_eq!(schema["type"], "object", "{name} schema type");
            assert!(schema["properties"].is_object(), "{name} properties");
            assert!(schema.get("oneOf").is_none(), "{name} oneOf");
            assert!(schema.get("anyOf").is_none(), "{name} anyOf");
            assert!(schema.get("$ref").is_none(), "{name} ref");
        }
    }

    #[test]
    fn guarded_core_adds_only_permission_requests() {
        let trusted = list_tools_for_profile("trusted-core");
        let guarded = list_tools_for_profile("guarded-core");
        assert_eq!(trusted.len(), 37);
        assert_eq!(guarded.len(), 38);
        assert!(guarded
            .iter()
            .any(|tool| tool["name"] == "request_permissions"));
    }

    #[test]
    fn public_schemas_allow_values_that_the_server_can_normalize() {
        let search_schema = input_schema("search_text");
        assert_eq!(
            search_schema["properties"]["context_lines"]["maximum"],
            1000
        );
        assert_eq!(
            search_schema["properties"]["max_preview_bytes"]["maximum"],
            65_536
        );

        let operation_log_schema = input_schema("operation_log");
        assert_eq!(
            operation_log_schema["properties"]["order"]["default"],
            "desc"
        );
        assert_eq!(
            operation_log_schema["properties"]["errors_only"]["default"],
            false
        );

        let push_schema = input_schema("git_push");
        assert_eq!(push_schema["properties"]["remote"]["default"], "origin");
        assert_eq!(push_schema["properties"]["dry_run"]["default"], false);

        let diff_schema = input_schema("git_diff");
        assert_eq!(diff_schema["properties"]["context_lines"]["maximum"], 1000);

        let list_schema = input_schema("list_files");
        assert_eq!(list_schema["properties"]["max_results"]["default"], 1_000);
        assert_eq!(
            list_schema["properties"]["include_generated"]["default"],
            false
        );
        let search_schema = input_schema("search_text");
        assert_eq!(search_schema["properties"]["max_results"]["default"], 200);
        assert_eq!(
            search_schema["properties"]["include_generated"]["default"],
            false
        );

        let wait_schema = input_schema("wait_command");
        assert_eq!(
            wait_schema["properties"]["timeout_ms"]["maximum"],
            60 * 60_000
        );
        assert_eq!(wait_schema["properties"]["heartbeat_ms"]["maximum"], 30_000);
        assert!(wait_schema["properties"]["heartbeat_ms"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("ignored"));

        let exec_schema = input_schema("exec_command");
        assert!(exec_schema["properties"]["operation_id"].is_object());
        assert!(exec_schema["properties"]["deduplicate"].is_object());
        assert!(exec_schema["properties"]["lock_group"].is_object());
        assert_eq!(
            exec_schema["properties"]["timeout_ms"]["maximum"],
            ABSOLUTE_COMMAND_TIMEOUT_MAX_MS
        );
        assert_eq!(
            exec_schema["properties"]["job_timeout_ms"]["maximum"],
            ABSOLUTE_JOB_TIMEOUT_MAX_MS
        );
        assert!(
            exec_schema["properties"]["post_checks"]["items"]["properties"]
                .get("job_timeout_ms")
                .is_none()
        );
        assert_eq!(
            exec_schema["properties"]["post_checks"]["items"]["properties"]["timeout_ms"]
                ["maximum"],
            ABSOLUTE_COMMAND_TIMEOUT_MAX_MS
        );

        let exec_many_schema = input_schema("exec_many");
        assert!(exec_many_schema["properties"]["operation_id"].is_object());
        assert_eq!(
            exec_many_schema["properties"]["commands"]["items"]["properties"]["timeout_ms"]
                ["maximum"],
            ABSOLUTE_COMMAND_TIMEOUT_MAX_MS
        );
        assert_eq!(
            exec_many_schema["properties"]["commands"]["items"]["properties"]["job_timeout_ms"]
                ["maximum"],
            ABSOLUTE_JOB_TIMEOUT_MAX_MS
        );
        assert_eq!(
            exec_many_schema["properties"]["commands"]["items"]["properties"]["run_if"]["enum"],
            json!(["success", "failure", "always"])
        );
        assert_eq!(
            exec_many_schema["properties"]["commands"]["items"]["properties"]["run_if"]["default"],
            "success"
        );
        assert_eq!(exec_many_schema["properties"]["action"]["default"], "run");
        assert_eq!(
            exec_many_schema["properties"]["action"]["enum"],
            json!(["run", "status", "cancel", "forget"])
        );
        assert_eq!(
            exec_many_schema["properties"]["result_mode"]["enum"],
            json!(["full", "summary", "none"])
        );
        assert_eq!(
            exec_many_schema["properties"]["yield_time_ms"]["default"],
            20_000
        );
        assert_eq!(
            exec_many_schema["properties"]["yield_time_ms"]["maximum"],
            300_000
        );
        assert_eq!(
            exec_many_schema["properties"]["commands"]["items"]["properties"]["yield_time_ms"]
                ["maximum"],
            30_000
        );

        let resolve_schema = input_schema("resolve_operation");
        assert!(resolve_schema["properties"]["operation_id"].is_object());
        assert!(resolve_schema["properties"]["command_fingerprint"].is_object());

        let format_schema = input_schema("format_files");
        assert_eq!(format_schema["properties"]["mode"]["default"], "plan");
        assert_eq!(format_schema["properties"]["scope"]["default"], "files");
        assert_eq!(format_schema["properties"]["max_files"]["maximum"], 10_000);
        assert_eq!(
            format_schema["properties"]["timeout_ms"]["maximum"],
            600_000
        );
        assert!(format_schema["properties"]["expected_sha256"]["additionalProperties"].is_object());
    }

    #[test]
    fn precise_edit_schema_is_discriminated_by_operation_type() {
        let edit = input_schema("edit");
        let variants = edit["properties"]["files"]["items"]["properties"]["edits"]["items"]
            ["oneOf"]
            .as_array()
            .expect("edit precise edit variants");

        assert_eq!(variants.len(), 5);
        assert_eq!(
            variants[0]["required"],
            json!(["type", "old_text", "new_text"])
        );
        assert_eq!(
            variants[3]["required"],
            json!(["type", "start_line", "end_line", "new_text"])
        );
        assert!(variants[4]["properties"].get("new_text").is_none());
        assert_eq!(variants[4]["additionalProperties"], false);
    }
}
