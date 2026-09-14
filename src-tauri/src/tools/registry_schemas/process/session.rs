use serde_json::{json, Value};

use crate::tools::session::{WAIT_COMMAND_TIMEOUT_DEFAULT_MS, WAIT_COMMAND_TIMEOUT_MAX_MS};

pub(super) fn input_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "wait_command" => json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "minLength": 1 },
                "cursor": { "type": "integer", "minimum": 0, "default": 0 },
                "timeout_ms": { "type": "integer", "minimum": 0, "maximum": WAIT_COMMAND_TIMEOUT_MAX_MS, "default": WAIT_COMMAND_TIMEOUT_DEFAULT_MS, "description": "Requested server-side event wait, separate from the child-process timeout. Values up to 60 minutes are accepted for compatibility, but each MCP response waits at most 20 seconds and returns retained next_actions so proxy heartbeat behavior cannot cause a lost response." },
                "heartbeat_ms": { "type": "integer", "minimum": 0, "maximum": 30000, "default": 0, "description": "Deprecated compatibility field. Accepted but ignored for application wait timing; long waits are transport-safely chunked instead of relying on proxy heartbeat forwarding." },
                "until": { "type": "string", "enum": ["output_or_exit", "exit", "finalized"], "default": "output_or_exit" },
                "output_mode": { "type": "string", "enum": ["delta", "tail", "all", "none", "summary"], "default": "delta" },
                "event_detail": { "type": "string", "enum": ["compact", "full"], "default": "compact", "description": "For delta output, compact returns sequence/stream/stream_offset metadata while stdout/stderr carry the data once. full additionally includes per-event decoded offsets, encoding, and data for callers that need exact stdout/stderr interleaving." },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 65536 },
                "tail_lines": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 100 }
            },
            "required": ["session_id"],
            "additionalProperties": false
        }),
        "resolve_operation" => json!({
            "type": "object",
            "properties": {
                "operation_id": { "type": "string", "minLength": 1, "maxLength": 128 },
                "command_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "cursor": { "type": "integer", "minimum": 0, "default": 0 },
                "output_mode": { "type": "string", "enum": ["delta", "tail", "all", "none", "summary"], "default": "tail" },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 65536 },
                "tail_lines": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 100 }
            },
            "additionalProperties": false
        }),
        "list_sessions" => json!({
            "type": "object",
            "properties": {
                "include_finalized": { "type": "boolean", "default": true },
                "status": { "type": "string", "enum": ["running", "verifying", "exited", "timed_out", "killed"] },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 100 }
            },
            "additionalProperties": false
        }),
        "send_input" => json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "minLength": 1 },
                "chars": { "type": "string", "default": "" },
                "close_stdin": { "type": "boolean", "default": false }
            },
            "required": ["session_id"],
            "additionalProperties": false
        }),
        "kill_session" => json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "minLength": 1 },
                "signal": { "type": "string", "enum": ["TERM", "KILL", "INT"], "default": "TERM" },
                "wait_ms": { "type": "integer", "minimum": 0, "maximum": 30000, "default": 5000 },
                "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 65536 }
            },
            "required": ["session_id"],
            "additionalProperties": false
        }),
        "read_output" => json!({
            "type": "object",
            "properties": {
                "output_ref": { "type": "string", "minLength": 1 },
                "offset": { "type": "integer", "minimum": 0, "default": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 4096 }
            },
            "required": ["output_ref"],
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}
