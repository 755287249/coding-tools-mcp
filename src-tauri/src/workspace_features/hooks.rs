use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::{discover_extensions, runtime, HookDescriptor, RuntimeFeatures};

const MAX_HOOK_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct HookBlocked {
    pub message: String,
    pub hook_key: String,
}

#[derive(Debug, Clone)]
pub struct HookPreResult {
    pub input: Value,
    pub blocked: Option<HookBlocked>,
    pub context: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HookPostResult {
    pub feedback: Vec<String>,
}

struct HookExecution {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn matcher_matches(matcher: Option<&str>, source: &str) -> bool {
    let Some(value) = matcher.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    if value.contains('|') && !value.chars().any(|ch| "()[]{}+*?^$\\".contains(ch)) {
        return value.split('|').any(|item| item.trim() == source);
    }
    if value == source {
        return true;
    }
    Regex::new(value)
        .map(|regex| regex.is_match(source))
        .unwrap_or(false)
}

async fn read_capped<R>(mut reader: R) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut retained = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if retained.len() < MAX_HOOK_OUTPUT_BYTES {
            let remaining = MAX_HOOK_OUTPUT_BYTES - retained.len();
            retained.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    }
    retained
}

async fn run_command_hook(hook: &HookDescriptor, input: &Value, cwd: &str) -> HookExecution {
    let Some(command) = hook
        .command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return HookExecution {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        };
    };
    let mut process = if !hook.args.is_empty() {
        let mut process = Command::new(command);
        process.args(&hook.args);
        process
    } else if cfg!(windows) {
        let mut process =
            Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()));
        process.args(["/d", "/s", "/c", command]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };
    process
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            return HookExecution {
                code: Some(1),
                stdout: String::new(),
                stderr: error.to_string(),
                timed_out: false,
            };
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let mut encoded = serde_json::to_vec(input).unwrap_or_default();
        encoded.push(b'\n');
        let _ = stdin.write_all(&encoded).await;
        let _ = stdin.shutdown().await;
    }
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(read_capped(stdout)));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_capped(stderr)));
    let timeout = Duration::from_millis(hook.timeout_ms.clamp(100, 120_000));
    let (code, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(error)) => {
            return HookExecution {
                code: Some(1),
                stdout: String::new(),
                stderr: error.to_string(),
                timed_out: false,
            };
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (None, true)
        }
    };
    let stdout = match stdout_task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    HookExecution {
        code,
        stdout: String::from_utf8_lossy(&stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
        timed_out,
    }
}

async fn run_http_hook(hook: &HookDescriptor, input: &Value) -> HookExecution {
    let Some(url) = hook.url.as_deref() else {
        return HookExecution {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        };
    };
    let timeout = Duration::from_millis(hook.timeout_ms.clamp(100, 120_000));
    let request = async {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .post(url)
            .header("content-type", "application/json")
            .json(input)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        let stdout = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_HOOK_OUTPUT_BYTES)])
            .trim()
            .to_string();
        Ok::<_, String>(HookExecution {
            code: Some(if status.is_success() {
                0
            } else {
                status.as_u16() as i32
            }),
            stdout,
            stderr: String::new(),
            timed_out: false,
        })
    };
    match tokio::time::timeout(timeout, request).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => HookExecution {
            code: Some(1),
            stdout: String::new(),
            stderr: error,
            timed_out: false,
        },
        Err(_) => HookExecution {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        },
    }
}

async fn run_hook(hook: &HookDescriptor, input: &Value, cwd: &str) -> HookExecution {
    match hook.handler_type.as_str() {
        "command" => run_command_hook(hook, input, cwd).await,
        "http" => run_http_hook(hook, input).await,
        _ => HookExecution {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        },
    }
}

fn record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn parse_output(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        None
    } else {
        serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
    }
}

fn specific(output: Option<&Value>) -> Option<&Map<String, Value>> {
    record(output.and_then(|value| value.get("hookSpecificOutput")))
}

fn text_field<'a>(map: Option<&'a Map<String, Value>>, fields: &[&str]) -> Option<&'a str> {
    let map = map?;
    fields
        .iter()
        .find_map(|field| map.get(*field).and_then(Value::as_str))
}

fn block_reason(output: Option<&Value>, stderr: &str) -> Option<String> {
    let specific = specific(output);
    let root = output.and_then(Value::as_object);
    let decision = text_field(specific, &["permissionDecision"])
        .or_else(|| text_field(root, &["decision"]))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if decision == "deny" || decision == "block" {
        return Some(
            text_field(specific, &["permissionDecisionReason"])
                .or_else(|| text_field(root, &["reason", "message"]))
                .filter(|value| !value.is_empty())
                .unwrap_or(if stderr.is_empty() {
                    "Blocked by Hook"
                } else {
                    stderr
                })
                .to_string(),
        );
    }
    if root
        .and_then(|map| map.get("continue"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Some(
            text_field(root, &["stopReason", "reason"])
                .unwrap_or("Blocked by Hook")
                .to_string(),
        );
    }
    None
}

async fn enabled_hooks(
    runtime: &RuntimeFeatures,
    event: &str,
    folder_id: Option<&str>,
) -> Vec<HookDescriptor> {
    let config = runtime.config();
    if !config.extensions.hooks.active {
        return Vec::new();
    }
    let selected = config
        .extensions
        .hooks
        .enabled
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    discover_extensions(&runtime.folders)
        .await
        .hooks
        .into_iter()
        .filter(|hook| {
            hook.event == event
                && hook.supported
                && hook.source_enabled
                && selected.contains(&hook.key)
                && hook
                    .folder_id
                    .as_deref()
                    .map(|candidate| Some(candidate) == folder_id)
                    .unwrap_or(true)
        })
        .collect()
}

async fn run_session_hooks(
    runtime: &RuntimeFeatures,
    event: &str,
    folder_id: &str,
    cwd: &str,
    session_id: &str,
    source: &str,
) -> HookPostResult {
    let hooks = enabled_hooks(runtime, event, Some(folder_id)).await;
    let mut feedback = Vec::new();
    for hook in hooks {
        if !matcher_matches(hook.matcher.as_deref(), source) {
            continue;
        }
        let payload = json!({
            "session_id": session_id,
            "cwd": cwd,
            "hook_event_name": event,
            "source": source
        });
        let result = run_hook(&hook, &payload, cwd).await;
        let output = parse_output(&result.stdout);
        if result.timed_out {
            feedback.push("Hook timed out.".into());
            continue;
        }
        let additional = text_field(specific(output.as_ref()), &["additionalContext"])
            .or_else(|| {
                text_field(
                    output.as_ref().and_then(Value::as_object),
                    &["additionalContext", "message"],
                )
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(additional) = additional {
            feedback.push(additional);
        } else if result.code != Some(0) && !result.stderr.is_empty() {
            feedback.push(result.stderr);
        }
    }
    HookPostResult { feedback }
}

async fn ensure_session_started(
    runtime: &RuntimeFeatures,
    folder_id: &str,
    cwd: &str,
    session_id: &str,
) -> Vec<String> {
    let should_start = {
        let mut sessions = runtime.sessions.lock().await;
        let key = (session_id.to_string(), folder_id.to_string());
        if sessions.contains_key(&key) {
            false
        } else {
            sessions.insert(key, cwd.to_string());
            true
        }
    };
    if should_start {
        run_session_hooks(
            runtime,
            "SessionStart",
            folder_id,
            cwd,
            session_id,
            "startup",
        )
        .await
        .feedback
    } else {
        Vec::new()
    }
}

pub async fn run_pre_tool_hooks(
    workspace_id: &str,
    folder_id: Option<&str>,
    cwd: &str,
    session_id: &str,
    tool_name: &str,
    input: Value,
) -> HookPreResult {
    let Some(runtime) = runtime(workspace_id) else {
        return HookPreResult {
            input,
            blocked: None,
            context: Vec::new(),
        };
    };
    let mut context = if let Some(folder_id) = folder_id {
        ensure_session_started(&runtime, folder_id, cwd, session_id).await
    } else {
        Vec::new()
    };
    let hooks = enabled_hooks(&runtime, "PreToolUse", folder_id).await;
    let mut current = input;
    for hook in hooks {
        if !matcher_matches(hook.matcher.as_deref(), tool_name) {
            continue;
        }
        let payload = json!({
            "session_id": session_id,
            "cwd": cwd,
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": current
        });
        let result = run_hook(&hook, &payload, cwd).await;
        let output = parse_output(&result.stdout);
        if result.timed_out {
            return HookPreResult {
                input: current,
                blocked: Some(HookBlocked {
                    message: "Hook timed out.".into(),
                    hook_key: hook.key,
                }),
                context,
            };
        }
        let reason = block_reason(output.as_ref(), &result.stderr);
        if result.code == Some(2) || reason.is_some() {
            return HookPreResult {
                input: current,
                blocked: Some(HookBlocked {
                    message: reason.unwrap_or_else(|| {
                        if result.stderr.is_empty() {
                            "Blocked by Hook.".into()
                        } else {
                            result.stderr
                        }
                    }),
                    hook_key: hook.key,
                }),
                context,
            };
        }
        let updated = specific(output.as_ref())
            .and_then(|map| map.get("updatedInput"))
            .or_else(|| output.as_ref().and_then(|value| value.get("updatedInput")))
            .filter(|value| value.is_object())
            .cloned();
        if let Some(updated) = updated {
            current = updated;
        }
        let additional = text_field(specific(output.as_ref()), &["additionalContext"])
            .or_else(|| {
                text_field(
                    output.as_ref().and_then(Value::as_object),
                    &["additionalContext"],
                )
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(additional) = additional {
            context.push(additional);
        }
    }
    HookPreResult {
        input: current,
        blocked: None,
        context,
    }
}

pub async fn run_post_tool_hooks(
    workspace_id: &str,
    folder_id: Option<&str>,
    cwd: &str,
    session_id: &str,
    tool_name: &str,
    input: &Value,
    response: &Value,
    success: bool,
) -> HookPostResult {
    let Some(runtime) = runtime(workspace_id) else {
        return HookPostResult::default();
    };
    let event = if success {
        "PostToolUse"
    } else {
        "PostToolUseFailure"
    };
    let hooks = enabled_hooks(&runtime, event, folder_id).await;
    let mut feedback = Vec::new();
    for hook in hooks {
        if !matcher_matches(hook.matcher.as_deref(), tool_name) {
            continue;
        }
        let payload = json!({
            "session_id": session_id,
            "cwd": cwd,
            "hook_event_name": event,
            "tool_name": tool_name,
            "tool_input": input,
            "tool_response": response
        });
        let result = run_hook(&hook, &payload, cwd).await;
        let output = parse_output(&result.stdout);
        let additional = text_field(specific(output.as_ref()), &["additionalContext"])
            .or_else(|| {
                text_field(
                    output.as_ref().and_then(Value::as_object),
                    &["additionalContext", "message"],
                )
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| (!result.stderr.is_empty()).then_some(result.stderr));
        if let Some(additional) = additional {
            feedback.push(additional);
        }
    }
    HookPostResult { feedback }
}

pub async fn run_session_end_hooks(runtime: &Arc<RuntimeFeatures>, source: &str) {
    let sessions = {
        let mut sessions = runtime.sessions.lock().await;
        sessions.drain().collect::<Vec<_>>()
    };
    for ((session_id, folder_id), cwd) in sessions {
        let _ =
            run_session_hooks(runtime, "SessionEnd", &folder_id, &cwd, &session_id, source).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_start_state_is_scoped_by_session_and_folder() {
        let runtime = RuntimeFeatures {
            isolated: std::sync::atomic::AtomicBool::new(false),
            folders: Vec::new(),
            config: std::sync::RwLock::new(crate::workspace_features::FeatureConfig {
                skills: crate::workspace::canonical::CanonicalToggle::default(),
                extensions: crate::workspace::canonical::CanonicalExtensions::default(),
            }),
            connections: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            sessions: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        };

        let _ = ensure_session_started(&runtime, "folder-a", "C:/a", "session-1").await;
        let _ = ensure_session_started(&runtime, "folder-b", "C:/b", "session-1").await;
        let _ = ensure_session_started(&runtime, "folder-a", "C:/a", "session-1").await;

        let sessions = runtime.sessions.lock().await;
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions
                .get(&("session-1".into(), "folder-a".into()))
                .map(String::as_str),
            Some("C:/a")
        );
        assert_eq!(
            sessions
                .get(&("session-1".into(), "folder-b".into()))
                .map(String::as_str),
            Some("C:/b")
        );
    }

    #[test]
    fn matcher_semantics_match_node_agent() {
        assert!(matcher_matches(None, "read_file"));
        assert!(matcher_matches(Some("read_file|write_file"), "read_file"));
        assert!(matcher_matches(Some("^read_.*$"), "read_file"));
        assert!(!matcher_matches(Some("write_file"), "read_file"));
    }
}
