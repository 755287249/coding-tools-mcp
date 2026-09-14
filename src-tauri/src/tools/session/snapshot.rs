use std::sync::atomic::Ordering;

use serde_json::{json, Value};

use super::output::{
    complete_output_boundary, decode_output_event, decode_process_output_with_encoding,
    summarize_stream, truncate_tail, EventDetail, OutputEvent, ProcessOutputEncoding,
    ProcessOutputSnapshot,
};
use super::{ExecSession, OutputMode, OutputOptions};

struct EventBatch {
    events: Vec<OutputEvent>,
    next_cursor: u64,
    cursor_expired: bool,
    has_more: bool,
}

fn event_batch_after(session: &ExecSession, cursor: u64, max_output_bytes: usize) -> EventBatch {
    let state = session.events.lock().expect("events lock");
    let latest_cursor = state.next_sequence.saturating_sub(1);
    let oldest = state.events.front().map(|event| event.sequence);
    let cursor_expired = oldest.is_some_and(|oldest| cursor.saturating_add(1) < oldest);
    let effective_cursor = if cursor_expired {
        oldest.unwrap_or(1).saturating_sub(1)
    } else {
        cursor
    };
    let mut bytes = 0usize;
    let mut events = Vec::new();
    for event in state
        .events
        .iter()
        .filter(|event| event.sequence > effective_cursor)
    {
        if !events.is_empty() && bytes.saturating_add(event.data.len()) > max_output_bytes {
            break;
        }
        bytes = bytes.saturating_add(event.data.len());
        events.push(event.clone());
    }
    let next_cursor = events
        .last()
        .map(|event| event.sequence)
        .unwrap_or(effective_cursor.min(latest_cursor));
    EventBatch {
        has_more: next_cursor < latest_cursor,
        events,
        next_cursor,
        cursor_expired,
    }
}

pub(super) fn capture_stream_snapshot(
    session: &ExecSession,
    stream: &str,
) -> ProcessOutputSnapshot {
    if session.has_sensitive_output() {
        let data = b"[REDACTED]".to_vec();
        return ProcessOutputSnapshot {
            total_bytes: data.len(),
            data,
            encoding: ProcessOutputEncoding::Unknown,
        };
    }
    match stream {
        "stderr" => session.stderr.lock().expect("stderr lock").snapshot(),
        _ => session.stdout.lock().expect("stdout lock").snapshot(),
    }
}

fn stream_encoding(session: &ExecSession, stream: &str) -> ProcessOutputEncoding {
    match stream {
        "stderr" => session.stderr.lock().expect("stderr lock").encoding,
        _ => session.stdout.lock().expect("stdout lock").encoding,
    }
}

pub(super) fn read_retained_stream_bytes(session: &ExecSession, stream: &str) -> (Vec<u8>, usize) {
    let snapshot = capture_stream_snapshot(session, stream);
    (snapshot.data, snapshot.total_bytes)
}

pub(super) fn build_summary(session: &ExecSession) -> Value {
    build_snapshot_with_options(
        session,
        OutputOptions {
            mode: OutputMode::None,
            event_detail: EventDetail::Full,
            cursor: session.latest_cursor(),
            max_output_bytes: 1,
            tail_lines: 1,
        },
    )
}

pub(super) fn build_snapshot(session: &ExecSession, max_output_bytes: usize) -> Value {
    build_snapshot_with_options(session, OutputOptions::tail(max_output_bytes))
}

pub(super) fn build_snapshot_with_options(session: &ExecSession, options: OutputOptions) -> Value {
    // Delta and metadata-only snapshots read from the event queue and do not
    // need to clone the retained stdout/stderr buffers (up to 2 MiB total).
    let retained_streams = matches!(
        options.mode,
        OutputMode::Summary | OutputMode::Tail | OutputMode::All
    )
    .then(|| {
        (
            capture_stream_snapshot(session, "stdout"),
            capture_stream_snapshot(session, "stderr"),
        )
    });
    let exit_code = *session.exit_code.lock().expect("exit_code lock");
    let termination_reason = session
        .termination_reason
        .lock()
        .expect("termination lock")
        .clone();
    let reason = termination_reason.as_deref().unwrap_or("running");
    let post_checks = session
        .post_check_result
        .lock()
        .expect("post check lock")
        .clone();
    let verification_ok = if session.post_checks_pending() {
        None
    } else {
        post_checks
            .as_ref()
            .and_then(|value| value.get("ok"))
            .and_then(Value::as_bool)
            .or(Some(true))
    };
    let execution_ok = if session.has_exited() {
        Some(reason == "exited" && exit_code == Some(0))
    } else {
        None
    };
    let command_ok = match (execution_ok, verification_ok) {
        (Some(execution), Some(verification)) => Some(execution && verification),
        _ => None,
    };
    let failure_origin = if command_ok != Some(false) {
        None
    } else if execution_ok == Some(true) && verification_ok == Some(false) {
        Some("post_check")
    } else if reason == "exited" {
        Some("child_process")
    } else {
        Some("process_lifecycle")
    };
    let diagnostic_summary = match failure_origin {
        Some("post_check") => Some(
            "Child process completed, but post-check verification failed. MCP policy did not block execution."
                .to_string(),
        ),
        Some("child_process") => Some(format!(
            "Child process started and exited with code {}. MCP policy did not block execution.",
            exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string())
        )),
        Some("process_lifecycle") => Some(format!(
            "Child process started but ended because of {reason}. MCP policy did not block execution."
        )),
        _ => None,
    };
    let status = if !session.has_exited() {
        "running"
    } else if session.post_checks_pending() {
        "verifying"
    } else {
        match reason {
            "process_timeout" => "timed_out",
            "killed" => "killed",
            _ => "exited",
        }
    };

    let (
        mut stdout,
        mut stderr,
        stdout_truncated,
        stderr_truncated,
        mut events,
        next_cursor,
        cursor_expired,
        has_more,
    ) = match options.mode {
        OutputMode::Delta => {
            let batch = event_batch_after(session, options.cursor, options.max_output_bytes);
            let stdout = batch
                .events
                .iter()
                .filter(|event| event.stream == "stdout")
                .flat_map(|event| event.data.iter().copied())
                .collect::<Vec<_>>();
            let stderr = batch
                .events
                .iter()
                .filter(|event| event.stream == "stderr")
                .flat_map(|event| event.data.iter().copied())
                .collect::<Vec<_>>();
            let stdout_encoding = stream_encoding(session, "stdout");
            let stderr_encoding = stream_encoding(session, "stderr");
            let events = batch
                .events
                .iter()
                .map(|event| {
                    let encoding = if event.stream == "stderr" {
                        stderr_encoding
                    } else {
                        stdout_encoding
                    };
                    if options.event_detail == EventDetail::Compact {
                        json!({
                            "sequence": event.sequence,
                            "stream": event.stream,
                            "stream_offset": event.stream_offset
                        })
                    } else {
                        json!({
                            "sequence": event.sequence,
                            "stream": event.stream,
                            "stream_offset": event.stream_offset,
                            "decoded_offset": event.stream_offset.saturating_sub(event.prefix.len()),
                            "encoding": encoding.as_str(),
                            "data": decode_output_event(event, encoding)
                        })
                    }
                })
                .collect::<Vec<_>>();
            let stdout_prefix = batch
                .events
                .iter()
                .find(|event| event.stream == "stdout")
                .map(|event| event.prefix.as_slice())
                .unwrap_or_default();
            let stderr_prefix = batch
                .events
                .iter()
                .find(|event| event.stream == "stderr")
                .map(|event| event.prefix.as_slice())
                .unwrap_or_default();
            let mut stdout_bytes = stdout_prefix.to_vec();
            stdout_bytes.extend_from_slice(&stdout);
            stdout_bytes.truncate(complete_output_boundary(&stdout_bytes, stdout_encoding));
            let mut stderr_bytes = stderr_prefix.to_vec();
            stderr_bytes.extend_from_slice(&stderr);
            stderr_bytes.truncate(complete_output_boundary(&stderr_bytes, stderr_encoding));
            (
                decode_process_output_with_encoding(&stdout_bytes, stdout_encoding),
                decode_process_output_with_encoding(&stderr_bytes, stderr_encoding),
                false,
                false,
                events,
                batch.next_cursor,
                batch.cursor_expired,
                batch.has_more,
            )
        }
        OutputMode::None => (
            String::new(),
            String::new(),
            false,
            false,
            Vec::new(),
            session.latest_cursor(),
            false,
            false,
        ),
        OutputMode::Summary => {
            let (stdout_stream, stderr_stream) = retained_streams
                .as_ref()
                .expect("summary snapshots retain streams");
            let stdout = summarize_stream(
                &stdout_stream.data,
                options.max_output_bytes,
                options.tail_lines,
                stdout_stream.encoding,
            );
            let stderr = summarize_stream(
                &stderr_stream.data,
                options.max_output_bytes,
                options.tail_lines,
                stderr_stream.encoding,
            );
            (
                stdout.content,
                stderr.content,
                stdout.truncated,
                stderr.truncated,
                Vec::new(),
                session.latest_cursor(),
                false,
                false,
            )
        }
        OutputMode::Tail | OutputMode::All => {
            let (stdout_stream, stderr_stream) = retained_streams
                .as_ref()
                .expect("tail snapshots retain streams");
            let stdout = truncate_tail(
                &stdout_stream.data,
                options.max_output_bytes,
                stdout_stream.encoding,
            );
            let stderr = truncate_tail(
                &stderr_stream.data,
                options.max_output_bytes,
                stderr_stream.encoding,
            );
            (
                stdout.content,
                stderr.content,
                stdout.truncated,
                stderr.truncated,
                Vec::new(),
                session.latest_cursor(),
                false,
                false,
            )
        }
    };

    let sensitive_output = session.has_sensitive_output();
    let mut redaction_count = 0u64;
    if sensitive_output {
        if !stdout.is_empty() {
            stdout = "[REDACTED]".into();
            redaction_count += 1;
        }
        if !stderr.is_empty() {
            stderr = "[REDACTED]".into();
            redaction_count += 1;
        }
        for event in &mut events {
            if let Some(data) = event.get_mut("data") {
                if !data.as_str().unwrap_or_default().is_empty() {
                    *data = Value::String("[REDACTED]".into());
                    redaction_count += 1;
                }
            }
        }
    }

    let mut payload = json!({
        "session_id": session.session_id,
        "interactive": session.interactive,
        "stdin_open": *session.stdin_open.lock().expect("stdin_open lock"),
        "status": status,
        "termination_reason": reason,
        "recoverable": matches!(reason, "process_timeout" | "killed" | "spawn_failed" | "server_restart" | "detached_timeout"),
        "suggestion": match reason {
            "process_timeout" => "读取保留输出，调整 timeout_ms 后重试",
            "detached_timeout" => "连接失联超过宽限时间；确认没有可恢复 session 后再使用新的 operation_id 重试",
            "killed" => "确认终止原因后重新执行命令",
            "exited" => "检查 process_exit_code、stderr 与 post_checks",
            "crashed" => "检查 stderr 后重试或恢复工作区",
            _ => "使用 wait_command 等待新输出或进程结束",
        },
        "process_exit_code": exit_code,
        "exit_code": exit_code,
        "request_timed_out": false,
        "process_timed_out": reason == "process_timeout",
        "process_still_running": !session.has_exited(),
        "transport_ok": true,
        "execution_ok": execution_ok,
        "verification_ok": verification_ok,
        "command_ok": command_ok,
        "post_checks_pending": session.post_checks_pending(),
        "post_checks": post_checks,
        "output_mode": options.mode.as_str(),
        "event_detail": if options.mode == OutputMode::Delta { options.event_detail.as_str() } else { "none" },
        "cursor": options.cursor,
        "next_cursor": next_cursor,
        "latest_cursor": session.latest_cursor(),
        "cursor_expired": cursor_expired,
        "has_more_output": has_more,
        "events": events,
        "stdout": stdout,
        "stderr": stderr,
        "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated,
        "sensitive_data_redacted": sensitive_output,
        "redaction_count": redaction_count,
        "warnings": if sensitive_output {
            vec!["Sensitive process output was withheld because the command referenced a protected credential source."]
        } else {
            Vec::<&str>::new()
        },
        "elapsed_ms": session.started_at.elapsed().as_millis(),
        "first_output_ms": session
            .first_output_at
            .lock()
            .expect("first output lock")
            .map(|first| first.duration_since(session.started_at).as_millis()),
        "sandbox_phase_durations_ms": {
            "prepare_ms": session.sandbox_prepare_ms,
            "startup_ms": session.sandbox_startup_ms,
            "cleanup_ms": *session
                .sandbox_cleanup_ms
                .lock()
                .expect("sandbox cleanup timing lock")
        },
        "output_refs": {
            "stdout": format!("output://{}/stdout", session.session_id),
            "stderr": format!("output://{}/stderr", session.session_id)
        }
    });
    if let Some(object) = payload.as_object_mut() {
        object.insert("failure_origin".into(), json!(failure_origin));
        object.insert("policy_blocked".into(), Value::Bool(false));
        object.insert("execution_attempted".into(), Value::Bool(true));
        object.insert("process_started".into(), Value::Bool(true));
        object.insert("diagnostic_summary".into(), json!(diagnostic_summary));
        if let Some(contract) = &session.process_timeout {
            object.insert("execution_mode".into(), json!(contract.execution_mode));
            object.insert(
                "requested_process_timeout_ms".into(),
                json!(contract.requested_timeout_ms),
            );
            object.insert(
                "effective_process_timeout_ms".into(),
                json!(contract.effective_timeout_ms),
            );
            object.insert("process_timeout_limit_ms".into(), json!(contract.limit_ms));
            object.insert(
                "process_deadline_ts_ms".into(),
                json!(session.process_deadline_ts_ms),
            );
            let remaining = if session.has_exited() {
                0
            } else {
                session
                    .process_deadline
                    .map(|deadline| {
                        deadline
                            .saturating_duration_since(std::time::Instant::now())
                            .as_millis()
                    })
                    .unwrap_or(0)
            };
            object.insert("process_timeout_remaining_ms".into(), json!(remaining));
            object.insert("timeout_clamped".into(), json!(false));
            object.insert("polling_extends_process_deadline".into(), json!(false));
            object.insert("timeout_scope".into(), json!("process"));
        }
        object.insert("process_id".into(), json!(session.process_id));
        object.insert(
            "process_tree_contained".into(),
            Value::Bool(session.process_tree_contained),
        );
        object.insert("operation_id".into(), json!(session.operation_id));
        object.insert(
            "command_fingerprint".into(),
            json!(session.command_fingerprint),
        );
        object.insert(
            "resource_lock_group".into(),
            json!(session.resource_lock_group),
        );
        object.insert(
            "resource_lock_target".into(),
            json!(session.resource_lock_target),
        );
        object.insert(
            "operation_lock_wait_ms".into(),
            json!(session.operation_lock_wait_ms),
        );
        object.insert(
            "resource_lock_wait_ms".into(),
            json!(session.resource_lock_wait_ms),
        );
        object.insert("deduplicated".into(), Value::Bool(false));
        object.insert("attached_to_session_id".into(), Value::Null);
        object.insert(
            "detached".into(),
            Value::Bool(session.detached_generation.load(Ordering::Acquire) != 0),
        );
        object.insert("started_ts_ms".into(), json!(session.started_ts_ms));
    }
    payload
}
