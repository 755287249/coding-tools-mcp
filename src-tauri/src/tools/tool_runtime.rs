use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum MutationLockGroup {
    History,
    WorkspaceContent,
    Git,
    Task,
    Cwd,
}

impl MutationLockGroup {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::WorkspaceContent => "workspace_content",
            Self::Git => "git",
            Self::Task => "task",
            Self::Cwd => "cwd",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolExecutionLane {
    Fast,
    Blocking,
    Process,
    Control,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolMutationPolicy {
    Never,
    Always,
    FormatApply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolBaselinePolicy {
    Never,
    Always,
    UnlessDryRun,
    FormatApply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolUsageFamily {
    Filesystem,
    Search,
    Quality,
    Process,
    Git,
    History,
    Runtime,
    Other,
}

impl ToolUsageFamily {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Search => "search",
            Self::Quality => "quality",
            Self::Process => "process",
            Self::Git => "git",
            Self::History => "history",
            Self::Runtime => "runtime",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ToolRuntimeDescriptor {
    pub lane: ToolExecutionLane,
    pub lock_groups: &'static [MutationLockGroup],
    pub mutation: ToolMutationPolicy,
    pub baseline: ToolBaselinePolicy,
    pub standalone_operation: bool,
    pub log_operation: bool,
    pub usage_family: ToolUsageFamily,
    pub workspace_selector: bool,
}

const NO_LOCKS: &[MutationLockGroup] = &[];
const HISTORY_LOCK: &[MutationLockGroup] = &[MutationLockGroup::History];
const WORKSPACE_CONTENT_LOCK: &[MutationLockGroup] = &[MutationLockGroup::WorkspaceContent];
const GIT_LOCK: &[MutationLockGroup] = &[MutationLockGroup::Git];
const GIT_AND_WORKSPACE_LOCK: &[MutationLockGroup] =
    &[MutationLockGroup::WorkspaceContent, MutationLockGroup::Git];
const TASK_LOCK: &[MutationLockGroup] = &[MutationLockGroup::Task];
const CWD_LOCK: &[MutationLockGroup] = &[MutationLockGroup::Cwd];

const DEFAULT: ToolRuntimeDescriptor = ToolRuntimeDescriptor {
    lane: ToolExecutionLane::Blocking,
    lock_groups: NO_LOCKS,
    mutation: ToolMutationPolicy::Never,
    baseline: ToolBaselinePolicy::Never,
    standalone_operation: false,
    log_operation: false,
    usage_family: ToolUsageFamily::Other,
    workspace_selector: false,
};

pub(crate) fn descriptor(name: &str) -> ToolRuntimeDescriptor {
    let canonical = match name {
        "edit_file" | "edit_many" => "edit",
        _ => name,
    };
    let mut runtime = DEFAULT;

    runtime.usage_family = if canonical.starts_with("git_") {
        ToolUsageFamily::Git
    } else if canonical.starts_with("history_session_") {
        ToolUsageFamily::History
    } else {
        match canonical {
            "read_file" | "read_many" | "list_files" | "apply_patch" | "edit" | "file_ops"
            | "view_image" => ToolUsageFamily::Filesystem,
            "search_text" | "project_map" => ToolUsageFamily::Search,
            "format_files" => ToolUsageFamily::Quality,
            "exec_command" | "exec_many" | "wait_command" | "resolve_operation"
            | "list_sessions" | "send_input" | "kill_session" | "read_output" => {
                ToolUsageFamily::Process
            }
            "server_info" | "query_tool_usage" | "set_default_cwd" | "request_permissions" => {
                ToolUsageFamily::Runtime
            }
            "desktop_displays" | "desktop_screenshot" | "desktop_click" | "desktop_drag"
            | "desktop_scroll" | "desktop_type" | "desktop_key" => ToolUsageFamily::Runtime,
            _ => ToolUsageFamily::Other,
        }
    };

    runtime.lane = match canonical {
        "server_info" => ToolExecutionLane::Fast,
        "wait_command" | "resolve_operation" | "list_sessions" | "send_input" | "kill_session"
        | "read_output" => ToolExecutionLane::Control,
        "exec_command" | "exec_many" | "exec_health_check" | "request_permissions" => {
            ToolExecutionLane::Process
        }
        _ => ToolExecutionLane::Blocking,
    };

    runtime.lock_groups = match canonical {
        "conversation_bootstrap"
        | "history_session_bootstrap"
        | "history_session_checkpoint"
        | "history_session_validate" => HISTORY_LOCK,
        "apply_patch" | "edit" | "file_ops" | "format_files" => WORKSPACE_CONTENT_LOCK,
        "git_restore" | "git_worktree" => GIT_AND_WORKSPACE_LOCK,
        "git_branch" | "git_stage" | "git_commit" | "git_push" => GIT_LOCK,
        "start_task" | "update_task" | "pause_task" | "resume_task" | "fail_task"
        | "close_failed_task" | "rollback_task" | "finish_task" => TASK_LOCK,
        "set_default_cwd" => CWD_LOCK,
        _ => NO_LOCKS,
    };

    runtime.mutation = match canonical {
        "format_files" => ToolMutationPolicy::FormatApply,
        "conversation_bootstrap"
        | "switch_workspace_folder"
        | "history_session_bootstrap"
        | "history_session_checkpoint"
        | "history_session_validate"
        | "apply_patch"
        | "edit"
        | "file_ops"
        | "git_branch"
        | "git_worktree"
        | "git_stage"
        | "git_commit"
        | "git_push"
        | "git_restore"
        | "exec_command"
        | "exec_many"
        | "send_input"
        | "kill_session"
        | "request_permissions"
        | "set_default_cwd"
        | "start_task"
        | "update_task"
        | "pause_task"
        | "resume_task"
        | "fail_task"
        | "close_failed_task"
        | "rollback_task"
        | "finish_task" => ToolMutationPolicy::Always,
        "desktop_click" | "desktop_drag" | "desktop_scroll" | "desktop_type" | "desktop_key" => {
            ToolMutationPolicy::Always
        }
        _ => ToolMutationPolicy::Never,
    };

    runtime.baseline = match canonical {
        "exec_command" => ToolBaselinePolicy::Always,
        "apply_patch" | "edit" | "file_ops" | "git_branch" | "git_worktree" | "git_stage"
        | "git_commit" | "git_restore" => ToolBaselinePolicy::UnlessDryRun,
        "format_files" => ToolBaselinePolicy::FormatApply,
        _ => ToolBaselinePolicy::Never,
    };

    runtime.workspace_selector = canonical.starts_with("git_")
        || matches!(
            canonical,
            "set_default_cwd"
                | "read_file"
                | "read_many"
                | "list_files"
                | "project_map"
                | "search_text"
                | "apply_patch"
                | "edit"
                | "file_ops"
                | "patch_check"
                | "format_files"
                | "view_image"
                | "exec_health_check"
                | "exec_command"
                | "exec_many"
                | "wait_command"
                | "resolve_operation"
                | "list_sessions"
                | "send_input"
                | "kill_session"
                | "read_output"
                | "request_permissions"
        );

    runtime.standalone_operation = matches!(
        canonical,
        "patch_check"
            | "apply_patch"
            | "edit"
            | "file_ops"
            | "format_files"
            | "exec_command"
            | "git_branch"
            | "git_worktree"
            | "git_stage"
            | "git_commit"
            | "git_push"
            | "git_restore"
    );
    runtime.log_operation = runtime.standalone_operation || canonical.starts_with("git_");
    runtime
}

pub(crate) fn request_mutates(name: &str, args: &Value) -> bool {
    match descriptor(name).mutation {
        ToolMutationPolicy::Never => false,
        ToolMutationPolicy::Always => true,
        ToolMutationPolicy::FormatApply => {
            args.get("mode").and_then(Value::as_str) == Some("apply")
        }
    }
}

pub(crate) fn is_mutating_tool(name: &str) -> bool {
    descriptor(name).mutation != ToolMutationPolicy::Never
}

pub(crate) fn requires_write_baseline(name: &str, args: &Value) -> bool {
    match descriptor(name).baseline {
        ToolBaselinePolicy::Never => false,
        ToolBaselinePolicy::Always => true,
        ToolBaselinePolicy::UnlessDryRun => !args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ToolBaselinePolicy::FormatApply => {
            args.get("mode").and_then(Value::as_str) == Some("apply")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_metadata_covers_critical_dispatch_contracts() {
        assert_eq!(descriptor("server_info").lane, ToolExecutionLane::Fast);
        assert_eq!(descriptor("exec_command").lane, ToolExecutionLane::Process);
        assert_eq!(descriptor("wait_command").lane, ToolExecutionLane::Control);
        assert_eq!(
            descriptor("conversation_bootstrap").lock_groups,
            HISTORY_LOCK
        );
        assert_eq!(
            descriptor("format_files").lock_groups,
            WORKSPACE_CONTENT_LOCK
        );
        assert_eq!(
            descriptor("git_restore").lock_groups,
            GIT_AND_WORKSPACE_LOCK
        );
        assert!(descriptor("git_status").log_operation);
        assert!(!descriptor("read_file").log_operation);
        assert!(descriptor("read_file").workspace_selector);
        assert!(descriptor("git_commit").workspace_selector);
        assert!(descriptor("exec_command").workspace_selector);
        assert!(!descriptor("history_session_checkpoint").workspace_selector);
    }

    #[test]
    fn aliases_and_argument_sensitive_policies_share_one_contract() {
        assert_eq!(descriptor("edit_file"), descriptor("edit"));
        assert_eq!(descriptor("edit_many"), descriptor("edit"));
        assert!(requires_write_baseline("edit", &serde_json::json!({})));
        assert!(!requires_write_baseline(
            "edit",
            &serde_json::json!({"dry_run": true})
        ));
        assert!(!request_mutates(
            "format_files",
            &serde_json::json!({"mode": "check"})
        ));
        assert!(request_mutates(
            "format_files",
            &serde_json::json!({"mode": "apply"})
        ));
    }
}
