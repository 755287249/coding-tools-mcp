mod common;

use std::fs;
use std::process::Command;

use coding_tools_mcp_desktop_lib::tools::list_tools_for_profile;
use common::*;
use serde_json::{json, Value};

#[cfg(windows)]
const TEST_PYTHON: &str = "python";
#[cfg(not(windows))]
const TEST_PYTHON: &str = "python3";

#[test]
fn server_info_returns_workspace_and_tools() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(&ctx, "server_info", json!({}));
    let payload = assert_ok(&out);
    assert_eq!(payload["server"], "coding-tools-mcp");
    assert_eq!(payload["version"], env!("CARGO_PKG_VERSION"));
    assert!(payload["tools"].is_array());
    assert!(payload["tool_count"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn read_file_happy_path() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(&ctx, "read_file", json!({"path": "src/math.js"}));
    let payload = assert_ok(&out);
    assert_eq!(payload["path"], "src/math.js");
    assert_eq!(payload["encoding"], "utf-8");
    assert_eq!(payload["sha256"].as_str().unwrap().len(), 64);
}

#[test]
fn read_many_and_edit_file_work_through_dispatch() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let batch = invoke(
        &ctx,
        "read_many",
        json!({
            "items": [
                { "path": "src/math.js", "start_line": 1, "end_line": 3 },
                { "path": "package.json" }
            ]
        }),
    );
    let batch = assert_ok(&batch);
    assert_eq!(batch["failed_count"], 0);
    assert_eq!(batch["results"].as_array().unwrap().len(), 2);

    let hash = batch["results"][0]["sha256"]
        .as_str()
        .expect("sha256")
        .to_string();
    let edit = invoke(
        &ctx,
        "edit_file",
        json!({
            "path": "src/math.js",
            "expected_sha256": hash,
            "edits": [{
                "type": "replace",
                "old_text": "return a - b;",
                "new_text": "return a + b;",
                "expected_occurrences": 1
            }]
        }),
    );
    let edit = assert_ok(&edit);
    assert_eq!(edit["applied"], true);
    assert!(edit["diff"].as_str().unwrap().contains("+  return a + b;"));
    assert!(fs::read_to_string(fx.root.join("src/math.js"))
        .unwrap()
        .contains("return a + b;"));
}

#[test]
fn project_map_and_search_v2_return_structured_results() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let mapped = invoke(
        &ctx,
        "project_map",
        json!({"path": ".", "max_depth": 4, "max_entries": 100}),
    );
    let mapped = assert_ok(&mapped);
    assert!(mapped["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"] == "package.json"));
    assert!(mapped["languages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["language"] == "JavaScript"));

    let searched = invoke(
        &ctx,
        "search_text",
        json!({
            "queries": ["return", {"query": "function", "case_sensitive": true}],
            "filename_query": "math",
            "max_results": 20
        }),
    );
    let searched = assert_ok(&searched);
    assert_eq!(searched["queries"].as_array().unwrap().len(), 2);
    assert!(searched["matches"].as_array().unwrap().iter().all(|item| {
        item["path"].as_str().unwrap_or("").contains("math")
            && item["match_id"]
                .as_str()
                .unwrap_or("")
                .starts_with("match-")
    }));
}

#[test]
fn read_many_accepts_matches_merges_ranges_and_reads_utf16() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let batch = invoke(
        &ctx,
        "read_many",
        json!({
            "matches": [
                {"path": "src/math.js", "line": 2},
                {"path": "src/math.js", "line": 3}
            ],
            "context_lines": 2,
            "merge_overlaps": true,
            "line_numbers": true
        }),
    );
    let batch = assert_ok(&batch);
    assert_eq!(batch["result_count"], 1);
    assert_eq!(batch["merged_count"], 1);
    assert!(batch["results"][0]["numbered_content"]
        .as_str()
        .unwrap_or("")
        .contains("|"));

    let mut utf16 = vec![0xff, 0xfe];
    for unit in "hello\n".encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(fx.root.join("utf16.txt"), utf16).expect("write utf16");
    let read = invoke(&ctx, "read_file", json!({"path": "utf16.txt"}));
    let read = assert_ok(&read);
    assert_eq!(read["encoding"], "utf-16le");
    assert_eq!(read["bom"], true);
    assert_eq!(read["content"], "hello\n");
}

#[test]
fn edit_many_and_file_ops_are_transactional_through_dispatch() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let reads = invoke(
        &ctx,
        "read_many",
        json!({"items": [{"path": "src/math.js"}, {"path": "TODO.md"}]}),
    );
    let reads = assert_ok(&reads);
    let edited = invoke(
        &ctx,
        "edit_many",
        json!({
            "files": [
                {
                    "path": "src/math.js",
                    "expected_sha256": reads["results"][0]["sha256"],
                    "edits": [{"type": "replace", "old_text": "return a - b;", "new_text": "return a + b;"}]
                },
                {
                    "path": "TODO.md",
                    "expected_sha256": reads["results"][1]["sha256"],
                    "edits": [{"type": "insert_after", "anchor": "TODO", "text": "\nDONE"}]
                }
            ]
        }),
    );
    let edited = assert_ok(&edited);
    assert_eq!(edited["applied"], true);
    assert_eq!(edited["files_modified"].as_array().unwrap().len(), 2);

    let operated = invoke(
        &ctx,
        "file_ops",
        json!({
            "operations": [
                {"type": "create", "path": "generated/a.txt", "content": "alpha\n"},
                {"type": "mkdir", "path": "generated/empty"}
            ]
        }),
    );
    let operated = assert_ok(&operated);
    assert_eq!(operated["applied"], true);
    assert_eq!(
        fs::read_to_string(fx.root.join("generated/a.txt")).unwrap(),
        "alpha\n"
    );
    assert!(fx.root.join("generated/empty").is_dir());
}

#[test]
fn structured_exec_program_args_and_env_work() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(
        &ctx,
        "exec_command",
        json!({
            "program": TEST_PYTHON,
            "args": ["-c", "import os; print(os.environ.get('MCP_TEST_VALUE', 'missing'))"],
            "env": {"MCP_TEST_VALUE": "structured-ok"},
            "timeout_ms": 10000,
            "yield_time_ms": 10000
        }),
    );
    let out = assert_ok(&out);
    assert_eq!(out["command_ok"], true, "{out}");
    assert!(out["stdout"]
        .as_str()
        .unwrap_or("")
        .contains("structured-ok"));
    assert_eq!(out["shell"], "none");
    assert!(out["environment_keys"]
        .as_array()
        .unwrap()
        .contains(&json!("MCP_TEST_VALUE")));
}

#[test]
fn unknown_tool_is_validation_error() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(&ctx, "definitely_not_a_tool", json!({}));
    let err = assert_err(&out);
    assert_eq!(err["error"]["code"], "INVALID_ARGUMENT");
    assert_eq!(err["error"]["category"], "validation");
}

#[test]
fn read_file_rejects_parent_path_outside_workspace() {
    let fx = malicious_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(&ctx, "read_file", json!({"path": "../outside-secret.txt"}));
    let error = assert_err(&out);
    assert_eq!(error["error"]["code"], "PATH_OUTSIDE_WORKSPACE");
    assert!(!format!("{error}").contains("TOP_SECRET"));
}

#[test]
fn read_file_rejects_absolute_path_outside_workspace() {
    let fx = malicious_fixture();
    let ctx = ctx_for(&fx.root);
    let outside = fx.root.parent().unwrap().join("outside-secret.txt");
    let out = invoke(
        &ctx,
        "read_file",
        json!({"path": outside.to_string_lossy()}),
    );
    let error = assert_err(&out);
    assert_eq!(error["error"]["code"], "ABSOLUTE_PATH_DENIED");
    assert!(!format!("{error}").contains("TOP_SECRET"));
}

#[test]
fn request_permissions_requires_a_resumable_operation_in_safe_mode() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(
        &ctx,
        "request_permissions",
        json!({
            "tool_name": "exec_command",
            "permission": "network",
            "reason": "verify compliance denial shape",
            "arguments": {"cmd": "curl https://example.com"}
        }),
    );
    assert_err(&out);
    assert_eq!(out["error"]["code"], "RESUME_ID_REQUIRED");
    assert_eq!(out["status"], "unsupported");
}

#[test]
fn request_permissions_exposes_public_schema_and_grants_in_dangerous_mode() {
    let tools = list_tools_for_profile("guarded-core");
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "request_permissions")
        .expect("request_permissions descriptor");
    let schema = &tool["inputSchema"];
    assert!(schema["properties"]["resume_id"].is_object());
    assert!(schema["properties"]["approve"].is_object());
    assert!(schema["properties"]["confirm"].is_object());
    assert!(schema.get("required").is_none());
    assert_eq!(tool["annotations"]["readOnlyHint"], false);
    assert_eq!(tool["annotations"]["destructiveHint"], true);
    assert!(schema["properties"]["permission"]["enum"]
        .as_array()
        .expect("permission enum")
        .contains(&json!("network")));

    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let runtime = ctx.runtime_config();
    ctx.update_runtime_config(runtime.policy, runtime.tool_profile, "dangerous".into());
    let args = json!({
        "tool_name": "exec_command",
        "permission": "network",
        "reason": "verify dangerous-mode compatibility",
        "arguments": {"cmd": "curl https://example.com"}
    });
    let out = invoke(&ctx, "request_permissions", args.clone());
    let payload = assert_ok(&out);
    assert_eq!(payload["status"], "granted");
    assert_eq!(payload["constraints"]["mode"], "dangerous");
    assert_eq!(payload["constraints"]["requested"], args);
}

#[test]
fn server_info_reports_execution_environment_metadata() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(&ctx, "server_info", json!({}));
    let payload = assert_ok(&out);
    assert_eq!(payload["permission_mode"], "trusted");
    assert!(payload["environment"]["workspace_exec"]["system_command_allowlist"].is_array());
    assert_eq!(
        payload["environment"]["powershell"]["selection_policy"],
        "pwsh_then_windows_powershell"
    );
}

#[test]
fn default_cwd_is_used_by_file_and_native_exec_tools() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    assert_ok(&invoke(&ctx, "set_default_cwd", json!({"path": "src"})));

    let file_result = invoke(&ctx, "read_file", json!({"path": "math.js"}));
    let file = assert_ok(&file_result);
    assert_eq!(file["path"], "src/math.js");

    let pwd_result = invoke(&ctx, "exec_command", json!({"cmd": "pwd"}));
    let pwd = assert_ok(&pwd_result);
    assert!(pwd["stdout"].as_str().unwrap_or("").contains("src"));
}

#[test]
fn git_log_root_does_not_pass_empty_pathspec() {
    let temp = tempfile::tempdir().expect("创建临时目录");
    let workspace = temp.path().join("repo");
    fs::create_dir_all(&workspace).expect("创建仓库目录");
    fs::write(workspace.join("README.md"), "初始内容\n").expect("写入文件");

    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "测试用户"],
        vec!["add", "README.md"],
        vec!["commit", "-q", "-m", "初始化"],
    ] {
        let output = Command::new("git")
            .current_dir(&workspace)
            .args(args)
            .output()
            .expect("执行 git");
        assert!(output.status.success(), "git 命令失败: {:?}", output);
    }

    let ctx = ctx_for(&workspace);
    let result = invoke(&ctx, "git_log", json!({"path": ".", "max_count": 3}));
    let payload = assert_ok(&result);
    assert_eq!(payload["is_repo"], true);
    assert_eq!(payload["commits"].as_array().unwrap().len(), 1);
    for commit in payload["commits"].as_array().unwrap() {
        for field in [
            "hash",
            "short_hash",
            "author_name",
            "author_email",
            "author_date",
            "subject",
        ] {
            assert_eq!(
                commit[field].as_str().unwrap(),
                commit[field].as_str().unwrap().trim()
            );
        }
    }
}

#[test]
fn advanced_profile_exposes_every_declared_tool() {
    let declared = coding_tools_mcp_desktop_lib::tools::registry::P0_TOOLS
        .iter()
        .map(|(name, ..)| *name)
        .collect::<std::collections::HashSet<_>>();
    let tool_values = coding_tools_mcp_desktop_lib::tools::list_tools_for_profile("advanced");
    let exposed = tool_values
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(declared, exposed);
    assert!(declared
        .iter()
        .all(|name| coding_tools_mcp_desktop_lib::tools::is_allowed_tool(name)));
}

#[test]
fn core_profile_keeps_the_default_capabilities_and_adds_history_tools() {
    let tools = coding_tools_mcp_desktop_lib::tools::list_tools_for_profile("core");
    let names = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<std::collections::HashSet<_>>();
    let expected = coding_tools_mcp_desktop_lib::tools::registry::CORE_TOOLS
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names, expected);
    assert_eq!(
        names.len(),
        coding_tools_mcp_desktop_lib::tools::registry::CORE_TOOLS.len()
    );
    assert!(names.contains("search_text"));
    assert!(names.contains("read_many"));
    assert!(names.contains("edit"));
    assert!(!names.contains("edit_file"));
    assert!(!names.contains("edit_many"));
    assert!(names.contains("wait_command"));
    assert!(names.contains("send_input"));
    assert!(names.contains("exec_many"));
    assert!(names.contains("query_tool_usage"));
    assert!(names.contains("list_workspace_folders"));
    assert!(names.contains("switch_workspace_folder"));
    assert!(!names.contains("write_stdin"));
    for tool in [
        "project_map",
        "edit",
        "file_ops",
        "git_branch",
        "git_stage",
        "git_commit",
        "git_restore",
    ] {
        assert!(names.contains(tool), "missing core tool: {tool}");
    }
    assert!(names.contains("history_session_bootstrap"));
    assert!(names.contains("history_session_checkpoint"));
    for removed in [
        "history_session_validate",
        "check_exec_environment",
        "get_default_cwd",
        "list_dir",
        "grep_text",
        "request_permissions",
    ] {
        assert!(!names.contains(removed), "removed core tool: {removed}");
    }
    assert!(!names.contains("harness_status"));
    assert!(!names.contains("start_task"));
}

#[test]
fn exec_health_check_reports_worker_and_pipe_status() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(&ctx, "exec_health_check", json!({}));
    let payload = assert_ok(&out);
    assert_eq!(payload["worker"]["alive"], true);
    assert_eq!(payload["session_create"], true);
    assert_eq!(payload["command_run"], true);
    assert_eq!(payload["stdout_capture"], true);
    assert_eq!(payload["stderr_capture"], true);
}

#[test]
fn native_diagnostics_support_pwd_and_ls_without_a_shell() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);

    let pwd_result = invoke(&ctx, "exec_command", json!({"cmd": "pwd"}));
    let pwd = assert_ok(&pwd_result);
    assert_eq!(pwd["command"], "pwd");
    assert!(pwd["stdout"]
        .as_str()
        .unwrap_or("")
        .contains("tiny-js-project"));
    assert_eq!(pwd["execution_mode"], "native_builtin");
    assert_eq!(pwd["harness_mode"], "standalone");
    assert_eq!(pwd["task_required"], false);
    assert_eq!(pwd["command_runner"], "native_builtin");
    assert_eq!(pwd["status"], "exited");
    assert_eq!(pwd["exit_code"], 0);
    assert_eq!(pwd["transport_ok"], true);
    assert_eq!(pwd["command_ok"], true);
    assert_eq!(pwd["duration_ms"], 0);
    assert_eq!(pwd["elapsed_ms"], 0);
    assert!(pwd["stdout"].is_string());
    assert_eq!(pwd["stderr"], "");

    let ls_result = invoke(&ctx, "exec_command", json!({"cmd": "ls"}));
    let ls = assert_ok(&ls_result);
    assert!(ls["stdout"].as_str().unwrap_or("").contains("src"));
    assert_eq!(ls["exit_code"], 0);
}

#[test]
fn direct_exec_uses_the_same_result_contract() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let result = invoke(
        &ctx,
        "exec_command",
        json!({"cmd": format!("{TEST_PYTHON} --version"), "filesystem_scope": "workspace"}),
    );
    let payload = assert_ok(&result);

    assert_eq!(payload["command"], format!("{TEST_PYTHON} --version"));
    assert_eq!(payload["execution_mode"], "direct");
    assert_eq!(payload["harness_mode"], "standalone");
    assert_eq!(payload["task_required"], false);
    assert_eq!(payload["status"], "exited");
    assert_eq!(payload["exit_code"], 0);
    assert!(payload["stdout"].is_string());
    assert!(payload["stderr"].is_string());
    assert!(payload["duration_ms"].is_u64());
    assert_eq!(payload["duration_ms"], payload["elapsed_ms"]);
    assert_eq!(payload["transport_ok"], true);
    assert_eq!(payload["command_ok"], true);
}

#[test]
fn nonzero_command_exit_keeps_transport_ok_but_sets_command_ok_false() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let result = invoke(
        &ctx,
        "exec_command",
        json!({
            "cmd": format!("{TEST_PYTHON} -c \"import sys; sys.exit(1)\""),
            "filesystem_scope": "workspace"
        }),
    );
    let payload = assert_ok(&result);

    assert_eq!(payload["ok"], true);
    assert_eq!(payload["transport_ok"], true);
    assert_eq!(payload["command_ok"], false);
    assert_eq!(payload["status"], "exited");
    assert_eq!(payload["exit_code"], 1);
    assert_eq!(payload["failure_origin"], "child_process");
    assert_eq!(payload["policy_blocked"], false);
    assert_eq!(payload["execution_attempted"], true);
    assert_eq!(payload["process_started"], true);
    assert!(payload["diagnostic_summary"]
        .as_str()
        .unwrap_or("")
        .contains("MCP policy did not block execution"));
}

#[test]
fn retained_session_timeout_stops_the_process_after_deadline() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let result = invoke(
        &ctx,
        "exec_command",
        json!({
            "cmd": format!("{TEST_PYTHON} -c \"import time; time.sleep(2)\""),
            "filesystem_scope": "workspace",
            "timeout_ms": 100,
            "yield_time_ms": 0
        }),
    );
    let payload = assert_ok(&result);
    assert_eq!(payload["status"], "running");
    assert_eq!(payload["transport_ok"], true);
    assert_eq!(payload["command_ok"], Value::Null);
    assert_eq!(payload["stdin_open"], true);
    let session_id = payload["session_id"].as_str().expect("session id");

    let after = invoke(
        &ctx,
        "wait_command",
        json!({
            "session_id": session_id,
            "timeout_ms": 5000,
            "until": "finalized",
            "output_mode": "none"
        }),
    );
    assert_eq!(after["termination_reason"], "process_timeout");
    assert_eq!(after["status"], "timed_out");
    assert_eq!(after["request_timed_out"], false);
    assert_eq!(after["process_timed_out"], true);
    assert_eq!(after["process_still_running"], false);
    assert_eq!(after["transport_ok"], true);
    assert_eq!(after["command_ok"], false);
    assert_eq!(after["stdin_open"], false);
    #[cfg(unix)]
    assert_eq!(after["exit_code"], Value::Null);
}

#[test]
fn killed_session_reports_command_failure_even_when_transport_succeeds() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let result = invoke(
        &ctx,
        "exec_command",
        json!({
            "cmd": format!("{TEST_PYTHON} -c \"import time; time.sleep(2)\""),
            "filesystem_scope": "workspace",
            "timeout_ms": 10_000,
            "yield_time_ms": 0
        }),
    );
    let payload = assert_ok(&result);
    let session_id = payload["session_id"].as_str().expect("session id");

    let killed = invoke(
        &ctx,
        "kill_session",
        json!({"session_id": session_id, "wait_ms": 2_000}),
    );
    let killed = assert_ok(&killed);
    assert_eq!(killed["status"], "killed");
    assert_eq!(killed["killed"], true);
    assert_eq!(killed["transport_ok"], true);
    assert_eq!(killed["command_ok"], false);
    #[cfg(unix)]
    assert_eq!(killed["exit_code"], Value::Null);
}

#[test]
fn list_files_accepts_glob_alias() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let out = invoke(
        &ctx,
        "list_files",
        json!({"glob": "**/*.js", "max_results": 10}),
    );
    let payload = assert_ok(&out);
    let entries = payload["entries"].as_array().expect("entries array");
    assert!(!entries.is_empty());
    assert!(entries
        .iter()
        .all(|f| f["path"].as_str().unwrap_or("").ends_with(".js")));
}

#[test]
fn search_text_filters_by_glob() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let hit = invoke(
        &ctx,
        "search_text",
        json!({"query": "function add", "glob": "**/*.js", "max_results": 10}),
    );
    let hit_payload = assert_ok(&hit);
    assert!(hit_payload["total_matches"].as_u64().unwrap_or(0) > 0);

    let miss = invoke(
        &ctx,
        "search_text",
        json!({"query": "function add", "glob": "**/*.py"}),
    );
    let miss_payload = assert_ok(&miss);
    assert_eq!(miss_payload["total_matches"].as_u64().unwrap_or(1), 0);
}

#[test]
fn removed_grep_alias_is_rejected_in_favor_of_search_text() {
    let fx = tiny_js_fixture();
    let ctx = ctx_for(&fx.root);
    let output = invoke(
        &ctx,
        "grep_text",
        json!({
            "query": "function\\s+add",
            "path": "."
        }),
    );
    let error = assert_err(&output);
    assert_eq!(error["error"]["code"], "INVALID_ARGUMENT");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Unknown tool"));
}
