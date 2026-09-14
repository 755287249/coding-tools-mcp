use super::*;
use crate::tools::context::ToolContext;
use crate::tools::dispatch::call_tool;
use serde_json::json;
use tempfile::tempdir;

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn appcontainer_backend_uses_shared_session_and_post_check_lifecycle() {
    let root = tempdir().expect("sandbox lifecycle root");
    let workspace = root.path().join("inside");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx =
        ToolContext::for_test(workspace.clone(), harness.path().to_path_buf()).expect("context");

    let main_escape = root.path().join("main-escaped.txt");
    let post_escape = root.path().join("post-escaped.txt");
    let main_script = workspace.join("sandbox-lifecycle-main.cmd");
    let post_script = workspace.join("sandbox-lifecycle-post.cmd");
    std::fs::write(
        &main_script,
        "@echo off\r\n> \"..\\main-escaped.txt\" echo escaped\r\nif exist \"..\\main-escaped.txt\" exit /b 8\r\necho sandbox-session-main-ok\r\nexit /b 0\r\n",
    )
    .expect("main script");
    std::fs::write(
        &post_script,
        "@echo off\r\n> \"..\\post-escaped.txt\" echo escaped\r\nif exist \"..\\post-escaped.txt\" exit /b 9\r\necho sandbox-session-post-ok\r\nexit /b 0\r\n",
    )
    .expect("post script");

    let prepared: std::sync::Arc<dyn crate::tools::sandbox::PreparedSandbox> = std::sync::Arc::from(
        crate::tools::sandbox::prepare_backend_for_test("appcontainer", &ctx.workspace)
            .expect("prepare AppContainer backend"),
    );
    let backend = backend::CommandExecutionBackend::Sandbox(prepared);
    let spec = ExecSpec {
        display: main_script.display().to_string(),
        program: main_script.display().to_string(),
        args: Vec::new(),
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let post_check = spec::PostCheckSpec {
        name: "sandbox-post-check".into(),
        exec: ExecSpec {
            display: post_script.display().to_string(),
            program: post_script.display().to_string(),
            args: Vec::new(),
            shell: "none".into(),
            env: Vec::new(),
            remove_env: Vec::new(),
        },
        expected_exit_code: 0,
        timeout: Duration::from_secs(5),
        max_output_bytes: 16_384,
    };
    let identity = execution_identity(
        &json!({"program": spec.program.clone(), "args": []}),
        &spec,
        &workspace,
        5_000,
        false,
        "",
        std::slice::from_ref(&post_check),
    );

    let result = crate::task_runtime::block_on(run_command(
        &ctx,
        backend,
        Some(0),
        &spec,
        &workspace,
        Duration::from_secs(5),
        Duration::from_secs(5),
        OutputOptions::tail(16_384),
        false,
        "",
        vec![post_check],
        false,
        identity,
        0,
        None,
    ))
    .expect("sandbox lifecycle run");

    assert_eq!(result["execution_ok"], true, "{result}");
    assert_eq!(result["verification_ok"], true, "{result}");
    assert_eq!(
        result["sandbox_phase_durations_ms"]["prepare_ms"], 0,
        "{result}"
    );
    assert!(
        result["sandbox_phase_durations_ms"]["startup_ms"].is_number(),
        "{result}"
    );
    assert!(
        result["sandbox_phase_durations_ms"]["cleanup_ms"].is_number(),
        "{result}"
    );
    assert!(result["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("sandbox-session-main-ok"));
    assert!(result["post_checks"]["results"][0]["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("sandbox-session-post-ok"));
    assert!(!main_escape.exists(), "main command escaped AppContainer");
    assert!(!post_escape.exists(), "post-check escaped AppContainer");
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn appcontainer_backend_shared_session_supports_stdin_and_timeout() {
    let workspace = tempdir().expect("sandbox lifecycle workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let prepared: std::sync::Arc<dyn crate::tools::sandbox::PreparedSandbox> = std::sync::Arc::from(
        crate::tools::sandbox::prepare_backend_for_test("appcontainer", &ctx.workspace)
            .expect("prepare AppContainer backend"),
    );
    let backend = backend::CommandExecutionBackend::Sandbox(prepared);

    let stdin_script = workspace.path().join("sandbox-stdin.cmd");
    std::fs::write(
        &stdin_script,
        "@echo off\r\nset /p CTMCP_INPUT=\r\necho sandbox-stdin=[%CTMCP_INPUT%]\r\necho sandbox-stderr-ok 1>&2\r\n",
    )
    .expect("stdin script");
    let stdin_spec = ExecSpec {
        display: stdin_script.display().to_string(),
        program: stdin_script.display().to_string(),
        args: Vec::new(),
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let stdin_identity = execution_identity(
        &json!({"program": stdin_spec.program.clone()}),
        &stdin_spec,
        workspace.path(),
        5_000,
        false,
        "hello-from-appcontainer\r\n",
        &[],
    );
    let stdin_result = crate::task_runtime::block_on(run_command(
        &ctx,
        backend.clone(),
        Some(0),
        &stdin_spec,
        workspace.path(),
        Duration::from_secs(5),
        Duration::from_secs(5),
        OutputOptions::tail(16_384),
        false,
        "hello-from-appcontainer\r\n",
        Vec::new(),
        false,
        stdin_identity,
        0,
        None,
    ))
    .expect("stdin lifecycle run");
    assert_eq!(stdin_result["execution_ok"], true, "{stdin_result}");
    assert!(stdin_result["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("sandbox-stdin=[hello-from-appcontainer]"));
    assert!(stdin_result["stderr"]
        .as_str()
        .unwrap_or_default()
        .contains("sandbox-stderr-ok"));
    assert_eq!(stdin_result["startup"]["attempts"], 1, "{stdin_result}");

    let timeout_script = workspace.path().join("sandbox-timeout.ps1");
    std::fs::write(
        &timeout_script,
        "Start-Sleep -Seconds 10\r\nWrite-Output 'sandbox-timeout-should-not-complete'\r\n",
    )
    .expect("timeout script");
    let timeout_spec = ExecSpec {
        display: timeout_script.display().to_string(),
        program: timeout_script.display().to_string(),
        args: Vec::new(),
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let timeout_identity = execution_identity(
        &json!({"program": timeout_spec.program.clone()}),
        &timeout_spec,
        workspace.path(),
        250,
        false,
        "",
        &[],
    );
    let timeout_result = crate::task_runtime::block_on(run_command(
        &ctx,
        backend,
        Some(0),
        &timeout_spec,
        workspace.path(),
        Duration::from_millis(250),
        Duration::from_secs(5),
        OutputOptions::tail(16_384),
        false,
        "",
        Vec::new(),
        false,
        timeout_identity,
        0,
        None,
    ))
    .expect("timeout lifecycle run");
    assert_eq!(
        timeout_result["process_timed_out"], true,
        "{timeout_result}"
    );
    assert_eq!(
        timeout_result["termination_reason"], "process_timeout",
        "{timeout_result}"
    );
    assert!(!timeout_result["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("sandbox-timeout-should-not-complete"));

    let cancel_prepared: std::sync::Arc<dyn crate::tools::sandbox::PreparedSandbox> =
        std::sync::Arc::from(
            crate::tools::sandbox::prepare_backend_for_test("appcontainer", &ctx.workspace)
                .expect("prepare cancellation AppContainer backend"),
        );
    let cancel_script = workspace.path().join("sandbox-cancel.ps1");
    let cancel_marker = workspace
        .path()
        .join("sandbox-cancel-should-not-complete.txt");
    std::fs::write(
        &cancel_script,
        "Write-Output 'sandbox-cancel-ready'\r\nStart-Sleep -Seconds 10\r\nSet-Content -LiteralPath 'sandbox-cancel-should-not-complete.txt' -Value 'completed'\r\n",
    )
    .expect("cancellation script");
    let cancel_spec = ExecSpec {
        display: cancel_script.display().to_string(),
        program: cancel_script.display().to_string(),
        args: Vec::new(),
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let cancel_identity = execution_identity(
        &json!({"program": cancel_spec.program.clone()}),
        &cancel_spec,
        workspace.path(),
        30_000,
        false,
        "",
        &[],
    );
    let cancel_started = crate::task_runtime::block_on(run_command(
        &ctx,
        backend::CommandExecutionBackend::Sandbox(cancel_prepared),
        Some(0),
        &cancel_spec,
        workspace.path(),
        Duration::from_secs(30),
        Duration::from_secs(5),
        OutputOptions::tail(16_384),
        false,
        "",
        Vec::new(),
        false,
        cancel_identity,
        0,
        None,
    ))
    .expect("cancellation lifecycle run");
    assert_eq!(
        cancel_started["process_still_running"], true,
        "{cancel_started}"
    );
    assert_eq!(
        cancel_started["process_tree_contained"], true,
        "{cancel_started}"
    );
    assert!(cancel_started["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("sandbox-cancel-ready"));
    let session_id = cancel_started["session_id"]
        .as_str()
        .expect("sandbox cancellation session id");
    let killed = crate::task_runtime::block_on(crate::tools::session::kill_session_async(
        &ctx.sessions,
        &json!({"session_id": session_id, "wait_ms": 5000}),
    ))
    .expect("kill sandbox session");
    assert_eq!(killed["process_still_running"], false, "{killed}");
    let finalized = crate::task_runtime::block_on(crate::tools::session::wait_command_async(
        &ctx.sessions,
        &json!({
            "session_id": session_id,
            "timeout_ms": 5000,
            "until": "finalized",
            "output_mode": "none"
        }),
    ))
    .expect("wait for sandbox cleanup finalization");
    assert_eq!(finalized["process_still_running"], false, "{finalized}");
    assert!(
        finalized["sandbox_phase_durations_ms"]["cleanup_ms"].is_number(),
        "{finalized}"
    );
    assert!(
        !cancel_marker.exists(),
        "cancelled AppContainer command completed"
    );
}

#[cfg(windows)]
#[tokio::test]
#[serial_test::serial(process_runtime)]
async fn public_exec_enabled_appcontainer_uses_production_boundary() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let script = workspace.path().join("public-appcontainer.cmd");
    let marker = workspace.path().join("public-appcontainer-marker.txt");
    std::fs::write(
        &script,
        "@echo off\r\necho public-appcontainer-ok\r\necho contained>public-appcontainer-marker.txt\r\n",
    )
    .expect("script");

    let runtime = ctx.runtime_config();
    ctx.shared_runtime_config().update_with_sandbox(
        runtime.policy,
        runtime.tool_profile,
        runtime.permission_mode,
        crate::workspace::SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..crate::workspace::SandboxConfig::default()
        },
    );

    let result = exec_command_async(
        &ctx,
        &json!({
            "program": script.to_string_lossy(),
            "timeout_ms": 5000,
            "yield_time_ms": 5000,
            "output_mode": "tail"
        }),
    )
    .await
    .expect("ready AppContainer public exec");
    assert_eq!(result["execution_ok"], true, "{result}");
    assert_eq!(result["process_exit_code"], 0, "{result}");
    assert_eq!(result["sandbox_enforced"], true, "{result}");
    assert_eq!(result["sandbox_backend"], "appcontainer", "{result}");
    assert_eq!(result["execution_boundary"], "appcontainer", "{result}");
    assert_eq!(result["process_tree_contained"], true, "{result}");
    assert!(result["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("public-appcontainer-ok"));
    assert_eq!(
        std::fs::read_to_string(marker)
            .expect("public AppContainer marker")
            .trim(),
        "contained"
    );
}

#[cfg(windows)]
#[tokio::test]
#[serial_test::serial(process_runtime)]
async fn public_exec_enabled_wslc_uses_production_boundary_when_explicitly_enabled() {
    if std::env::var("CTMCP_TEST_WSLC").as_deref() != Ok("1") {
        return;
    }

    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    std::fs::write(
        workspace.path().join("wslc-public-marker.txt"),
        "public-wslc-ok\n",
    )
    .expect("marker");

    let runtime = ctx.runtime_config();
    ctx.shared_runtime_config().update_with_sandbox(
        runtime.policy,
        runtime.tool_profile,
        runtime.permission_mode,
        crate::workspace::SandboxConfig {
            enabled: true,
            backend: "wslc".into(),
            options: std::collections::BTreeMap::from([(
                "wslc.image".into(),
                std::env::var("CTMCP_TEST_WSLC_IMAGE").unwrap_or_else(|_| "alpine:3.20".into()),
            )]),
            ..crate::workspace::SandboxConfig::default()
        },
    );

    let result = exec_command_async(
        &ctx,
        &json!({
            "program": "cat",
            "args": ["wslc-public-marker.txt"],
            "timeout_ms": 30000,
            "yield_time_ms": 30000,
            "output_mode": "tail"
        }),
    )
    .await
    .expect("ready WSLC public exec");
    assert_eq!(result["execution_ok"], true, "{result}");
    assert_eq!(result["process_exit_code"], 0, "{result}");
    assert_eq!(result["sandbox_enforced"], true, "{result}");
    assert_eq!(result["sandbox_backend"], "wslc", "{result}");
    assert_eq!(result["execution_boundary"], "wslc", "{result}");
    assert_eq!(result["process_tree_contained"], true, "{result}");
    assert!(result["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("public-wslc-ok"));
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn exec_health_check_reports_the_selected_disabled_boundary() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    let result = call_tool(&ctx, "exec_health_check", &json!({}));
    assert_eq!(result["status"], "success", "{result}");
    assert_eq!(result["probe"]["sandbox_enforced"], false, "{result}");
    assert_eq!(
        result["probe"]["execution_boundary"], "policy_only",
        "{result}"
    );
    assert_eq!(
        result["sandbox_verification"]["required"], false,
        "{result}"
    );
    assert!(
        result["sandbox_verification"]["verified"].is_null(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["prepare_ms"].is_null(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["startup_ms"].is_null(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["cleanup_ms"].is_null(),
        "{result}"
    );
    assert_eq!(result["stdout_capture"], true, "{result}");
    assert_eq!(result["stderr_capture"], true, "{result}");
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn exec_health_check_reports_enabled_appcontainer_boundary() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let runtime = ctx.runtime_config();
    ctx.shared_runtime_config().update_with_sandbox(
        runtime.policy,
        runtime.tool_profile,
        runtime.permission_mode,
        crate::workspace::SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..crate::workspace::SandboxConfig::default()
        },
    );

    let result = call_tool(&ctx, "exec_health_check", &json!({}));
    assert_eq!(result["status"], "success", "{result}");
    assert_eq!(result["probe"]["sandbox_enforced"], true, "{result}");
    assert_eq!(
        result["probe"]["execution_boundary"], "appcontainer",
        "{result}"
    );
    assert_eq!(result["sandbox_verification"]["required"], true, "{result}");
    assert_eq!(result["sandbox_verification"]["verified"], true, "{result}");
    assert_eq!(
        result["sandbox_verification"]["backend"], "appcontainer",
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["prepare_ms"].is_number(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["startup_ms"].is_number(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["cleanup_ms"].is_number(),
        "{result}"
    );
    assert_eq!(result["stdout_capture"], true, "{result}");
    assert_eq!(result["stderr_capture"], true, "{result}");
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn exec_health_check_reports_enabled_wslc_boundary_when_explicitly_enabled() {
    if std::env::var("CTMCP_TEST_WSLC").as_deref() != Ok("1") {
        return;
    }

    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let runtime = ctx.runtime_config();
    ctx.shared_runtime_config().update_with_sandbox(
        runtime.policy,
        runtime.tool_profile,
        runtime.permission_mode,
        crate::workspace::SandboxConfig {
            enabled: true,
            backend: "wslc".into(),
            options: std::collections::BTreeMap::from([(
                "wslc.image".into(),
                std::env::var("CTMCP_TEST_WSLC_IMAGE").unwrap_or_else(|_| "alpine:3.20".into()),
            )]),
            ..crate::workspace::SandboxConfig::default()
        },
    );

    let result = call_tool(&ctx, "exec_health_check", &json!({}));
    assert_eq!(result["status"], "success", "{result}");
    assert_eq!(result["probe"]["sandbox_enforced"], true, "{result}");
    assert_eq!(result["probe"]["sandbox_backend"], "wslc", "{result}");
    assert_eq!(result["probe"]["execution_boundary"], "wslc", "{result}");
    assert_eq!(result["sandbox_verification"]["required"], true, "{result}");
    assert_eq!(result["sandbox_verification"]["verified"], true, "{result}");
    assert_eq!(result["probe"]["process_tree_contained"], true, "{result}");
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["prepare_ms"].is_number(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["startup_ms"].is_number(),
        "{result}"
    );
    assert!(
        result["probe"]["sandbox_phase_durations_ms"]["cleanup_ms"].is_number(),
        "{result}"
    );
    assert_eq!(result["stdout_capture"], true, "{result}");
    assert_eq!(result["stderr_capture"], true, "{result}");
}

#[cfg(windows)]
#[test]
fn server_info_sandbox_telemetry_tracks_readiness_and_enablement() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let descriptor = crate::tools::sandbox::backend_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.id == "appcontainer")
        .expect("AppContainer descriptor");
    let selected_available = descriptor.host_supported && descriptor.enforcement_ready;

    let disabled = call_tool(&ctx, "server_info", &json!({}));
    assert_eq!(
        disabled["environment"]["filesystem_sandbox"]["enabled"],
        false
    );
    assert_eq!(
        disabled["environment"]["filesystem_sandbox"]["available"],
        selected_available
    );
    assert_eq!(
        disabled["environment"]["workspace_exec"]["sandbox_enforced"],
        false
    );
    assert_eq!(
        disabled["environment"]["workspace_exec"]["boundary"],
        "policy_only"
    );
    assert_eq!(disabled["environment"]["workspace_exec"]["available"], true);
    assert_eq!(disabled["runtime_revision"]["source_workspace"], false);
    assert_eq!(
        disabled["runtime_revision"]["trust_state"],
        "not_applicable"
    );
    assert_eq!(
        disabled["runtime_revision"]["workspace_clean_verified"],
        false
    );
    assert_eq!(
        disabled["runtime_revision"]["workspace_clean_verification_tool"],
        "git_status"
    );
    assert_eq!(
        disabled["environment"]["filesystem_sandbox"]["verification_tool"],
        "exec_health_check"
    );
    assert_eq!(
        disabled["environment"]["filesystem_sandbox"]["live_verification_required"],
        false
    );

    let runtime = ctx.runtime_config();
    ctx.shared_runtime_config().update_with_sandbox(
        runtime.policy,
        runtime.tool_profile,
        runtime.permission_mode,
        crate::workspace::SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..crate::workspace::SandboxConfig::default()
        },
    );
    let enabled = call_tool(&ctx, "server_info", &json!({}));
    assert_eq!(
        enabled["environment"]["filesystem_sandbox"]["enabled"],
        true
    );
    assert_eq!(
        enabled["environment"]["filesystem_sandbox"]["live_verification_required"],
        true
    );
    assert_eq!(
        enabled["environment"]["filesystem_sandbox"]["enforced"],
        selected_available
    );
    assert_eq!(
        enabled["environment"]["workspace_exec"]["available"],
        selected_available
    );
    assert_eq!(
        enabled["environment"]["workspace_exec"]["sandbox_enforced"],
        selected_available
    );
    assert_eq!(
        enabled["environment"]["workspace_exec"]["boundary"],
        if selected_available {
            "appcontainer"
        } else {
            "sandbox_unavailable"
        }
    );
}

#[cfg(windows)]
#[test]
fn server_info_reports_enabled_wslc_boundary_when_explicitly_enabled() {
    if std::env::var("CTMCP_TEST_WSLC").as_deref() != Ok("1") {
        return;
    }

    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let runtime = ctx.runtime_config();
    ctx.shared_runtime_config().update_with_sandbox(
        runtime.policy,
        runtime.tool_profile,
        runtime.permission_mode,
        crate::workspace::SandboxConfig {
            enabled: true,
            backend: "wslc".into(),
            options: std::collections::BTreeMap::from([(
                "wslc.image".into(),
                std::env::var("CTMCP_TEST_WSLC_IMAGE").unwrap_or_else(|_| "alpine:3.20".into()),
            )]),
            ..crate::workspace::SandboxConfig::default()
        },
    );

    let result = call_tool(&ctx, "server_info", &json!({}));
    assert_eq!(result["environment"]["filesystem_sandbox"]["enabled"], true);
    assert_eq!(
        result["environment"]["filesystem_sandbox"]["available"],
        true
    );
    assert_eq!(
        result["environment"]["filesystem_sandbox"]["enforced"],
        true
    );
    assert_eq!(
        result["environment"]["filesystem_sandbox"]["backend"],
        "wslc"
    );
    assert_eq!(result["environment"]["workspace_exec"]["available"], true);
    assert_eq!(
        result["environment"]["workspace_exec"]["sandbox_enforced"],
        true
    );
    assert_eq!(
        result["environment"]["workspace_exec"]["sandbox_backend"],
        "wslc"
    );
    assert_eq!(result["environment"]["workspace_exec"]["boundary"], "wslc");
}

fn assert_failure_result(error: WorkspaceError, expected_code: &str) {
    // Kept near timeout inference tests so failures remain easy to diagnose.
    let spec = ExecSpec {
        display: "missing-command".into(),
        program: "missing-command".into(),
        args: Vec::new(),
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let result = execution_failure_result(&error, &spec, Path::new("C:/workspace"))
        .expect("应转换为统一执行结果");
    assert_eq!(result["transport_ok"], true);
    assert_eq!(result["command_ok"], false);
    assert_eq!(result["status"], "spawn_failed");
    assert_eq!(result["error"]["code"], expected_code);
}

#[cfg(windows)]
#[test]
fn wsl_workspace_wraps_the_inner_command_without_windows_path_resolution() {
    let root = Path::new(r"\\?\UNC\wsl.localhost\Ubuntu-24.04\opt\src\SampleProject");
    let request = json!({"cmd": "cargo test"});
    let spec = resolve_exec_spec(
        &request,
        root,
        root,
        &crate::tools::policy::PolicySettings::default(),
    )
    .expect("WSL exec spec");
    let command = prepared_command(&spec, root, CommandIoMode::PostCheck);
    let command = command.as_std();

    assert_eq!(command.get_program(), "wsl.exe");
    assert_eq!(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec![
            "--distribution",
            "Ubuntu-24.04",
            "--cd",
            "/opt/src/SampleProject",
            "--exec",
            "cargo",
            "test",
        ]
    );

    let absolute = resolve_exec_spec(
        &json!({"program": "/usr/bin/cargo", "args": ["check"]}),
        root,
        root,
        &crate::tools::policy::PolicySettings::default(),
    )
    .expect("allowlisted absolute WSL program");
    assert_eq!(absolute.program, "/usr/bin/cargo");

    let spoofed = resolve_exec_spec(
        &json!({"program": "/tmp/cargo", "args": ["check"]}),
        root,
        root,
        &crate::tools::policy::PolicySettings::default(),
    )
    .expect_err("allowlisted basename outside trusted system directories must be rejected")
    .to_error_value();
    assert_eq!(spoofed["code"], "EXECUTABLE_OUTSIDE_WORKSPACE");

    let traversed = resolve_exec_spec(
        &json!({"program": "/usr/bin/../../tmp/cargo", "args": ["check"]}),
        root,
        root,
        &crate::tools::policy::PolicySettings::default(),
    )
    .expect_err("path traversal must not disguise an untrusted executable")
    .to_error_value();
    assert_eq!(traversed["code"], "EXECUTABLE_OUTSIDE_WORKSPACE");
}

#[test]
fn wsl_absolute_program_normalization_is_lexical_and_bounded() {
    assert_eq!(
        normalize_wsl_absolute_program_path("/usr/bin/../local/bin/cargo"),
        Some("/usr/local/bin/cargo".into())
    );
    assert_eq!(
        normalize_wsl_absolute_program_path("/usr//bin/./cargo"),
        Some("/usr/bin/cargo".into())
    );
    assert_eq!(
        normalize_wsl_absolute_program_path("/../../tmp/cargo"),
        None
    );
    assert!(is_trusted_wsl_system_program("/usr/bin/cargo"));
    assert!(is_trusted_wsl_system_program("/snap/bin/prettier"));
    assert!(!is_trusted_wsl_system_program("/tmp/cargo"));
    assert!(!is_trusted_wsl_system_program("/home/dev/bin/cargo"));
}

#[cfg(windows)]
#[test]
fn wsl_workspace_rejects_paths_unavailable_to_the_target_distribution() {
    let root = Path::new(r"\\?\UNC\wsl.localhost\Ubuntu-24.04\opt\src\SampleProject");
    let policy = crate::tools::policy::PolicySettings::default();

    let cross_distro = resolve_exec_spec(
        &json!({
            "program": "cargo",
            "args": [r"\\wsl.localhost\Debian\tmp\Cargo.toml"]
        }),
        root,
        root,
        &policy,
    )
    .expect_err("cross-distribution path must be rejected")
    .to_error_value();
    assert_eq!(cross_distro["code"], "WSL_CROSS_DISTRIBUTION_PATH");
    assert_eq!(cross_distro["details"]["workspace_distro"], "Ubuntu-24.04");
    assert_eq!(cross_distro["details"]["path_distro"], "Debian");

    let host_path = resolve_exec_spec(
        &json!({"program": "cargo", "args": [r"C:\src\Cargo.toml"]}),
        root,
        root,
        &policy,
    )
    .expect_err("Windows host path must be translated")
    .to_error_value();
    assert_eq!(host_path["code"], "WSL_HOST_PATH_REQUIRES_TRANSLATION");
    assert_eq!(host_path["details"]["position"], "args[0]");

    let unc_host_path = resolve_exec_spec(
        &json!({"program": "cargo", "args": [r"\\server\share\Cargo.toml"]}),
        root,
        root,
        &policy,
    )
    .expect_err("Windows UNC host path must be rejected")
    .to_error_value();
    assert_eq!(unc_host_path["code"], "WSL_HOST_PATH_REQUIRES_TRANSLATION");

    let invalid_env = resolve_exec_spec(
        &json!({"program": "cargo", "args": ["check"], "env": {"--help": "1"}}),
        root,
        root,
        &policy,
    )
    .expect_err("option-like WSL environment names must be rejected")
    .to_error_value();
    assert_eq!(invalid_env["code"], "WSL_ENVIRONMENT_INVALID");

    let invalid_removed_env = resolve_exec_spec(
        &json!({"program": "cargo", "args": ["check"], "remove_env": ["A=B"]}),
        root,
        root,
        &policy,
    )
    .expect_err("invalid removed WSL environment names must be rejected")
    .to_error_value();
    assert_eq!(invalid_removed_env["code"], "WSL_ENVIRONMENT_INVALID");
}

#[test]
fn docker_and_podman_select_the_portable_resolution_target() {
    let docker = crate::workspace::SandboxConfig {
        enabled: true,
        backend: "docker".into(),
        ..crate::workspace::SandboxConfig::default()
    };
    let podman = crate::workspace::SandboxConfig {
        enabled: true,
        backend: "podman".into(),
        ..crate::workspace::SandboxConfig::default()
    };
    assert_eq!(
        resolution_target_for_sandbox(&docker),
        ExecResolutionTarget::PortableSandbox
    );
    assert_eq!(
        resolution_target_for_sandbox(&podman),
        ExecResolutionTarget::PortableSandbox
    );
}

#[test]
fn docker_sbx_resolves_commands_inside_the_portable_target() {
    let workspace = tempdir().expect("workspace");
    let policy = crate::tools::policy::PolicySettings::default();

    let program = resolve_exec_spec_for_target(
        &json!({"program": "python", "args": ["--version"]}),
        workspace.path(),
        workspace.path(),
        &policy,
        ExecResolutionTarget::PortableSandbox,
    )
    .expect("portable program");
    assert_eq!(program.program, "python");
    assert_eq!(program.args, vec!["--version"]);

    let shell = resolve_exec_spec_for_target(
        &json!({"script": "printf portable", "shell": "sh"}),
        workspace.path(),
        workspace.path(),
        &policy,
        ExecResolutionTarget::PortableSandbox,
    )
    .expect("portable sh");
    assert_eq!(shell.program, "sh");
    assert_eq!(shell.args, vec!["-c", "printf portable"]);

    let cmd_error = resolve_exec_spec_for_target(
        &json!({"script": "echo host", "shell": "cmd"}),
        workspace.path(),
        workspace.path(),
        &policy,
        ExecResolutionTarget::PortableSandbox,
    )
    .expect_err("Windows cmd shell must not be forwarded into the Linux sandbox");
    assert!(cmd_error.to_string().contains("shell=cmd is unavailable"));
}

#[test]
fn 程序不存在时返回统一执行结果() {
    assert_failure_result(
        WorkspaceError::Tool {
            code: "COMMAND_REJECTED",
            message: "Program not found on PATH: missing-command".into(),
            category: "runtime",
            retryable: false,
        },
        "COMMAND_REJECTED",
    );
}

#[test]
fn 启动失败时返回统一执行结果() {
    assert_failure_result(
        WorkspaceError::ToolDetails {
            code: "COMMAND_SPAWN_FAILED",
            message: "Failed to start command".into(),
            category: "runtime",
            retryable: true,
            details: json!({"recoverable": true}),
        },
        "COMMAND_SPAWN_FAILED",
    );
}

#[test]
fn resolves_an_arbitrarily_named_workspace_local_entry() {
    let workspace = tempdir().expect("workspace");
    let entry = workspace.path().join("scripts").join("anything.cmd");
    std::fs::create_dir_all(entry.parent().expect("parent")).expect("scripts");
    std::fs::write(&entry, "echo test").expect("entry");
    let resolved = resolve_program(
        "scripts/anything.cmd",
        workspace.path(),
        workspace.path(),
        &crate::tools::policy::PolicySettings::default(),
    )
    .expect("workspace entry resolves");
    assert_eq!(
        std::path::Path::new(&resolved),
        entry.canonicalize().unwrap()
    );
}

#[cfg(windows)]
#[test]
fn windows_scripts_use_their_platform_runners() {
    let batch = command_for_program("C:/workspace/run-anything.cmd", &[]);
    assert_eq!(batch.as_std().get_program().to_string_lossy(), "cmd.exe");
    assert!(batch.as_std().get_args().any(|arg| arg == "/c"));
    assert_eq!(
        windows_batch_command_line(
            r"\\?\C:\workspace\Life Brain\run & tooling.cmd",
            &["argument & value".to_string()]
        ),
        r#"call "C:\workspace\Life Brain\run & tooling.cmd" "argument & value""#
    );

    let script = command_for_program("C:/workspace/run-anything.ps1", &[]);
    let runner = script
        .as_std()
        .get_program()
        .to_string_lossy()
        .to_ascii_lowercase();
    assert!(runner.contains("powershell") || runner.contains("pwsh"));
    assert!(script.as_std().get_args().any(|arg| arg == "-Command"));
}

#[cfg(windows)]
#[test]
fn neutral_process_spec_preserves_windows_launcher_and_environment_layers() {
    let workspace = tempdir().expect("workspace");
    let spec = ExecSpec {
        display: "run-anything.cmd".into(),
        program: "C:/workspace/run-anything.cmd".into(),
        args: vec!["argument & value".into()],
        shell: "none".into(),
        env: vec![
            ("KEEP_ME".into(), "yes".into()),
            ("PYTHONUTF8".into(), "0".into()),
        ],
        remove_env: vec!["DROP_ME".into(), "PYTHONUTF8".into()],
    };
    let prepared = prepared_process_spec(&spec, workspace.path());

    assert!(!prepared.using_wsl);
    assert_eq!(prepared.program, std::path::PathBuf::from("cmd.exe"));
    assert_eq!(prepared.args, vec!["/d", "/s", "/c"]);
    assert_eq!(
        prepared.windows_raw_arg.as_deref(),
        Some(r#"call "C:/workspace/run-anything.cmd" "argument & value""#)
    );
    assert_eq!(prepared.cwd.as_deref(), Some(workspace.path()));
    assert!(prepared.env.contains(&("KEEP_ME".into(), "yes".into())));
    assert!(prepared.env.contains(&("PYTHONUTF8".into(), "0".into())));
    assert!(prepared.remove_env.iter().any(|key| key == "PYTHONUTF8"));
    assert!(prepared
        .required_env
        .contains(&("PYTHONUTF8".into(), "1".into())));
    assert!(prepared
        .required_env
        .contains(&("PYTHONIOENCODING".into(), "utf-8".into())));
}

#[cfg(windows)]
#[test]
fn neutral_process_spec_normalizes_powershell_without_tokio_command_state() {
    let prepared = process_spec_for_program(
        "C:/workspace/run-anything.ps1",
        &["argument with spaces".into()],
    );
    let runner = prepared.program.to_string_lossy().to_ascii_lowercase();
    assert!(runner.contains("powershell") || runner.contains("pwsh"));
    assert!(prepared.args.iter().any(|arg| arg == "-Command"));
    assert!(prepared.args.last().is_some_and(
        |script| script.contains("run-anything.ps1") && script.contains("argument with spaces")
    ));
    assert!(prepared.windows_raw_arg.is_none());
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn powershell_script_mode_prefers_pwsh_and_preserves_utf8_output() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    let output = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "script": "Write-Output '中文輸出'",
            "shell": "powershell",
            "confirm": true,
            "timeout_ms": 10_000,
            "yield_time_ms": 10_000
        }),
    );

    assert_eq!(output["command_ok"], true, "{output}");
    assert!(
        output["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("中文輸出"),
        "{output}"
    );
    let environment = powershell_environment();
    assert_eq!(output["program"], environment["selected"], "{output}");
    if environment["pwsh_available"] == true {
        assert!(output["program"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("pwsh"));
    }
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn windows_workspace_scripts_and_python_unicode_execute_successfully() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    std::fs::write(
        workspace.path().join("any-name.cmd"),
        "@echo tooling-cmd-ok\r\n",
    )
    .expect("cmd script");
    std::fs::write(
        workspace.path().join("any-name.ps1"),
        "Write-Output 'tooling-powershell-ok'\r\n",
    )
    .expect("powershell script");
    std::fs::write(
        workspace.path().join("workflow_probe.py"),
        "print('workflow-ok')\n",
    )
    .expect("python module");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    for command in [
        "any-name.cmd",
        "any-name.ps1",
        "cmd /c echo tooling-cmd-ok",
        "powershell -NoProfile -Command \"Write-Output tooling-powershell-ok\"",
        "python -c \"print('中文输出正常 ✅')\"",
    ] {
        let initial = call_tool(
            &ctx,
            "exec_command",
            &json!({ "cmd": command, "timeout_ms": 10_000, "yield_time_ms": 10_000 }),
        );
        assert_eq!(initial["ok"], true, "{command}: {initial}");
        assert_eq!(initial["startup"]["attempts"], 1, "{command}: {initial}");
        assert_eq!(
            initial["startup"]["error_dialog_suppressed"], true,
            "{command}: {initial}"
        );
        let output = if initial["process_still_running"] == true {
            call_tool(
                &ctx,
                "wait_command",
                &json!({
                    "session_id": initial["session_id"],
                    "cursor": initial["next_cursor"],
                    "timeout_ms": 10_000,
                    "until": "finalized"
                }),
            )
        } else {
            initial
        };
        assert_eq!(output["command_ok"], true, "{command}: {output}");
    }

    for _ in 0..10 {
        let initial = call_tool(
            &ctx,
            "exec_command",
            &json!({
                "cmd": "python -m workflow_probe",
                "timeout_ms": 10_000,
                "yield_time_ms": 10_000
            }),
        );
        let initial_stdout = initial["stdout"].as_str().unwrap_or_default().to_string();
        let output = if initial["process_still_running"] == true {
            call_tool(
                &ctx,
                "wait_command",
                &json!({
                    "session_id": initial["session_id"],
                    "cursor": initial["next_cursor"],
                    "timeout_ms": 10_000,
                    "until": "finalized"
                }),
            )
        } else {
            initial
        };
        assert_eq!(output["command_ok"], true, "{output}");
        assert!(
            initial_stdout.contains("workflow-ok")
                || output["stdout"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("workflow-ok"),
            "{output}"
        );
    }
}

#[cfg(windows)]
#[test]
#[serial_test::serial(process_runtime)]
fn windows_batch_scripts_preserve_space_paths_and_arguments() {
    let parent = tempdir().expect("workspace parent");
    let workspace = parent.path().join("Life Brain 中文");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx =
        ToolContext::for_test(workspace.clone(), harness.path().to_path_buf()).expect("context");

    for extension in ["cmd", "bat"] {
        let script_name = format!("run & tooling.{extension}");
        std::fs::write(
                workspace.join(&script_name),
                "@echo off\r\nif not \"%~1\"==\"argument & value\" exit /b 7\r\necho tooling-space-path-ok\r\n",
            )
            .expect("batch script");

        let command = format!(r#""{script_name}" "argument & value""#);
        let output = call_tool(
            &ctx,
            "exec_command",
            &json!({ "cmd": command, "timeout_ms": 10_000, "yield_time_ms": 10_000 }),
        );
        assert_eq!(output["command_ok"], true, "{script_name}: {output}");
        let stdout = output["stdout"].as_str().unwrap_or_default();
        assert!(
            stdout.contains("tooling-space-path-ok"),
            "{script_name}: {output}"
        );
    }
}

#[cfg(windows)]
fn delayed_output_command() -> &'static str {
    "cmd.exe /D /C \"(echo alpha)& ping -n 2 127.0.0.1 >nul & (echo beta)\""
}

#[cfg(unix)]
fn delayed_output_command() -> &'static str {
    "sh -c \"printf 'alpha\\n'; sleep 1; printf 'beta\\n'\""
}

#[cfg(windows)]
fn immediate_success_command() -> &'static str {
    "cmd.exe /D /C \"exit /B 0\""
}

#[cfg(unix)]
fn immediate_success_command() -> &'static str {
    "sh -c \"true\""
}

#[cfg(windows)]
fn sleeping_command() -> &'static str {
    "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 1200\""
}

#[cfg(unix)]
fn sleeping_command() -> &'static str {
    "sh -c \"sleep 2\""
}

#[test]
fn cargo_target_lock_uses_manifest_and_tauri_target_directories() {
    let workspace = tempdir().expect("workspace");
    let src_tauri = workspace.path().join("src-tauri");
    std::fs::create_dir_all(&src_tauri).expect("src-tauri");
    std::fs::write(
        src_tauri.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .expect("manifest");
    let tauri = ExecSpec {
        display: "cargo tauri build".into(),
        program: "cargo".into(),
        args: vec!["tauri".into(), "build".into()],
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let (_, tauri_target) = cargo_target_lock(&tauri, workspace.path()).expect("tauri lock");
    assert_eq!(
        std::path::Path::new(&tauri_target),
        src_tauri.join("target")
    );

    let manifest = ExecSpec {
        display: "cargo check --manifest-path src-tauri/Cargo.toml".into(),
        program: "cargo".into(),
        args: vec![
            "check".into(),
            "--manifest-path".into(),
            "src-tauri/Cargo.toml".into(),
        ],
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let (manifest_group, manifest_target) =
        cargo_target_lock(&manifest, workspace.path()).expect("manifest lock");
    assert_eq!(
        std::path::Path::new(&manifest_target),
        src_tauri.join("target")
    );
    let (tauri_group, _) = cargo_target_lock(&tauri, workspace.path()).expect("tauri lock");
    assert_eq!(manifest_group, tauri_group);
}

#[test]
fn automatic_cargo_dedupe_uses_request_shape_after_executable_resolution() {
    let workspace = tempdir().expect("workspace");
    let spec = ExecSpec {
        display: r"C:\Users\tester\.cargo\bin\cargo.exe test --manifest-path src-tauri/Cargo.toml"
            .into(),
        program: r"C:\Users\tester\.cargo\bin\cargo.exe".into(),
        args: vec![
            "test".into(),
            "--manifest-path".into(),
            "src-tauri/Cargo.toml".into(),
        ],
        shell: "none".into(),
        env: Vec::new(),
        remove_env: Vec::new(),
    };
    let request = json!({
        "program": "cargo",
        "args": ["test", "--manifest-path", "src-tauri/Cargo.toml"]
    });

    let identity = execution_identity(&request, &spec, workspace.path(), 30_000, false, "", &[]);

    assert!(
        identity
            .operation_id
            .as_deref()
            .is_some_and(|operation_id| operation_id.starts_with("auto:")),
        "resolved cargo.exe paths must not disable automatic deduplication: {identity:?}"
    );
}

#[test]
#[serial_test::serial(process_runtime)]
fn automatic_dedupe_only_reattaches_active_sessions() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    let completed_request = json!({
        "cmd": immediate_success_command(),
        "deduplicate": true,
        "timeout_ms": 5000,
        "yield_time_ms": 5000,
        "output_mode": "none"
    });
    let first = call_tool(&ctx, "exec_command", &completed_request);
    assert_eq!(first["process_still_running"], false, "{first}");
    assert_eq!(first["deduplicated"], false, "{first}");
    let second = call_tool(&ctx, "exec_command", &completed_request);
    assert_ne!(second["session_id"], first["session_id"], "{second}");
    assert_eq!(second["deduplicated"], false, "{second}");

    let active_request = json!({
        "cmd": sleeping_command(),
        "deduplicate": true,
        "timeout_ms": 5000,
        "yield_time_ms": 0,
        "output_mode": "none"
    });
    let active = call_tool(&ctx, "exec_command", &active_request);
    assert_eq!(active["process_still_running"], true, "{active}");
    let active_session_id = active["session_id"].as_str().expect("session id");
    let duplicate = call_tool(&ctx, "exec_command", &active_request);
    assert_eq!(duplicate["session_id"], active_session_id, "{duplicate}");
    assert_eq!(duplicate["deduplicated"], true, "{duplicate}");
    assert_eq!(
        duplicate["attached_to_session_id"], active_session_id,
        "{duplicate}"
    );
    let _ = call_tool(
        &ctx,
        "kill_session",
        &json!({"session_id": active_session_id, "wait_ms": 5000}),
    );
}

#[test]
#[serial_test::serial(process_runtime)]
fn wait_command_caps_long_requested_wait_to_transport_safe_chunk() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let started = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": sleeping_command(),
            "timeout_ms": 10_000,
            "yield_time_ms": 0,
            "output_mode": "none"
        }),
    );
    let session_id = started["session_id"].as_str().expect("session id");
    let waited_at = std::time::Instant::now();
    let waited = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "cursor": started["next_cursor"],
            "timeout_ms": 60 * 60_000,
            "until": "exit",
            "output_mode": "none"
        }),
    );
    assert!(
        waited_at.elapsed() < std::time::Duration::from_secs(5),
        "{waited}"
    );
    assert_eq!(waited["wait_timeout_ms"], 60 * 60_000, "{waited}");
    assert_eq!(waited["effective_wait_ms"], 20_000, "{waited}");
    assert_eq!(waited["process_still_running"], false, "{waited}");
}
#[test]
#[serial_test::serial(process_runtime)]
fn duplicate_operations_reattach_and_ignore_legacy_wait_heartbeats() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let request = json!({
        "cmd": sleeping_command(),
        "operation_id": "dedupe-regression",
        "timeout_ms": 5000,
        "yield_time_ms": 0,
        "output_mode": "none"
    });
    let started = call_tool(&ctx, "exec_command", &request);
    assert_eq!(started["process_still_running"], true, "{started}");
    assert_eq!(started["deduplicated"], false, "{started}");
    let session_id = started["session_id"].as_str().expect("session id");

    let duplicate = call_tool(&ctx, "exec_command", &request);
    assert_eq!(duplicate["deduplicated"], true, "{duplicate}");
    assert_eq!(duplicate["session_id"], session_id, "{duplicate}");
    assert_eq!(
        duplicate["attached_to_session_id"], session_id,
        "{duplicate}"
    );

    let resolved = call_tool(
        &ctx,
        "resolve_operation",
        &json!({"operation_id": "dedupe-regression", "output_mode": "none"}),
    );
    assert_eq!(resolved["session_id"], session_id, "{resolved}");
    assert_eq!(resolved["deduplicated"], true, "{resolved}");

    let listed = call_tool(
        &ctx,
        "list_sessions",
        &json!({"include_finalized": false, "limit": 10}),
    );
    assert!(listed["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .any(|session| session["session_id"] == session_id));

    let waited = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "cursor": started["next_cursor"],
            "timeout_ms": 1000,
            "heartbeat_ms": 25,
            "until": "finalized",
            "output_mode": "none"
        }),
    );
    assert_eq!(waited["heartbeat"], false, "{waited}");
    assert_eq!(waited["request_timed_out"], true, "{waited}");
    assert_eq!(waited["effective_wait_ms"], 1000, "{waited}");
    assert!(
        waited["actual_wait_ms"].as_u64().unwrap_or(0) >= 900,
        "{waited}"
    );
    assert_eq!(waited["process_still_running"], true, "{waited}");
    assert_eq!(
        waited["next_actions"][0]["arguments"]["session_id"], session_id,
        "{waited}"
    );
    assert!(
        waited["next_actions"][0]["arguments"]
            .get("heartbeat_ms")
            .is_none(),
        "{waited}"
    );

    let conflict = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": delayed_output_command(),
            "operation_id": "dedupe-regression",
            "timeout_ms": 5000,
            "yield_time_ms": 0,
            "output_mode": "none"
        }),
    );
    assert_eq!(conflict["ok"], false, "{conflict}");
    assert_eq!(
        conflict["error"]["code"], "OPERATION_ID_CONFLICT",
        "{conflict}"
    );

    let killed = call_tool(
        &ctx,
        "kill_session",
        &json!({"session_id": session_id, "wait_ms": 5000}),
    );
    assert_eq!(killed["process_still_running"], false, "{killed}");
}

#[test]
#[serial_test::serial(process_runtime)]
fn wait_command_returns_only_new_sequence_events() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    let started = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": delayed_output_command(),
            "timeout_ms": 5000,
            "yield_time_ms": 0,
            "output_mode": "delta"
        }),
    );
    let session_id = started["session_id"].as_str().expect("session id");
    let first = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "cursor": 0,
            "timeout_ms": 3000,
            "until": "output_or_exit",
            "output_mode": "delta"
        }),
    );
    assert_eq!(first["request_timed_out"], false, "{first}");
    assert_eq!(first["event_detail"], "compact", "{first}");
    assert!(first["events"][0].get("data").is_none(), "{first}");
    assert!(
        first["session_registry_wait_ms"].as_u64().is_some(),
        "{first}"
    );
    assert!(first["actual_wait_ms"].as_u64().is_some(), "{first}");
    assert!(first["snapshot_ms"].as_u64().is_some(), "{first}");
    assert!(
        first["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("alpha"),
        "{first}"
    );
    let cursor = first["next_cursor"].as_u64().expect("cursor");

    let second = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "cursor": cursor,
            "timeout_ms": 5000,
            "until": "finalized",
            "output_mode": "delta",
            "event_detail": "full"
        }),
    );
    let stdout = second["stdout"].as_str().unwrap_or_default();
    assert_eq!(second["event_detail"], "full", "{second}");
    assert!(
        second["events"][0]["data"]
            .as_str()
            .unwrap_or_default()
            .contains("beta"),
        "{second}"
    );
    assert!(stdout.contains("beta"), "{second}");
    assert!(
        !stdout.contains("alpha"),
        "old output must not repeat: {second}"
    );
    assert!(
        second["next_cursor"].as_u64().unwrap_or(0) > cursor,
        "{second}"
    );
    assert_eq!(second["process_still_running"], false, "{second}");
}

#[test]
#[serial_test::serial(process_runtime)]
fn wait_timeout_does_not_become_process_timeout() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let started = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": sleeping_command(),
            "timeout_ms": 5000,
            "yield_time_ms": 0,
            "output_mode": "none"
        }),
    );
    let session_id = started["session_id"].as_str().expect("session id");
    let waited = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "cursor": 0,
            "timeout_ms": 50,
            "until": "output_or_exit",
            "output_mode": "none"
        }),
    );
    assert_eq!(waited["request_timed_out"], true, "{waited}");
    assert_eq!(waited["process_timed_out"], false, "{waited}");
    assert_eq!(waited["process_still_running"], true, "{waited}");
    let _ = call_tool(
        &ctx,
        "kill_session",
        &json!({"session_id": session_id, "wait_ms": 5000}),
    );
}

#[test]
#[serial_test::serial(process_runtime)]
fn non_tty_session_accepts_late_stdin_when_initial_stdin_is_empty() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    #[cfg(windows)]
    let command = r#"powershell -NoProfile -Command "$line = [Console]::In.ReadLine(); Write-Output ('stdin:' + $line)""#;
    #[cfg(unix)]
    let command = r#"sh -c 'IFS= read -r line; printf "stdin:%s\n" "$line"'"#;

    let started = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": command,
            "timeout_ms": 5000,
            "yield_time_ms": 0,
            "output_mode": "none"
        }),
    );
    assert_eq!(started["process_still_running"], true, "{started}");
    let session_id = started["session_id"].as_str().expect("session id");

    let sent = call_tool(
        &ctx,
        "send_input",
        &json!({
            "session_id": session_id,
            "chars": "late-input\n",
            "close_stdin": true
        }),
    );
    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(sent["bytes_written"], 11, "{sent}");
    assert_eq!(sent["stdin_closed"], true, "{sent}");

    let finished = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "cursor": 0,
            "timeout_ms": 5000,
            "until": "finalized",
            "output_mode": "all"
        }),
    );
    assert_eq!(finished["command_ok"], true, "{finished}");
    assert!(
        finished["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("stdin:late-input"),
        "{finished}"
    );
}
#[test]
#[serial_test::serial(process_runtime)]
fn process_timeout_is_reported_separately() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let started = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": sleeping_command(),
            "timeout_ms": 100,
            "yield_time_ms": 0,
            "output_mode": "none"
        }),
    );
    let session_id = started["session_id"].as_str().expect("session id");
    let finished = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "timeout_ms": 5000,
            "until": "finalized",
            "output_mode": "none"
        }),
    );
    assert_eq!(finished["request_timed_out"], false, "{finished}");
    assert_eq!(finished["process_timed_out"], true, "{finished}");
    assert_eq!(finished["process_still_running"], false, "{finished}");
    assert_eq!(
        finished["termination_reason"], "process_timeout",
        "{finished}"
    );
}

#[test]
#[serial_test::serial(process_runtime)]
fn kill_session_retains_exited_session_until_post_checks_finalize() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    #[cfg(windows)]
    let main_command = "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 50; exit 0\"";
    #[cfg(unix)]
    let main_command = "sh -c \"sleep 0.05; exit 0\"";
    #[cfg(windows)]
    let slow_check = "powershell -NoProfile -Command \"Start-Sleep -Milliseconds 750; exit 0\"";
    #[cfg(unix)]
    let slow_check = "sh -c \"sleep 0.75; exit 0\"";

    let started = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": main_command,
            "timeout_ms": 5000,
            "yield_time_ms": 0,
            "post_checks": [{"name": "slow-verify", "cmd": slow_check, "timeout_ms": 10_000}]
        }),
    );
    let session_id = started["session_id"].as_str().expect("session id");
    let exited = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "timeout_ms": 5000,
            "until": "exit",
            "output_mode": "none"
        }),
    );
    assert_eq!(exited["process_still_running"], false, "{exited}");

    let verifying = call_tool(
        &ctx,
        "kill_session",
        &json!({"session_id": session_id, "wait_ms": 0}),
    );
    assert_eq!(verifying["status"], "verifying", "{verifying}");
    assert_eq!(verifying["evicted"], false, "{verifying}");

    let finalized = call_tool(
        &ctx,
        "wait_command",
        &json!({
            "session_id": session_id,
            "timeout_ms": 15_000,
            "until": "finalized",
            "output_mode": "none"
        }),
    );
    assert_eq!(finalized["request_timed_out"], false, "{finalized}");
    assert_eq!(
        finalized["post_checks"]["results"][0]["passed"], true,
        "{finalized}"
    );

    let evicted = call_tool(
        &ctx,
        "kill_session",
        &json!({"session_id": session_id, "wait_ms": 0}),
    );
    assert_eq!(evicted["evicted"], true, "{evicted}");
}

#[test]
#[serial_test::serial(process_runtime)]
fn post_checks_are_part_of_final_command_success() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    #[cfg(windows)]
    let main_command = "cmd /d /c echo main-ok";
    #[cfg(unix)]
    let main_command = "sh -c \"printf main-ok\"";
    #[cfg(windows)]
    let passing_check = "cmd /d /c echo verify-ok";
    #[cfg(unix)]
    let passing_check = "sh -c \"printf verify-ok\"";
    #[cfg(windows)]
    let failing_check = "cmd /d /c exit 7";
    #[cfg(unix)]
    let failing_check = "sh -c \"exit 7\"";

    let passed = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": main_command,
            "timeout_ms": 5000,
            "yield_time_ms": 5000,
            "post_checks": [{"name": "verify", "cmd": passing_check}]
        }),
    );
    assert_eq!(passed["execution_ok"], true, "{passed}");
    assert_eq!(passed["verification_ok"], true, "{passed}");
    assert_eq!(passed["command_ok"], true, "{passed}");
    assert_eq!(
        passed["post_checks"]["results"][0]["passed"], true,
        "{passed}"
    );
    assert_eq!(
        passed["post_checks"]["execution_mode"], "parallel",
        "{passed}"
    );
    assert_eq!(passed["post_checks"]["max_concurrency"], 1, "{passed}");
    assert_eq!(
        passed["post_checks"]["results"][0]["startup"]["attempts"], 1,
        "{passed}"
    );

    let failed = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": main_command,
            "timeout_ms": 5000,
            "yield_time_ms": 5000,
            "post_checks": [{"name": "verify", "cmd": failing_check}]
        }),
    );
    assert_eq!(failed["execution_ok"], true, "{failed}");
    assert_eq!(failed["verification_ok"], false, "{failed}");
    assert_eq!(failed["command_ok"], false, "{failed}");
}

#[tokio::test]
#[serial_test::serial(process_runtime)]
async fn exec_returns_after_first_output_while_process_is_running() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    #[cfg(windows)]
    let command = "powershell -NoProfile -Command \"Write-Output ready; Start-Sleep -Seconds 3\"";
    #[cfg(unix)]
    let command = "sh -c \"printf ready; sleep 3\"";

    let result = exec_command_async(
        &ctx,
        &json!({
            "cmd": command,
            "timeout_ms": 10_000,
            "yield_time_ms": 5_000,
            "output_mode": "tail"
        }),
    )
    .await
    .expect("exec result");

    assert!(result["first_output_ms"].as_u64().is_some(), "{result}");
    assert!(result["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("ready"));
    assert_eq!(result["process_still_running"], true, "{result}");
    let session_id = result["session_id"].as_str().expect("session id");
    let _ = crate::tools::session::kill_session_async(
        &ctx.sessions,
        &json!({"session_id": session_id, "wait_ms": 5000}),
    )
    .await;
}

#[test]
#[serial_test::serial(process_runtime)]
fn permission_grant_resumes_original_operation() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");

    #[cfg(windows)]
    let shell = "powershell";
    #[cfg(windows)]
    let command = "Write-Output permission-resumed";
    #[cfg(unix)]
    let shell = "sh";
    #[cfg(unix)]
    let command = "printf permission-resumed";

    let blocked = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": command,
            "shell": shell,
            "timeout_ms": 5000,
            "yield_time_ms": 5000
        }),
    );
    assert_eq!(blocked["ok"], false, "{blocked}");
    let resume_id = blocked["error"]["details"]["permission_request"]["resume_id"]
        .as_str()
        .expect("resume id");
    let denied = call_tool(
        &ctx,
        "request_permissions",
        &json!({"resume_id": resume_id, "approve": true, "scope": "once"}),
    );
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(
        denied["error"]["code"], "PERMISSION_NOT_APPROVED",
        "{denied}"
    );
    let resumed = call_tool(
        &ctx,
        "request_permissions",
        &json!({
            "resume_id": resume_id,
            "approve": true,
            "confirm": true,
            "scope": "once"
        }),
    );
    assert_eq!(resumed["ok"], true, "{resumed}");
    assert_eq!(resumed["resumed"], true, "{resumed}");
    assert_eq!(resumed["command_ok"], true, "{resumed}");
    assert!(resumed["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("permission-resumed"));
}

#[test]
#[serial_test::serial(process_runtime)]
fn output_modes_none_and_summary_reduce_payload() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Write-Output repeated; Write-Output repeated; Write-Output final\"";
    #[cfg(unix)]
    let command = "sh -c \"printf 'repeated\\nrepeated\\nfinal\\n'\"";

    let none = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": command,
            "timeout_ms": 5000,
            "yield_time_ms": 5000,
            "output_mode": "none"
        }),
    );
    assert_eq!(none["command_ok"], true, "{none}");
    assert_eq!(none["stdout"], "", "{none}");
    assert!(none["output_refs"]["stdout"].is_string(), "{none}");

    let summary = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "cmd": command,
            "timeout_ms": 5000,
            "yield_time_ms": 5000,
            "output_mode": "summary",
            "tail_lines": 10
        }),
    );
    let stdout = summary["stdout"].as_str().unwrap_or_default();
    assert_eq!(stdout.matches("repeated").count(), 1, "{summary}");
    assert!(stdout.contains("final"), "{summary}");
}

#[cfg(unix)]
#[test]
fn unix_workspace_scripts_preserve_space_paths_and_arguments() {
    use std::os::unix::fs::PermissionsExt;

    let parent = tempdir().expect("workspace parent");
    let workspace = parent.path().join("Life Brain 中文");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let harness = tempdir().expect("harness");
    let script_name = "run tooling";
    let script_path = workspace.join(script_name);
    std::fs::write(
        &script_path,
        "#!/bin/sh\nprintf 'tooling-space-path-ok\\n'\nprintf 'argument=[%s]\\n' \"$1\"\n",
    )
    .expect("shell script");
    let mut permissions = std::fs::metadata(&script_path)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script_path, permissions).expect("script executable");

    let ctx = ToolContext::for_test(workspace, harness.path().to_path_buf()).expect("context");
    let command = format!(r#""{script_name}" "argument with spaces""#);
    let output = call_tool(
        &ctx,
        "exec_command",
        &json!({ "cmd": command, "timeout_ms": 10_000, "yield_time_ms": 10_000 }),
    );
    assert_eq!(output["command_ok"], true, "{output}");
    let stdout = output["stdout"].as_str().unwrap_or_default();
    assert!(stdout.contains("tooling-space-path-ok"), "{output}");
    assert!(
        stdout.contains("argument=[argument with spaces]"),
        "{output}"
    );
}
