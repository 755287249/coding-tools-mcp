use serde_json::{json, Value};

use crate::tools::execution_timeout::ABSOLUTE_JOB_TIMEOUT_MAX_MS;
use crate::tools::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS;

pub(super) fn input_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "exec_many" => json!({
            "type": "object",
            "properties": {
                "commands": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 256,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Stable command identifier used by DAG dependencies" },
                            "depends_on": { "type": "array", "maxItems": 256, "items": { "type": "string", "minLength": 1, "maxLength": 128 }, "default": [], "description": "Command IDs that must reach a terminal graph result before this command is evaluated in dag mode" },
                            "run_if": { "type": "string", "enum": ["success", "failure", "always"], "default": "success", "description": "Generic dependency outcome condition for dag mode. success requires every dependency to succeed; failure runs when at least one dependency did not succeed; always runs after all dependencies become terminal regardless of outcome." },
                            "lock_group": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Shared named resource lock such as cargo-target, node-generated, or git-index" },
                            "resource_class": { "type": "string", "enum": ["io_heavy"], "description": "Opt-in host-wide heavy I/O admission. io_heavy commands are serialized across exec_many graphs to preserve control-plane responsiveness under disk saturation." },
                            "operation_id": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Stable idempotency key. Retries with the same command reattach to the retained session." },
                            "deduplicate": { "type": "boolean", "description": "Coalesce identical in-flight retries. Completed automatic results are never reused; use operation_id to reattach a retained completed command. Defaults to true for safe Cargo check, test, build, and format commands." },
                            "cmd": { "type": "string", "minLength": 1 },
                            "script": { "type": "string", "minLength": 1, "description": "Structured shell script body; requires shell other than none" },
                            "program": { "type": "string", "minLength": 1 },
                            "args": { "type": "array", "maxItems": 1000, "items": { "type": "string" }, "default": [] },
                            "shell": { "type": "string", "enum": ["none", "cmd", "powershell", "sh"], "default": "none" },
                            "env": { "type": "object", "maxProperties": 64, "additionalProperties": { "type": "string", "maxLength": 4096 } },
                            "secret_env": { "type": "object", "maxProperties": 64, "additionalProperties": { "type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9._-]+$" }, "description": "Map child environment names to workspace-local secret references. Secret values never cross the MCP request." },
                            "remove_env": { "type": "array", "maxItems": 64, "items": { "type": "string" } },
                            "workdir": { "type": "string", "default": "." },
                            "timeout_ms": { "type": "integer", "minimum": 1, "maximum": ABSOLUTE_COMMAND_TIMEOUT_MAX_MS, "default": 30000, "description": "Fixed ordinary child-process lifetime; mutually exclusive with job_timeout_ms." },
                            "job_timeout_ms": { "type": "integer", "minimum": 1, "maximum": ABSOLUTE_JOB_TIMEOUT_MAX_MS, "description": "Opt-in fixed long-running child-process budget. Requires the command operation_id and never renews through graph polling." },
                            "max_output_bytes": { "type": "integer", "minimum": 1024, "maximum": 1048576, "default": 65536 },
                            "yield_time_ms": { "type": "integer", "minimum": 0, "maximum": 30000, "default": 30000 },
                            "output_mode": { "type": "string", "enum": ["delta", "tail", "all", "none", "summary"], "default": "tail", "description": "Per-command process output projection. Matches exec_command output_mode values." },
                            "tty": { "type": "boolean", "default": false },
                            "stdin": { "type": "string", "default": "" },
                            "stdin_secret": { "type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9._-]+$", "description": "Workspace-local secret reference whose value is written to child stdin. Mutually exclusive with stdin." },
                            "confirm": { "type": "boolean", "default": false },
                            "filesystem_scope": { "type": "string", "enum": ["workspace"], "default": "workspace" },
                            "reason": { "type": "string", "default": "" }
                        },
                        "additionalProperties": false
                    }
                },
                "operation_id": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Stable retained graph identifier. Reuse it without commands to reattach to the same exec_many graph instead of starting duplicate commands." },
                "action": { "type": "string", "enum": ["run", "status", "cancel", "forget"], "default": "run", "description": "Run or reattach by default; status returns immediately, cancel terminates active graph children, and forget releases a completed retained graph immediately." },
                "reason": { "type": "string", "default": "", "description": "Optional reason recorded when cancelling a retained graph." },
                "result_mode": { "type": "string", "enum": ["full", "summary", "none"], "description": "Controls per-command result detail. When omitted, small completed results stay full while aggregate child stdout/stderr above 16 KiB is automatically compacted to summary; retained running/status/cancel responses are compact where supported. Use full to opt into complete retained child output." },
                "yield_time_ms": { "type": "integer", "minimum": 0, "maximum": 300000, "default": 20000, "description": "Requested wait for retained graph completion. Requests up to 300000 ms remain accepted for compatibility, but each exec_many MCP response is transport-safely capped at 20000 ms; the graph keeps running and can be reattached with operation_id." },
                "mode": { "type": "string", "enum": ["auto", "sequential", "parallel", "dag"], "default": "auto", "description": "Execution scheduler. Auto uses dependencies, hard safety rules, resource locks, and historical pair statistics. Unknown command pairs remain sequential until explicit parallel observations provide enough safe evidence; explicit parallel is never silently overridden." },
                "max_parallel": { "type": "integer", "minimum": 1, "maximum": 256, "description": "Maximum concurrently running batch commands. Auto mode bounds the requested value by 8, workspace process admission, and historical resource-serialization recommendations." },
                "stop_on_error": { "type": "boolean", "default": true }
            },
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}
