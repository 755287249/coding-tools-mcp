use crate::secret::SecretStore;
use crate::tools::context::ToolContext;
use crate::tools::dispatch::call_tool;
use serde_json::json;
use tempfile::tempdir;

#[test]
#[serial_test::serial(process_runtime)]
fn exec_command_resolves_workspace_secret_references_without_exposing_plaintext() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let secret_name = format!("exec_secret_test_{}", std::process::id());
    let secret_value = "rust-local-secret-value-12345";
    SecretStore::set(&ctx.profile_id, &secret_name, secret_value).expect("store test secret");

    #[cfg(windows)]
    let (shell, script) = (
        "powershell",
        "$inputText = [Console]::In.ReadToEnd(); if ($env:CTMCP_TEST_SECRET -ceq $inputText) { exit 0 } else { exit 7 }",
    );
    #[cfg(unix)]
    let (shell, script) = (
        "sh",
        "input=$(cat); [ \"$CTMCP_TEST_SECRET\" = \"$input\" ]",
    );

    let output = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "script": script,
            "shell": shell,
            "secret_env": { "CTMCP_TEST_SECRET": secret_name.clone() },
            "stdin_secret": secret_name,
            "confirm": true,
            "timeout_ms": 10_000,
            "yield_time_ms": 10_000,
            "output_mode": "all"
        }),
    );

    SecretStore::set(
        &ctx.profile_id,
        &format!("exec_secret_test_{}", std::process::id()),
        "",
    )
    .expect("clear test secret");
    assert_eq!(output["command_ok"], true, "{output}");
    assert_eq!(output["exit_code"], 0, "{output}");
    assert!(
        !serde_json::to_string(&output)
            .expect("serialize output")
            .contains(secret_value),
        "secret leaked into result: {output}"
    );
}

#[test]
fn exec_command_reports_missing_workspace_secret_reference() {
    let workspace = tempdir().expect("workspace");
    let harness = tempdir().expect("harness");
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("context");
    let missing_name = format!("missing_exec_secret_{}", std::process::id());
    SecretStore::set(&ctx.profile_id, &missing_name, "").expect("clear missing test secret");

    let output = call_tool(
        &ctx,
        "exec_command",
        &json!({
            "program": "git",
            "args": ["--version"],
            "secret_env": { "CTMCP_TEST_SECRET": missing_name }
        }),
    );

    assert_eq!(output["ok"], false, "{output}");
    assert_eq!(output["error"]["code"], "SECRET_NOT_FOUND", "{output}");
}
