mod desktop;
mod file;
mod git;
mod history;
mod image;
mod mutation;
mod permission;
mod process;
mod task;
mod workspace;

use serde_json::Value;

type SchemaRouter = fn(&str) -> Option<Value>;

const SCHEMA_ROUTERS: &[(&str, SchemaRouter)] = &[
    ("desktop", desktop::input_schema),
    ("file", file::input_schema),
    ("mutation", mutation::input_schema),
    ("permission", permission::input_schema),
    ("image", image::input_schema),
    ("workspace", workspace::input_schema),
    ("history", history::input_schema),
    ("task", task::input_schema),
    ("process", process::input_schema),
    ("git", git::input_schema),
];

pub(super) fn input_schema(name: &str) -> Option<Value> {
    SCHEMA_ROUTERS.iter().find_map(|(_, router)| router(name))
}

#[cfg(test)]
mod tests {
    use super::{input_schema, SCHEMA_ROUTERS};
    use crate::tools::registry_metadata::P0_TOOLS;

    #[test]
    fn routes_extracted_domains_without_claiming_remaining_schemas() {
        for (domain, names) in [
            (
                "file",
                &[
                    "read_file",
                    "read_many",
                    "project_map",
                    "list_files",
                    "search_text",
                ][..],
            ),
            (
                "mutation",
                &[
                    "apply_patch",
                    "edit",
                    "format_files",
                    "file_ops",
                    "patch_check",
                ][..],
            ),
            (
                "workspace",
                &[
                    "list_workspace_folders",
                    "conversation_bootstrap",
                    "switch_workspace_folder",
                    "query_tool_usage",
                    "set_default_cwd",
                    "set_todos",
                    "update_plan",
                    "report_progress",
                ][..],
            ),
            ("permission", &["request_permissions"][..]),
            ("image", &["view_image"][..]),
            (
                "desktop",
                &[
                    "desktop_displays",
                    "desktop_screenshot",
                    "desktop_click",
                    "desktop_drag",
                    "desktop_scroll",
                    "desktop_type",
                    "desktop_key",
                ][..],
            ),
            (
                "history",
                &[
                    "history_session_bootstrap",
                    "history_session_checkpoint",
                    "history_session_validate",
                ][..],
            ),
            (
                "task",
                &[
                    "harness_status",
                    "operation_log",
                    "project_state",
                    "start_task",
                    "update_task",
                    "pause_task",
                    "resume_task",
                    "fail_task",
                    "close_failed_task",
                    "rollback_task",
                    "finish_task",
                    "task_context",
                    "list_task_events",
                    "change_summary",
                ][..],
            ),
            (
                "process",
                &[
                    "exec_health_check",
                    "exec_many",
                    "exec_command",
                    "wait_command",
                    "resolve_operation",
                    "list_sessions",
                    "send_input",
                    "kill_session",
                    "read_output",
                ][..],
            ),
            (
                "git",
                &[
                    "git_status",
                    "git_diff",
                    "git_log",
                    "git_show",
                    "git_blame",
                    "git_branch",
                    "git_worktree",
                    "git_stage",
                    "git_commit",
                    "git_push",
                    "git_restore",
                ][..],
            ),
        ] {
            for name in names {
                assert!(
                    input_schema(name).is_some(),
                    "missing {domain} schema for {name}"
                );
            }
        }
        assert!(input_schema("server_info").is_none());
    }

    #[test]
    fn every_catalog_tool_has_one_schema_owner_or_the_documented_fallback() {
        for (name, _, _, _, _, _) in P0_TOOLS {
            let owners: Vec<_> = SCHEMA_ROUTERS
                .iter()
                .filter_map(|(domain, router)| router(name).is_some().then_some(*domain))
                .collect();

            if *name == "server_info" {
                assert!(
                    owners.is_empty(),
                    "fallback tool {name} claimed by {owners:?}"
                );
            } else {
                assert_eq!(owners.len(), 1, "schema owners for {name}: {owners:?}");
            }
        }
    }
}
