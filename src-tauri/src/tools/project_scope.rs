//! Capability-scoped projects expose only operations whose implementation respects
//! a workspace root. Native Git/formatter and desktop/extension processes must use
//! exec_command, which is forced through an OS sandbox for these contexts.
pub(crate) fn allowed(name: &str) -> bool {
    matches!(
        super::registry::canonical_tool_name(name),
        "list_workspace_folders"
            | "switch_workspace_folder"
            | "conversation_bootstrap"
            | "read_file"
            | "read_many"
            | "list_files"
            | "search_text"
            | "project_map"
            | "view_image"
            | "apply_patch"
            | "patch_check"
            | "edit"
            | "file_ops"
            | "exec_command"
            | "exec_many"
            | "wait_command"
            | "send_input"
            | "kill_session"
            | "read_output"
            | "list_sessions"
            | "resolve_operation"
            | "request_permissions"
            | "set_default_cwd"
            | "server_info"
            | "chat_open"
            | "chat_wait"
            | "chat_reply"
            | "chat_upload"
            | "chat_close"
            | "chat_discuss"
            | "set_todos"
            | "update_plan"
            | "report_progress"
            | "history_session_list"
            | "history_session_read"
            | "history_session_checkpoint"
            | "history_session_bootstrap"
            | "history_session_validate"
    )
}
pub(crate) fn denied(ctx: &super::ToolContext, name: &str) -> Option<serde_json::Value> {
    if ctx.sessions.is_closed() {
        return Some(super::workspace::tool_err_code(
            "PROJECT_STOPPED",
            "Project runtime has stopped.",
            "permission",
        ));
    }
    let runtime = ctx.runtime_config();
    // Full access exposes every tool, including Git and formatters.
    (runtime.project_isolation
        && !runtime.permission_mode.eq_ignore_ascii_case("dangerous")
        && !allowed(name))
    .then(||super::workspace::tool_err_code(
        "PROJECT_ISOLATION_REQUIRED",
        "This operation has no project sandbox. Use exec_command inside the project's OS sandbox for Git, formatters and other programs.","permission"))
}
