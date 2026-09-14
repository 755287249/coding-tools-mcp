use serde_json::{json, Value};

pub(super) fn input_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "git_status" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": "." },
                "include_untracked": { "type": "boolean", "default": true },
                "max_entries": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 1000 }
            },
            "additionalProperties": false
        }),
        "git_diff" => json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" }, "default": [] },
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "staged": { "type": "boolean", "default": false },
                "unstaged": { "type": "boolean", "default": true },
                "context_lines": { "type": "integer", "minimum": 0, "maximum": 1000, "default": 3, "description": "Values above 20 are normalized to 20" },
                "max_bytes": { "type": "integer", "minimum": 1024, "maximum": 1048576, "default": 262144 }
            },
            "additionalProperties": false
        }),
        "git_log" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": "." },
                "ref": { "type": "string", "default": "HEAD" },
                "max_count": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
                "skip": { "type": "integer", "minimum": 0, "maximum": 10000, "default": 0 }
            },
            "additionalProperties": false
        }),
        "git_show" => json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "rev": { "type": "string", "default": "HEAD" },
                "path": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" } },
                "include_diff": { "type": "boolean", "default": true },
                "context_lines": { "type": "integer", "minimum": 0, "maximum": 1000, "default": 3, "description": "Values above 20 are normalized to 20" },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 262144 }
            },
            "additionalProperties": false
        }),
        "git_blame" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1 },
                "rev": { "type": "string" },
                "start_line": { "type": "integer", "minimum": 1, "default": 1 },
                "end_line": { "type": "integer", "minimum": 1 },
                "max_lines": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 200 }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "git_branch" => json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["create", "switch", "delete"] },
                "name": { "type": "string", "minLength": 1 },
                "start_point": { "type": "string" },
                "switch": { "type": "boolean", "default": true },
                "force": { "type": "boolean", "default": false },
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "expected_repo_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "expected_head": { "type": "string" },
                "dry_run": { "type": "boolean", "default": false },
                "confirm": { "type": "boolean", "default": false },
                "reason": { "type": "string", "default": "" }
            },
            "required": ["action", "name"],
            "additionalProperties": false
        }),
        "git_worktree" => json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "create", "remove"] },
                "path": { "type": "string", "minLength": 1 },
                "branch": { "type": "string", "minLength": 1 },
                "branch_mode": { "type": "string", "enum": ["auto", "create", "existing"], "default": "auto" },
                "start_point": { "type": "string", "default": "HEAD" },
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "expected_repo_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "expected_head": { "type": "string" },
                "dry_run": { "type": "boolean", "default": false },
                "force": { "type": "boolean", "default": false },
                "delete_branch": { "type": "boolean", "default": false },
                "confirm": { "type": "boolean", "default": false },
                "reason": { "type": "string", "default": "" }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
        "git_stage" => json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" } },
                "all": { "type": "boolean", "default": false },
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "expected_repo_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "expected_head": { "type": "string" },
                "dry_run": { "type": "boolean", "default": false },
                "reason": { "type": "string", "default": "" }
            },
            "additionalProperties": false
        }),
        "git_commit" => json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "minLength": 1, "maxLength": 10000 },
                "paths": { "type": "array", "items": { "type": "string" } },
                "all": { "type": "boolean", "default": false },
                "allow_empty": { "type": "boolean", "default": false },
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "expected_repo_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "require_clean_index_before": { "type": "boolean" },
                "expected_head": { "type": "string" },
                "dry_run": { "type": "boolean", "default": false },
                "reason": { "type": "string", "default": "" }
            },
            "required": ["message"],
            "additionalProperties": false
        }),
        "git_push" => json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "remote": { "type": "string", "minLength": 1, "default": "origin" },
                "branch": { "type": "string", "minLength": 1, "description": "Defaults to the current branch; required for detached HEAD." },
                "set_upstream": { "type": "boolean", "default": false },
                "expected_repo_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "expected_head": { "type": "string" },
                "dry_run": { "type": "boolean", "default": false },
                "reason": { "type": "string", "default": "" }
            },
            "additionalProperties": false
        }),
        "git_restore" => json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "minItems": 1, "items": { "type": "string" } },
                "staged": { "type": "boolean", "default": false },
                "worktree": { "type": "boolean" },
                "source": { "type": "string" },
                "repo_path": { "type": "string", "default": ".", "description": "Workspace-relative Git repository or linked worktree root" },
                "expected_repo_fingerprint": { "type": "string", "minLength": 64, "maxLength": 64 },
                "expected_head": { "type": "string" },
                "dry_run": { "type": "boolean", "default": false },
                "confirm": { "type": "boolean", "default": false },
                "reason": { "type": "string", "default": "" }
            },
            "required": ["paths"],
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}
