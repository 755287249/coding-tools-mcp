use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::tools::workspace::WorkspaceError;

use super::runner::{prepared_command, CommandIoMode};
use super::spec::ExecSpec;

#[derive(Clone, Debug)]
struct TestRunnerDescriptor {
    runner: &'static str,
    help_args: Vec<String>,
    selector_syntax: &'static str,
}

#[derive(Clone, Debug)]
struct TestRunnerCapabilities {
    runner: &'static str,
    probe_ok: bool,
    supports_filter: bool,
    supports_test_name_pattern: bool,
    selector_syntax: &'static str,
}

static CAPABILITY_CACHE: OnceLock<Mutex<HashMap<String, TestRunnerCapabilities>>> = OnceLock::new();

fn capability_cache() -> &'static Mutex<HashMap<String, TestRunnerCapabilities>> {
    CAPABILITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn executable_stem(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let lower = Path::new(&normalized)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&normalized)
        .to_ascii_lowercase();
    lower
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .trim_end_matches(".phar")
        .to_string()
}

fn wrapper_runner<'a>(
    args: &'a [String],
    program: &str,
    candidate: &str,
) -> Option<(Vec<String>, &'a [String])> {
    if executable_stem(program) != "php"
        || args.is_empty()
        || executable_stem(&args[0]) != candidate
    {
        return None;
    }
    Some((vec![args[0].clone()], &args[1..]))
}

fn describe_test_runner(spec: &ExecSpec) -> Option<TestRunnerDescriptor> {
    let program = executable_stem(&spec.program);
    let mut prefix = Vec::<String>::new();
    let mut remaining = spec.args.as_slice();
    let (runner, selector_syntax) = if program == "codecept" {
        ("codeception", "tests/path/Test.php:testMethod")
    } else if program == "phpunit" {
        ("phpunit", "--filter <pattern>")
    } else if let Some((wrapper, rest)) = wrapper_runner(&spec.args, &spec.program, "codecept") {
        prefix = wrapper;
        remaining = rest;
        ("codeception", "tests/path/Test.php:testMethod")
    } else if let Some((wrapper, rest)) = wrapper_runner(&spec.args, &spec.program, "phpunit") {
        prefix = wrapper;
        remaining = rest;
        ("phpunit", "--filter <pattern>")
    } else if program == "pytest" || program == "py.test" {
        ("pytest", "-k <expression> or path::test_name")
    } else if matches!(program.as_str(), "python" | "python3")
        && spec.args.get(0).is_some_and(|arg| arg == "-m")
        && spec.args.get(1).is_some_and(|arg| arg == "pytest")
    {
        prefix = vec!["-m".into(), "pytest".into()];
        remaining = &spec.args[2..];
        ("pytest", "-k <expression> or path::test_name")
    } else if program == "cargo" && spec.args.first().is_some_and(|arg| arg == "test") {
        prefix = vec!["test".into()];
        remaining = &spec.args[1..];
        ("cargo", "cargo test <test-name>")
    } else if matches!(program.as_str(), "node" | "nodejs")
        && spec.args.iter().any(|arg| arg == "--test")
    {
        ("node", "--test-name-pattern <pattern>")
    } else {
        return None;
    };
    let mut help_args = prefix;
    if runner == "codeception" && remaining.iter().any(|arg| arg == "run") {
        help_args.push("run".into());
    }
    help_args.push("--help".into());
    Some(TestRunnerDescriptor {
        runner,
        help_args,
        selector_syntax,
    })
}

fn requested_selector(args: &[String]) -> Option<&str> {
    args.iter().map(String::as_str).find(|arg| {
        *arg == "--filter"
            || arg.starts_with("--filter=")
            || *arg == "--test-name-pattern"
            || arg.starts_with("--test-name-pattern=")
    })
}

fn cargo_value_option(value: &str) -> bool {
    matches!(
        value,
        "--manifest-path"
            | "--package"
            | "-p"
            | "--exclude"
            | "--features"
            | "--target"
            | "--target-dir"
            | "--jobs"
            | "-j"
            | "--profile"
            | "--color"
            | "--message-format"
            | "--config"
            | "--test"
            | "--bin"
            | "--example"
            | "--bench"
    )
}

fn cargo_target_flag(value: &str) -> bool {
    matches!(
        value,
        "--lib" | "--bins" | "--tests" | "--benches" | "--examples" | "--all-targets" | "--doc"
    )
}

fn cargo_target_value_option(value: &str) -> bool {
    matches!(value, "--test" | "--bin" | "--example" | "--bench")
}

fn cargo_inline_target_value_option(value: &str) -> bool {
    ["--test=", "--bin=", "--example=", "--bench="]
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

fn cargo_filter_index(args: &[String]) -> Option<usize> {
    let end = args
        .iter()
        .position(|value| value == "--")
        .unwrap_or(args.len());
    let mut index = 1usize;
    while index < end {
        let value = &args[index];
        if value.starts_with('-') {
            index += if cargo_value_option(value) && !value.contains('=') {
                2
            } else {
                1
            };
            continue;
        }
        return Some(index);
    }
    None
}

fn cargo_has_target_scope(args: &[String]) -> bool {
    let end = args
        .iter()
        .position(|value| value == "--")
        .unwrap_or(args.len());
    let mut index = 1usize;
    while index < end {
        let value = &args[index];
        if cargo_target_flag(value)
            || cargo_target_value_option(value)
            || cargo_inline_target_value_option(value)
        {
            return true;
        }
        index += if cargo_value_option(value) && !value.contains('=') {
            2
        } else {
            1
        };
    }
    false
}

fn strip_cargo_target_scope(args: &[String]) -> Vec<String> {
    let separator = args.iter().position(|value| value == "--");
    let end = separator.unwrap_or(args.len());
    let mut result = Vec::new();
    let mut index = 0usize;
    while index < end {
        let value = &args[index];
        if cargo_target_flag(value) || cargo_inline_target_value_option(value) {
            index += 1;
            continue;
        }
        if cargo_target_value_option(value) {
            index = (index + 2).min(end);
            continue;
        }
        result.push(value.clone());
        index += 1;
    }
    if let Some(separator) = separator {
        result.extend(args[separator..].iter().cloned());
    }
    result
}

fn node_test_value_option(value: &str) -> bool {
    matches!(
        value,
        "--test-concurrency"
            | "--test-name-pattern"
            | "--test-reporter"
            | "--test-reporter-destination"
            | "--test-shard"
            | "--test-timeout"
            | "--test-isolation"
    )
}

fn node_test_file_indexes(args: &[String]) -> Vec<usize> {
    let Some(test_index) = args.iter().position(|value| value == "--test") else {
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut consumes_value = false;
    for (index, value) in args.iter().enumerate().skip(test_index + 1) {
        if consumes_value {
            consumes_value = false;
            continue;
        }
        if node_test_value_option(value) {
            consumes_value = true;
            continue;
        }
        if value.starts_with('-') {
            continue;
        }
        files.push(index);
    }
    files
}

fn remove_option_with_value(args: &[String], option: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        let value = &args[index];
        if value == option {
            index = (index + 2).min(args.len());
            continue;
        }
        if value.starts_with(&format!("{option}=")) {
            index += 1;
            continue;
        }
        result.push(value.clone());
        index += 1;
    }
    result
}

fn strip_node_test_files(args: &[String]) -> Vec<String> {
    let file_indexes = node_test_file_indexes(args)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    args.iter()
        .enumerate()
        .filter(|(index, _)| !file_indexes.contains(index))
        .map(|(_, value)| value.clone())
        .collect()
}

fn workflow_value(
    spec: &ExecSpec,
    runner: &str,
    current_stage: &str,
    next: Option<(&str, Vec<String>)>,
    workdir: Option<&str>,
) -> Value {
    let (next_stage, next_actions) = if let Some((stage, args)) = next {
        let mut arguments = json!({
            "program": spec.program,
            "args": args
        });
        if let (Some(workdir), Some(object)) = (workdir, arguments.as_object_mut()) {
            object.insert("workdir".into(), json!(workdir));
        }
        (
            json!(stage),
            json!([{
                "action": "run_next_test_stage",
                "action_id": format!("test-stage-{stage}"),
                "tool": "exec_command",
                "stage": stage,
                "required_arguments": [],
                "arguments": arguments,
                "reason": format!("test_{current_stage}_passed")
            }]),
        )
    } else {
        (Value::Null, json!([]))
    };
    json!({
        "schema_version": 1,
        "runner": runner,
        "stage_sequence": ["preflight", "focused", "affected", "full"],
        "capability_preflight": "automatic",
        "scope_source": "command",
        "current_stage": current_stage,
        "next_stage": next_stage,
        "advance_condition": "command_ok=true",
        "full_regression_deferred": current_stage != "full",
        "next_actions": next_actions
    })
}

pub(super) fn test_workflow(spec: &ExecSpec, workdir: Option<&str>) -> Option<Value> {
    let descriptor = describe_test_runner(spec)?;
    match descriptor.runner {
        "cargo" => {
            let filter_index = cargo_filter_index(&spec.args);
            let target_scoped = cargo_has_target_scope(&spec.args);
            if let Some(filter_index) = filter_index {
                let affected_args = spec
                    .args
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != filter_index)
                    .map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                Some(workflow_value(
                    spec,
                    "cargo",
                    "focused",
                    Some((
                        if target_scoped { "affected" } else { "full" },
                        affected_args,
                    )),
                    workdir,
                ))
            } else if target_scoped {
                Some(workflow_value(
                    spec,
                    "cargo",
                    "affected",
                    Some(("full", strip_cargo_target_scope(&spec.args))),
                    workdir,
                ))
            } else {
                Some(workflow_value(spec, "cargo", "full", None, workdir))
            }
        }
        "node" => {
            let focused = requested_selector(&spec.args)
                .is_some_and(|selector| selector.starts_with("--test-name-pattern"));
            if focused {
                let affected_args = remove_option_with_value(&spec.args, "--test-name-pattern");
                let next_stage = if node_test_file_indexes(&affected_args).is_empty() {
                    "full"
                } else {
                    "affected"
                };
                Some(workflow_value(
                    spec,
                    "node",
                    "focused",
                    Some((next_stage, affected_args)),
                    workdir,
                ))
            } else if !node_test_file_indexes(&spec.args).is_empty() {
                Some(workflow_value(
                    spec,
                    "node",
                    "affected",
                    Some(("full", strip_node_test_files(&spec.args))),
                    workdir,
                ))
            } else {
                Some(workflow_value(spec, "node", "full", None, workdir))
            }
        }
        _ => None,
    }
}

fn capabilities_from_help(
    descriptor: &TestRunnerDescriptor,
    output: &str,
    probe_ok: bool,
) -> TestRunnerCapabilities {
    TestRunnerCapabilities {
        runner: descriptor.runner,
        probe_ok,
        supports_filter: output.contains("--filter"),
        supports_test_name_pattern: output.contains("--test-name-pattern"),
        selector_syntax: descriptor.selector_syntax,
    }
}

fn capability_key(spec: &ExecSpec, descriptor: &TestRunnerDescriptor, cwd: &Path) -> String {
    format!(
        "{}\0{}\0{}",
        cwd.display(),
        spec.program,
        descriptor.help_args.join("\0")
    )
}

async fn probe_capabilities(
    spec: &ExecSpec,
    descriptor: &TestRunnerDescriptor,
    cwd: &Path,
) -> TestRunnerCapabilities {
    let mut help_spec = spec.clone();
    help_spec.args = descriptor.help_args.clone();
    let mut command = prepared_command(&help_spec, cwd, CommandIoMode::PostCheck);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output()).await;
    match output {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}\n{stderr}");
            capabilities_from_help(
                descriptor,
                &combined,
                output.status.success() || !combined.trim().is_empty(),
            )
        }
        _ => capabilities_from_help(descriptor, "", false),
    }
}

pub(super) async fn preflight_test_runner_capabilities(
    spec: &ExecSpec,
    cwd: &Path,
) -> Result<Option<Value>, WorkspaceError> {
    let Some(descriptor) = describe_test_runner(spec) else {
        return Ok(None);
    };
    let Some(selector) = requested_selector(&spec.args) else {
        return Ok(None);
    };
    let key = capability_key(spec, &descriptor, cwd);
    let cached = capability_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned());
    let (capabilities, cache_status) = if let Some(cached) = cached {
        (cached, "hit")
    } else {
        let probed = probe_capabilities(spec, &descriptor, cwd).await;
        if let Ok(mut cache) = capability_cache().lock() {
            cache.insert(key, probed.clone());
        }
        (probed, "miss")
    };
    let unsupported = capabilities.probe_ok
        && ((selector.starts_with("--filter") && !capabilities.supports_filter)
            || (selector.starts_with("--test-name-pattern")
                && !capabilities.supports_test_name_pattern));
    let metadata = json!({
        "runner": capabilities.runner,
        "capability_cache": cache_status,
        "probe_ok": capabilities.probe_ok,
        "selector_syntax": capabilities.selector_syntax,
        "supports_filter": capabilities.supports_filter,
        "supports_test_name_pattern": capabilities.supports_test_name_pattern
    });
    if !unsupported {
        return Ok(Some(metadata));
    }
    Err(WorkspaceError::ToolDetails {
        code: "TEST_RUNNER_CAPABILITY_MISMATCH",
        message: format!(
            "{} does not support requested selector {selector}",
            capabilities.runner
        ),
        category: "validation",
        retryable: false,
        details: json!({
            "runner": capabilities.runner,
            "capability_cache": cache_status,
            "probe_ok": capabilities.probe_ok,
            "selector_syntax": capabilities.selector_syntax,
            "supports_filter": capabilities.supports_filter,
            "supports_test_name_pattern": capabilities.supports_test_name_pattern,
            "unsupported_argument": selector,
            "stage": "test_runner_preflight",
            "suggestion": format!("Use {} instead of {selector}.", capabilities.selector_syntax),
            "recovery_actions": [{
                "action": "use_supported_test_selector",
                "tool": "exec_command",
                "required_arguments": ["program", "args"],
                "reason": "runner_selector_unsupported"
            }]
        }),
    })
}

#[cfg(test)]
pub(super) fn clear_capability_cache_for_test() {
    if let Ok(mut cache) = capability_cache().lock() {
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codeception_spec() -> ExecSpec {
        ExecSpec {
            display: "php vendor/bin/codecept run unit --filter Example".into(),
            program: "PHP.EXE".into(),
            args: vec![
                "vendor/bin/codecept".into(),
                "run".into(),
                "unit".into(),
                "--filter".into(),
                "Example".into(),
            ],
            shell: "none".into(),
            env: Vec::new(),
            remove_env: Vec::new(),
        }
    }

    #[test]
    fn codeception_wrapper_builds_run_help_probe_and_selector_hint() {
        let descriptor = describe_test_runner(&codeception_spec()).expect("runner");
        assert_eq!(descriptor.runner, "codeception");
        assert_eq!(
            descriptor.help_args,
            vec!["vendor/bin/codecept", "run", "--help"]
        );
        assert_eq!(descriptor.selector_syntax, "tests/path/Test.php:testMethod");
    }

    #[test]
    fn cached_unsupported_codeception_filter_is_rejected_without_probe() {
        clear_capability_cache_for_test();
        let spec = codeception_spec();
        let descriptor = describe_test_runner(&spec).expect("runner");
        let cwd = Path::new("workspace");
        let key = capability_key(&spec, &descriptor, cwd);
        capability_cache().lock().expect("cache").insert(
            key,
            TestRunnerCapabilities {
                runner: "codeception",
                probe_ok: true,
                supports_filter: false,
                supports_test_name_pattern: false,
                selector_syntax: "tests/path/Test.php:testMethod",
            },
        );
        let error = crate::task_runtime::block_on(preflight_test_runner_capabilities(&spec, cwd))
            .expect_err("unsupported cached selector must fail");
        let value = error.to_error_value();
        assert_eq!(value["code"], "TEST_RUNNER_CAPABILITY_MISMATCH");
        assert_eq!(value["details"]["capability_cache"], "hit");
        assert_eq!(value["details"]["unsupported_argument"], "--filter");
    }

    #[test]
    fn node_workflow_advances_focused_to_affected_to_full() {
        let focused = ExecSpec {
            display: "node --test --test-name-pattern ambiguous test/editRecovery.test.mjs".into(),
            program: "node".into(),
            args: vec![
                "--import".into(),
                "./test/setup.mjs".into(),
                "--test".into(),
                "--test-name-pattern".into(),
                "ambiguous".into(),
                "test/editRecovery.test.mjs".into(),
            ],
            shell: "none".into(),
            env: Vec::new(),
            remove_env: Vec::new(),
        };
        let workflow = test_workflow(&focused, Some("packages/node-agent")).expect("workflow");
        assert_eq!(workflow["current_stage"], "focused");
        assert_eq!(workflow["next_stage"], "affected");
        assert_eq!(workflow["advance_condition"], "command_ok=true");
        assert_eq!(workflow["next_actions"][0]["required_arguments"], json!([]));
        assert_eq!(
            workflow["next_actions"][0]["arguments"]["args"],
            json!([
                "--import",
                "./test/setup.mjs",
                "--test",
                "test/editRecovery.test.mjs"
            ])
        );
        assert_eq!(
            workflow["next_actions"][0]["arguments"]["workdir"],
            "packages/node-agent"
        );

        let affected = ExecSpec {
            args: vec![
                "--import".into(),
                "./test/setup.mjs".into(),
                "--test".into(),
                "test/editRecovery.test.mjs".into(),
            ],
            ..focused.clone()
        };
        let workflow = test_workflow(&affected, Some("packages/node-agent")).expect("workflow");
        assert_eq!(workflow["current_stage"], "affected");
        assert_eq!(workflow["next_stage"], "full");
        assert_eq!(
            workflow["next_actions"][0]["arguments"]["args"],
            json!(["--import", "./test/setup.mjs", "--test"])
        );

        let full = ExecSpec {
            args: vec!["--test".into()],
            ..focused
        };
        let workflow = test_workflow(&full, None).expect("workflow");
        assert_eq!(workflow["current_stage"], "full");
        assert!(workflow["next_actions"]
            .as_array()
            .is_some_and(Vec::is_empty));
    }

    #[test]
    fn cargo_workflow_preserves_build_scope_while_advancing_stages() {
        let focused = ExecSpec {
            display: "cargo test --manifest-path src-tauri/Cargo.toml --lib specific_test".into(),
            program: "cargo".into(),
            args: vec![
                "test".into(),
                "--manifest-path".into(),
                "src-tauri/Cargo.toml".into(),
                "--lib".into(),
                "specific_test".into(),
            ],
            shell: "none".into(),
            env: Vec::new(),
            remove_env: Vec::new(),
        };
        let workflow = test_workflow(&focused, Some(".")).expect("workflow");
        assert_eq!(workflow["current_stage"], "focused");
        assert_eq!(workflow["next_stage"], "affected");
        assert_eq!(
            workflow["next_actions"][0]["arguments"]["args"],
            json!(["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib"])
        );

        let affected = ExecSpec {
            args: vec![
                "test".into(),
                "--manifest-path".into(),
                "src-tauri/Cargo.toml".into(),
                "--lib".into(),
            ],
            ..focused.clone()
        };
        let workflow = test_workflow(&affected, Some(".")).expect("workflow");
        assert_eq!(workflow["current_stage"], "affected");
        assert_eq!(workflow["next_stage"], "full");
        assert_eq!(
            workflow["next_actions"][0]["arguments"]["args"],
            json!(["test", "--manifest-path", "src-tauri/Cargo.toml"])
        );

        let full = ExecSpec {
            args: vec![
                "test".into(),
                "--manifest-path".into(),
                "src-tauri/Cargo.toml".into(),
            ],
            ..focused
        };
        let workflow = test_workflow(&full, None).expect("workflow");
        assert_eq!(workflow["current_stage"], "full");
        assert!(workflow["next_actions"]
            .as_array()
            .is_some_and(Vec::is_empty));
    }

    #[test]
    fn help_capabilities_detect_supported_selectors() {
        let descriptor = TestRunnerDescriptor {
            runner: "phpunit",
            help_args: vec!["--help".into()],
            selector_syntax: "--filter <pattern>",
        };
        let capabilities = capabilities_from_help(
            &descriptor,
            "Options:\n  --filter <pattern>\n  --test-name-pattern <pattern>",
            true,
        );
        assert!(capabilities.supports_filter);
        assert!(capabilities.supports_test_name_pattern);
    }
}
