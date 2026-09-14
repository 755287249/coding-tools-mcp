use serde_json::{json, Value};

pub(super) fn input_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "harness_status" => json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        "operation_log" => json!({
            "type": "object",
            "properties": {
                "cursor": { "type": "integer", "minimum": 0, "default": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 },
                "order": { "type": "string", "enum": ["desc", "asc"], "default": "desc" },
                "tool": { "type": "string", "minLength": 1, "maxLength": 128 },
                "kind": { "type": "string", "minLength": 1, "maxLength": 64 },
                "errors_only": { "type": "boolean", "default": false },
                "since_ts_ms": { "type": "integer", "minimum": 0 }
            },
            "additionalProperties": false
        }),
        "project_state" => json!({
            "type": "object",
            "properties": {
                "max_files": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 200 }
            },
            "additionalProperties": false
        }),
        "start_task" => json!({
            "type": "object",
            "properties": {
                "objective": { "type": "string", "minLength": 1 },
                "existing_task": { "type": "string", "enum": ["error", "finish_if_complete"], "default": "error" }
            },
            "required": ["objective"],
            "additionalProperties": false
        }),
        "update_task" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "minLength": 1 },
                "completed_steps": { "type": "array", "items": { "type": "string" } },
                "pending_steps": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }),
        "pause_task" | "resume_task" | "fail_task" | "close_failed_task" | "rollback_task" => {
            json!({
                "type": "object",
                "properties": { "task_id": { "type": "string", "minLength": 1 } },
                "required": ["task_id"],
                "additionalProperties": false
            })
        }
        "finish_task" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "minLength": 1 },
                "summary": { "type": "string" },
                "allow_unverified": { "type": "boolean", "default": false }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }),
        "task_context" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "max_bytes": { "type": "integer", "minimum": 8192, "maximum": 131072, "default": 32768 }
            },
            "additionalProperties": false
        }),
        "list_task_events" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "minLength": 1 },
                "cursor": { "type": "integer", "minimum": 0, "default": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }),
        "change_summary" => json!({
            "type": "object",
            "properties": { "task_id": { "type": "string" }, "change_id": { "type": "string" } },
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}
