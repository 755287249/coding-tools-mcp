use serde_json::{json, Value};

pub(super) fn input_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "list_workspace_folders" => json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        "conversation_bootstrap" => json!({
            "type": "object",
            "properties": {
                "folder_id": { "type": "string", "minLength": 1 },
                "session_key": { "type": "string", "minLength": 1 },
                "title": { "type": "string" },
                "history_dir": { "type": "string", "default": "docs/history-session" },
                "create_if_missing": { "type": "boolean", "default": true },
                "response_mode": { "type": "string", "enum": ["compact", "full"], "default": "compact" }
            },
            "additionalProperties": false
        }),
        "switch_workspace_folder" => json!({
            "type": "object",
            "required": ["folder_id"],
            "properties": {
                "folder_id": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        }),
        "query_tool_usage" => json!({
            "type": "object",
            "properties": {
                "tools": { "type": "array", "maxItems": 100, "items": { "type": "string" }, "default": [] },
                "exclude_tools": { "type": "array", "maxItems": 100, "items": { "type": "string" }, "default": ["query_tool_usage"] },
                "outcomes": { "type": "array", "maxItems": 20, "items": { "type": "string" }, "default": [] },
                "scope": { "type": "string", "enum": ["current_runtime", "current_version", "all"], "default": "current_runtime" },
                "sort_by": { "type": "string", "enum": ["calls", "errors", "duration_ms", "p95_ms", "response_bytes", "request_bytes", "queue_wait_ms"], "default": "calls" },
                "top": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
                "errors_only": { "type": "boolean", "default": false },
                "min_duration_ms": { "type": "integer", "minimum": 0, "default": 0 },
                "since_ts_ms": { "type": "integer", "minimum": 0, "default": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 },
                "cursor": { "type": "integer", "minimum": 0, "maximum": 10000, "default": 0 },
                "aggregate": { "type": "boolean", "default": true },
                "include_records": { "type": "boolean", "default": false },
                "include_payloads": { "type": "boolean", "default": false },
                "include_slowest": { "type": "boolean", "default": false },
                "include_largest": { "type": "boolean", "default": false },
                "include_performance": { "type": "boolean", "default": true },
                "include_bursts": { "type": "boolean", "default": false },
                "include_async_sessions": { "type": "boolean", "default": true },
                "burst_idle_ms": { "type": "integer", "minimum": 1000, "maximum": 3600000, "default": 120000 }
            },
            "additionalProperties": false
        }),
        "set_default_cwd" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": "." }
            },
            "additionalProperties": false
        }),
        "set_todos" => json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "maxLength": 400, "description": "What is being built, in one sentence. Kept from the previous plan when omitted." },
                "todos": {
                    "type": "array",
                    "maxItems": 24,
                    "description": "Full ordered checklist. Empty array clears the plan.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "minLength": 1, "maxLength": 80 },
                            "title": { "type": "string", "minLength": 1, "maxLength": 400 },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["id", "title", "status"],
                        "additionalProperties": false
                    }
                },
                "external_task_id": { "type": "string", "maxLength": 100 }
            },
            "required": ["todos"],
            "additionalProperties": false
        }),
        "update_plan" => json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "maxLength": 400 },
                "explanation": { "type": "string", "maxLength": 2000 },
                "plan": {
                    "type": "array",
                    "maxItems": 24,
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": { "type": "string", "minLength": 1, "maxLength": 400 },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["plan"],
            "additionalProperties": false
        }),
        "report_progress" => json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "minLength": 1, "maxLength": 2000 },
                "phase": { "type": "string", "maxLength": 160 },
                "percent": { "type": "integer", "minimum": 0, "maximum": 100, "description": "Your own estimate; the panel never auto-completes." },
                "todo_id": { "type": "string", "maxLength": 80 }
            },
            "required": ["message"],
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}
