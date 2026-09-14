mod log_writer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::tools::redaction::{is_sensitive_key, redact_sensitive_text};
use crate::tools::tool_runtime::{
    descriptor as tool_runtime, request_mutates as runtime_request_mutates,
};

use log_writer::append_tool_usage_log;

const MAX_LOG_VALUE_BYTES: usize = 16 * 1024;
const MAX_LOG_STRING_CHARS: usize = 4 * 1024;
const MAX_ARGUMENT_RECORD_BYTES: usize = 4 * 1024;
const MAX_ARGUMENT_PREVIEW_BYTES: usize = 512;
const TOOL_USAGE_LOG_SCHEMA_VERSION: u64 = 7;
const DIAGNOSTICS_SCHEMA_VERSION: u64 = 1;
const DIAGNOSTICS_HOST_KIND: &str = "rust_desktop";
const ACTIVITY_BURST_IDLE_MS: u64 = 120_000;
static TOOL_USAGE_CALL_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static RUNTIME_BOOT_ID: OnceLock<String> = OnceLock::new();
static ACTIVITY_STATES: OnceLock<Mutex<HashMap<String, ActivityState>>> = OnceLock::new();

#[derive(Default)]
struct ActivityState {
    last_completed_ts_ms: u64,
    burst_id: u64,
    burst_sequence: u64,
    active_requests: u64,
    last_failure_signature: Option<String>,
    last_failure_burst_id: u64,
    repeat_failure_count: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolRequestTiming {
    pub previous_response_completed_ts_ms: Option<u64>,
    pub orchestration_gap_ms: Option<u64>,
    pub activity_burst_id: u64,
    pub activity_burst_sequence: u64,
    pub concurrent_request: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AsyncSessionTelemetry<'a> {
    pub profile_id: &'a str,
    pub session_id: &'a str,
    pub command_kind: &'a str,
    pub started_ts_ms: u64,
    pub child_process_total_ms: u64,
    pub first_output_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub termination_reason: &'a str,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

pub(crate) fn runtime_boot_id() -> &'static str {
    RUNTIME_BOOT_ID
        .get_or_init(|| uuid::Uuid::new_v4().to_string())
        .as_str()
}

pub(crate) fn begin_tool_request(profile_id: &str, started_ts_ms: u128) -> ToolRequestTiming {
    let started_ts_ms = started_ts_ms.min(u64::MAX as u128) as u64;
    let states = ACTIVITY_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = states.entry(profile_id.to_string()).or_default();
    let concurrent_request = state.active_requests > 0;
    let previous = (state.last_completed_ts_ms > 0).then_some(state.last_completed_ts_ms);
    let gap = if concurrent_request {
        None
    } else {
        previous.map(|completed| started_ts_ms.saturating_sub(completed))
    };
    if state.burst_id == 0 || gap.is_some_and(|gap| gap > ACTIVITY_BURST_IDLE_MS) {
        state.burst_id = state.burst_id.saturating_add(1).max(1);
        state.burst_sequence = 0;
    }
    state.burst_sequence = state.burst_sequence.saturating_add(1);
    state.active_requests = state.active_requests.saturating_add(1);
    ToolRequestTiming {
        previous_response_completed_ts_ms: previous,
        orchestration_gap_ms: gap,
        activity_burst_id: state.burst_id,
        activity_burst_sequence: state.burst_sequence,
        concurrent_request,
    }
}

fn complete_tool_request(profile_id: &str, completed_ts_ms: u128) {
    let completed_ts_ms = completed_ts_ms.min(u64::MAX as u128) as u64;
    let states = ACTIVITY_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = states.entry(profile_id.to_string()).or_default();
    state.last_completed_ts_ms = state.last_completed_ts_ms.max(completed_ts_ms);
    state.active_requests = state.active_requests.saturating_sub(1);
}

pub(crate) struct ToolUsageInput<'a> {
    pub profile_id: &'a str,
    pub transport_mode: &'a str,
    pub protocol_version: &'a str,
    pub request_id: &'a Value,
    pub method: &'a str,
    pub tool_name: &'a str,
    pub arguments: &'a Value,
    pub request_json_bytes: usize,
    pub rpc_fast_path: bool,
    pub request_timing: &'a ToolRequestTiming,
    pub started_ts_ms: u128,
    pub duration_ms: u128,
    pub outcome: &'a str,
    pub response: Option<&'a Value>,
    pub worker_error: Option<&'a str>,
    pub host_session_key: Option<&'a str>,
    pub redact_telemetry: bool,
}

pub(crate) fn format_log_value(value: &Value) -> String {
    let sanitized = sanitize_log_value(value, None);
    let mut serialized = serde_json::to_string(&sanitized).unwrap_or_else(|_| "null".to_string());
    if serialized.len() > MAX_LOG_VALUE_BYTES {
        let mut boundary = MAX_LOG_VALUE_BYTES;
        while !serialized.is_char_boundary(boundary) {
            boundary -= 1;
        }
        serialized.truncate(boundary);
        serialized.push_str("...[TRUNCATED]");
    }
    serialized
}

pub(crate) fn format_request_log_value(value: &Value) -> String {
    let sanitized = sanitize_log_value(value, None);
    let serialized = serde_json::to_vec(&sanitized).unwrap_or_default();
    let preview = format_log_value(value);
    let keys = sanitized
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    serde_json::to_string(&json!({
        "payload_omitted": true,
        "json_bytes": serialized.len(),
        "sha256": format!("{:x}", Sha256::digest(&serialized)),
        "keys": keys,
        "path": sanitized.get("path").and_then(Value::as_str),
        "workdir": sanitized.get("workdir").and_then(Value::as_str),
        "preview": truncate_utf8(&preview, 256)
    }))
    .unwrap_or_else(|_| "null".to_string())
}

fn failure_signature(record: &Map<String, Value>) -> Option<String> {
    let is_error = record.get("is_error").and_then(Value::as_bool) == Some(true)
        || record
            .get("outcome")
            .and_then(Value::as_str)
            .is_some_and(|outcome| outcome != "success");
    if !is_error {
        return None;
    }
    let mut identity = vec![
        json!("tool-failure-v1"),
        record.get("tool").cloned().unwrap_or(Value::Null),
        record
            .get("arguments_sha256")
            .cloned()
            .unwrap_or(Value::Null),
        record
            .get("error_code")
            .or_else(|| record.get("rpc_error_code"))
            .cloned()
            .unwrap_or(Value::Null),
        record.get("error_category").cloned().unwrap_or(Value::Null),
    ];
    for field in [
        "path",
        "file_index",
        "edit_index",
        "start_line",
        "end_line",
        "expected_sha256",
        "actual_sha256",
        "expected_occurrences",
        "actual_occurrences",
        "recovery_reason",
    ] {
        identity.push(
            record
                .get(&format!("error_{field}"))
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    let serialized = serde_json::to_vec(&identity).unwrap_or_default();
    Some(format!("{:x}", Sha256::digest(serialized)))
}

fn reset_failure_chain(state: &mut ActivityState) {
    state.last_failure_signature = None;
    state.last_failure_burst_id = 0;
    state.repeat_failure_count = 0;
}

fn annotate_repeated_failure(profile_id: &str, record: &mut Value) {
    let Some(object) = record.as_object_mut() else {
        return;
    };
    let signature = failure_signature(object);
    let burst_id = object
        .get("activity_burst_id")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let concurrent = object.get("concurrent_request").and_then(Value::as_bool) == Some(true);
    let states = ACTIVITY_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = states.entry(profile_id.to_string()).or_default();
    let Some(signature) = signature else {
        reset_failure_chain(state);
        return;
    };
    let repeated = !concurrent
        && state.last_failure_signature.as_deref() == Some(signature.as_str())
        && state.last_failure_burst_id == burst_id;
    let count = if repeated {
        state.repeat_failure_count.saturating_add(1)
    } else {
        1
    };
    object.insert("failure_signature".into(), json!(signature.clone()));
    object.insert("repeat_failure_count".into(), json!(count));
    object.insert("repeated_failure".into(), json!(count > 1));
    object.insert("retry_without_change".into(), json!(count > 1));
    if concurrent {
        reset_failure_chain(state);
    } else {
        state.last_failure_signature = Some(signature);
        state.last_failure_burst_id = burst_id;
        state.repeat_failure_count = count;
    }
}

pub(crate) fn record_tool_usage(input: ToolUsageInput<'_>) {
    let completed_ts_ms = input.started_ts_ms.saturating_add(input.duration_ms);
    let mut record = build_tool_usage_record(&input);
    annotate_repeated_failure(input.profile_id, &mut record);
    let diagnostic_event_id = uuid::Uuid::new_v4().to_string();
    if let Some(object) = record.as_object_mut() {
        object.insert(
            "diagnostic_event_id".into(),
            json!(diagnostic_event_id.clone()),
        );
    }
    let recovery_fields = recovery_event_fields(
        &record,
        &diagnostic_event_id,
        input.tool_name,
        input.outcome,
    );
    let selection = record
        .get("selected_workspace_id")
        .and_then(Value::as_str)
        .and_then(|folder_id| {
            crate::workspace_features::selected_skill_learning_attribution(
                input.profile_id,
                input.host_session_key,
                folder_id,
            )
        });
    crate::knowledge::enqueue_tool_usage_record(
        input.profile_id,
        &record,
        crate::knowledge::ExperienceAttribution {
            conversation_context_id: input.host_session_key.map(str::to_string),
            skill: selection.as_ref().map(|value| value.skill.clone()),
            canary: selection.and_then(|value| value.canary),
            ..crate::knowledge::ExperienceAttribution::default()
        },
    );
    append_tool_usage_log(input.profile_id, record);
    if let Some(fields) = recovery_fields {
        record_diagnostic_event(
            input.profile_id,
            "recovery_event",
            "recovery_attempt",
            if input.outcome == "success" {
                "info"
            } else {
                "warning"
            },
            if input.outcome == "success" {
                "none"
            } else {
                "tool"
            },
            fields,
        );
    }
    complete_tool_request(input.profile_id, completed_ts_ms);
}

fn recovery_event_fields(
    record: &Value,
    diagnostic_event_id: &str,
    tool_name: &str,
    outcome: &str,
) -> Option<Value> {
    if record.get("recovery_attempt").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let mut fields = Map::new();
    fields.insert(
        "linked_diagnostic_event_id".into(),
        json!(diagnostic_event_id),
    );
    fields.insert("tool".into(), json!(tool_name));
    fields.insert("recovery_succeeded".into(), json!(outcome == "success"));
    for field in [
        "retry_of_call_sequence",
        "recovery_of_operation_id_hash",
        "recovery_action_id",
    ] {
        if let Some(value) = record.get(field) {
            fields.insert(field.to_string(), value.clone());
        }
    }
    Some(Value::Object(fields))
}

pub(crate) fn record_async_session_finalized(input: AsyncSessionTelemetry<'_>) {
    append_tool_usage_log(
        input.profile_id,
        build_async_session_record(&input, unix_timestamp_ms()),
    );
}

pub(crate) fn record_diagnostic_event(
    profile_id: &str,
    event_type: &str,
    event: &str,
    severity: &str,
    failure_domain: &str,
    fields: Value,
) {
    let timestamp_ms = unix_timestamp_ms();
    let mut record = sanitize_log_value(&fields, None)
        .as_object()
        .cloned()
        .unwrap_or_default();
    record.insert(
        "schema_version".into(),
        json!(TOOL_USAGE_LOG_SCHEMA_VERSION),
    );
    record.insert(
        "diagnostic_schema_version".into(),
        json!(DIAGNOSTICS_SCHEMA_VERSION),
    );
    record.insert("event".into(), json!(event));
    record.insert("event_type".into(), json!(event_type));
    record.insert("host_kind".into(), json!(DIAGNOSTICS_HOST_KIND));
    record.insert("host_version".into(), json!(env!("CARGO_PKG_VERSION")));
    record.insert("timestamp_ms".into(), json!(timestamp_ms));
    record.insert("conversation_operation_id".into(), Value::Null);
    record.insert("severity".into(), json!(severity));
    record.insert("failure_domain".into(), json!(failure_domain));
    record.insert("workspace_id".into(), json!(profile_id));
    record.insert("runtime_boot_id".into(), json!(runtime_boot_id()));
    record.insert("server_version".into(), json!(env!("CARGO_PKG_VERSION")));
    record.insert("started_ts_ms".into(), json!(timestamp_ms));
    record.insert("completed_ts_ms".into(), json!(timestamp_ms));
    append_tool_usage_log(profile_id, Value::Object(record));
}

fn build_async_session_record(input: &AsyncSessionTelemetry<'_>, completed_ts_ms: u64) -> Value {
    let failed = input.exit_code.is_some_and(|code| code != 0);
    json!({
        "schema_version": TOOL_USAGE_LOG_SCHEMA_VERSION,
        "diagnostic_schema_version": DIAGNOSTICS_SCHEMA_VERSION,
        "event": "async_session_finalized",
        "event_type": "process_session",
        "host_kind": DIAGNOSTICS_HOST_KIND,
        "host_version": env!("CARGO_PKG_VERSION"),
        "timestamp_ms": input.started_ts_ms,
        "conversation_operation_id": Value::Null,
        "severity": if failed { "error" } else { "info" },
        "failure_domain": if failed { "process" } else { "none" },
        "workspace_id": input.profile_id,
        "runtime_boot_id": runtime_boot_id(),
        "server_version": env!("CARGO_PKG_VERSION"),
        "session_id": input.session_id,
        "command_kind": input.command_kind,
        "started_ts_ms": input.started_ts_ms,
        "completed_ts_ms": completed_ts_ms,
        "child_process_total_ms": input.child_process_total_ms,
        "first_output_ms": input.first_output_ms,
        "exit_code": input.exit_code,
        "termination_reason": input.termination_reason,
        "stdout_bytes": input.stdout_bytes,
        "stderr_bytes": input.stderr_bytes
    })
}

pub(crate) fn tool_usage_log_health() -> Value {
    log_writer::tool_usage_log_health()
}

pub(crate) fn legacy_compat_write_enabled() -> bool {
    log_writer::legacy_compat_write_enabled()
}

pub(crate) fn legacy_compat_write_env() -> &'static str {
    log_writer::LEGACY_COMPAT_WRITE_ENV
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn sanitize_log_value(value: &Value, key: Option<&str>) -> Value {
    if key.is_some_and(is_sensitive_key) {
        return Value::String("[REDACTED]".to_string());
    }

    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), sanitize_log_value(value, Some(key))))
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| sanitize_log_value(value, None))
                .collect(),
        ),
        Value::String(value) => {
            let (redacted, _) = redact_sensitive_text(value);
            let shortened: String = redacted.chars().take(MAX_LOG_STRING_CHARS).collect();
            if shortened.chars().count() < redacted.chars().count() {
                Value::String(format!("{shortened}...[TRUNCATED]"))
            } else {
                Value::String(shortened)
            }
        }
        _ => value.clone(),
    }
}

fn json_byte_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn tool_family(tool_name: &str) -> &'static str {
    tool_runtime(tool_name).usage_family.as_str()
}

fn request_mutates(tool_name: &str, arguments: &Value) -> bool {
    runtime_request_mutates(tool_name, arguments)
}

fn argument_command_preview(arguments: &Value) -> Option<String> {
    if let Some(cmd) = arguments.get("cmd").and_then(Value::as_str) {
        return Some(redact_sensitive_text(cmd).0);
    }
    if let Some(script) = arguments.get("script").and_then(Value::as_str) {
        return Some(redact_sensitive_text(script).0);
    }

    let program = arguments.get("program").and_then(Value::as_str)?;
    let mut command = program.to_string();
    if let Some(args) = arguments.get("args").and_then(Value::as_array) {
        for argument in args.iter().filter_map(Value::as_str) {
            command.push(' ');
            command.push_str(argument);
        }
    }
    Some(redact_sensitive_text(&command).0)
}

pub(crate) fn command_kind(arguments: &Value) -> &'static str {
    let text = argument_command_preview(arguments).unwrap_or_else(|| arguments.to_string());
    classify_command_text(&text)
}

pub(crate) fn classify_command_text(command: &str) -> &'static str {
    let command = command.to_ascii_lowercase();
    if command.contains("wait-process")
        || command.contains("get-process cargo")
        || command.contains("start-sleep")
    {
        "wait_poll"
    } else if command.contains("mcp-tool-usage")
        || command.contains("mcp-requests.log")
        || command.contains("query_tool_usage")
    {
        "log_query"
    } else if command.contains("cargo test")
        || command.contains("test-fast.ps1")
        || command.contains("test-full.ps1")
    {
        "cargo_test"
    } else if command.contains("cargo check") || command.contains("cargo-local.ps1 check") {
        "cargo_check"
    } else if command.contains("cargo build")
        || command.contains("tauri build")
        || command.contains("npm run build")
        || command.contains("pnpm build")
    {
        "build"
    } else if command.contains("rustfmt") || command.contains("cargo fmt") {
        "format"
    } else if command.trim_start().starts_with("git ")
        || command.contains(" git ")
        || command.contains("git.exe")
    {
        "git"
    } else if command.contains("pytest") || command.contains("python -m pytest") {
        "test"
    } else if command.contains("powershell") || command.contains("pwsh") {
        "shell"
    } else if command.contains("python") {
        "python"
    } else {
        "process"
    }
}

fn insert_structured_metric(
    record: &mut Map<String, Value>,
    structured: &Value,
    source: &str,
    destination: &str,
) {
    if let Some(value) = structured.get(source) {
        record.insert(destination.to_string(), value.clone());
    }
}

fn insert_array_count(
    record: &mut Map<String, Value>,
    structured: &Value,
    source: &str,
    destination: &str,
) {
    if let Some(values) = structured.get(source).and_then(Value::as_array) {
        record.insert(destination.to_string(), json!(values.len()));
    }
}

fn insert_warning_severity_counts(record: &mut Map<String, Value>, structured: &Value) {
    let Some(warnings) = structured.get("warnings").and_then(Value::as_array) else {
        return;
    };
    let mut notice = 0u64;
    let mut deprecation = 0u64;
    let mut recoverable = 0u64;
    let mut security = 0u64;
    let mut data_loss = 0u64;
    for warning in warnings.iter().filter_map(Value::as_str) {
        let lowered = warning.to_ascii_lowercase();
        if lowered.contains("deprecated") || lowered.contains("deprecation") {
            deprecation += 1;
        } else if ["security", "sandbox", "unsafe", "permission", "credential"]
            .iter()
            .any(|keyword| lowered.contains(keyword))
        {
            security += 1;
        } else if ["data loss", "overwrite", "delete", "irreversible"]
            .iter()
            .any(|keyword| lowered.contains(keyword))
        {
            data_loss += 1;
        } else if ["retry", "recover", "temporary", "retained"]
            .iter()
            .any(|keyword| lowered.contains(keyword))
        {
            recoverable += 1;
        } else {
            notice += 1;
        }
    }
    record.insert("notice_count".into(), json!(notice));
    record.insert("deprecation_count".into(), json!(deprecation));
    record.insert("recoverable_warning_count".into(), json!(recoverable));
    record.insert("security_warning_count".into(), json!(security));
    record.insert("data_loss_warning_count".into(), json!(data_loss));
}

fn mcp_text_content_bytes(result: &Value) -> usize {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .map(str::len)
                .sum()
        })
        .unwrap_or(0)
}

fn semantic_tool_arguments(arguments: &Value) -> Value {
    let mut semantic = arguments.clone();
    if let Some(object) = semantic.as_object_mut() {
        for field in [
            "retry_of_call_sequence",
            "recovery_of_operation_id",
            "recovery_action_id",
        ] {
            object.remove(field);
        }
    }
    semantic
}

fn conversation_operation_id(response: Option<&Value>) -> Option<&str> {
    response
        .and_then(|value| value.get("result"))
        .and_then(|value| value.get("structuredContent"))
        .and_then(|value| value.get("harness_operation_id"))
        .and_then(Value::as_str)
}

fn build_tool_usage_record(input: &ToolUsageInput<'_>) -> Value {
    let sanitized_arguments = if input.redact_telemetry {
        sanitize_log_value(input.arguments, None)
    } else {
        input.arguments.clone()
    };
    let arguments_bytes = serde_json::to_vec(&sanitized_arguments).unwrap_or_default();
    let semantic_arguments = semantic_tool_arguments(input.arguments);
    let sanitized_semantic_arguments = if input.redact_telemetry {
        sanitize_log_value(&semantic_arguments, None)
    } else {
        semantic_arguments.clone()
    };
    let semantic_argument_bytes =
        serde_json::to_vec(&sanitized_semantic_arguments).unwrap_or_default();
    let arguments_sha256 = format!("{:x}", Sha256::digest(&semantic_argument_bytes));
    let mut record = Map::new();
    record.insert(
        "schema_version".into(),
        json!(TOOL_USAGE_LOG_SCHEMA_VERSION),
    );
    record.insert(
        "diagnostic_schema_version".into(),
        json!(DIAGNOSTICS_SCHEMA_VERSION),
    );
    record.insert(
        "event_type".into(),
        json!(if input.outcome == "rpc_error" {
            "transport_event"
        } else {
            "tool_call"
        }),
    );
    record.insert("host_kind".into(), json!(DIAGNOSTICS_HOST_KIND));
    record.insert("host_version".into(), json!(env!("CARGO_PKG_VERSION")));
    record.insert("event".into(), json!("tool_call"));
    record.insert("timestamp_ms".into(), json!(input.started_ts_ms));
    record.insert(
        "conversation_operation_id".into(),
        conversation_operation_id(input.response)
            .map(|value| json!(value))
            .unwrap_or(Value::Null),
    );
    record.insert("started_ts_ms".into(), json!(input.started_ts_ms));
    record.insert(
        "completed_ts_ms".into(),
        json!(input.started_ts_ms.saturating_add(input.duration_ms)),
    );
    record.insert("workspace_id".into(), json!(input.profile_id));
    if let Some(folder_id) = input
        .response
        .and_then(|response| response.get("result"))
        .and_then(|result| result.get("structuredContent"))
        .and_then(|structured| structured.get("resolved_workspace_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        record.insert("selected_workspace_id".into(), json!(folder_id));
    }
    record.insert("runtime_boot_id".into(), json!(runtime_boot_id()));
    record.insert("server_version".into(), json!(env!("CARGO_PKG_VERSION")));
    record.insert("transport_mode".into(), json!(input.transport_mode));
    record.insert("protocol_version".into(), json!(input.protocol_version));
    record.insert("request_id".into(), input.request_id.clone());
    record.insert(
        "call_sequence".into(),
        json!(TOOL_USAGE_CALL_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1),
    );
    record.insert("method".into(), json!(input.method));
    record.insert("tool".into(), json!(input.tool_name));
    record.insert("tool_family".into(), json!(tool_family(input.tool_name)));
    record.insert(
        "mutating_tool".into(),
        json!(request_mutates(input.tool_name, input.arguments)),
    );
    record.insert("deprecated_tool".into(), Value::Bool(false));
    record.insert("rpc_fast_path".into(), json!(input.rpc_fast_path));
    record.insert(
        "previous_response_completed_ts_ms".into(),
        json!(input.request_timing.previous_response_completed_ts_ms),
    );
    record.insert(
        "orchestration_gap_ms".into(),
        json!(input.request_timing.orchestration_gap_ms),
    );
    record.insert(
        "activity_burst_id".into(),
        json!(input.request_timing.activity_burst_id),
    );
    record.insert(
        "activity_burst_sequence".into(),
        json!(input.request_timing.activity_burst_sequence),
    );
    record.insert(
        "concurrent_request".into(),
        json!(input.request_timing.concurrent_request),
    );
    record.insert(
        "orchestration_gap_semantics".into(),
        json!("time from the previous completed tool response to this request being received; includes client, network, platform, and model orchestration"),
    );
    record.insert("duration_ms".into(), json!(input.duration_ms));
    record.insert("outcome".into(), json!(input.outcome));
    let failure_domain = match input.outcome {
        "success" => "none",
        "rpc_error" => "transport",
        _ => "tool",
    };
    record.insert("failure_domain".into(), json!(failure_domain));
    record.insert(
        "severity".into(),
        json!(if failure_domain == "none" {
            "info"
        } else {
            "error"
        }),
    );
    record.insert("request_json_bytes".into(), json!(input.request_json_bytes));
    record.insert("arguments_json_bytes".into(), json!(arguments_bytes.len()));
    record.insert("arguments_sha256".into(), json!(arguments_sha256));
    record.insert(
        "semantic_arguments_json_bytes".into(),
        json!(semantic_argument_bytes.len()),
    );
    if let Some(sequence) = input
        .arguments
        .get("retry_of_call_sequence")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
    {
        record.insert("retry_of_call_sequence".into(), json!(sequence));
    }
    if let Some(operation_id) = input
        .arguments
        .get("recovery_of_operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        record.insert(
            "recovery_of_operation_id_hash".into(),
            json!(format!("{:x}", Sha256::digest(operation_id.as_bytes()))),
        );
    }
    if let Some(action_id) = input
        .arguments
        .get("recovery_action_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        record.insert(
            "recovery_action_id".into(),
            sanitize_log_value(
                &Value::String(action_id.to_string()),
                Some("recovery_action_id"),
            ),
        );
    }
    let recovery_attempt = record.contains_key("retry_of_call_sequence")
        || record.contains_key("recovery_of_operation_id_hash")
        || record.contains_key("recovery_action_id");
    record.insert("recovery_attempt".into(), Value::Bool(recovery_attempt));
    record.insert(
        "arguments_truncated".into(),
        json!(arguments_bytes.len() > MAX_ARGUMENT_RECORD_BYTES),
    );

    if arguments_bytes.len() <= MAX_ARGUMENT_RECORD_BYTES {
        record.insert("arguments".into(), sanitized_arguments);
    } else {
        let preview = format_log_value(input.arguments);
        record.insert(
            "arguments_preview".into(),
            Value::String(truncate_utf8(&preview, MAX_ARGUMENT_PREVIEW_BYTES)),
        );
    }
    if let Some(object) = input.arguments.as_object() {
        let mut keys = object.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        let field_bytes = object
            .iter()
            .map(|(key, value)| (key.clone(), json!(json_byte_len(value))))
            .collect::<Map<String, Value>>();
        record.insert("argument_keys".into(), json!(keys));
        record.insert("argument_field_bytes".into(), Value::Object(field_bytes));
    }
    if let Some(command) = argument_command_preview(input.arguments) {
        record.insert("command_kind".into(), json!(command_kind(input.arguments)));
        record.insert(
            "command_preview".into(),
            sanitize_log_value(&Value::String(command), None),
        );
    }
    for field in ["reason", "workdir", "path", "output_mode"] {
        if let Some(value) = input.arguments.get(field) {
            record.insert(
                format!("argument_{field}"),
                sanitize_log_value(value, Some(field)),
            );
        }
    }
    if let Some(apply) = input.arguments.get("apply_proposal") {
        record.insert("edit_proposal_apply_requested".into(), Value::Bool(true));
        if let Some(id) = apply.get("proposal_id").and_then(Value::as_str) {
            record.insert(
                "edit_proposal_id_hash".into(),
                json!(format!("{:x}", Sha256::digest(id.as_bytes()))),
            );
        }
        record.insert(
            "edit_proposal_patch_bytes".into(),
            json!(apply
                .get("patch")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0)),
        );
        record.insert(
            "edit_proposal_replacement_bytes".into(),
            json!(apply
                .get("replacement")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0)),
        );
        record.insert(
            "edit_proposal_requested_format".into(),
            json!(if apply.get("patch").is_some() {
                "patch"
            } else if apply.get("replacement").is_some() {
                "replacement"
            } else {
                "accept"
            }),
        );
    }

    if let Some(response) = input.response {
        record.insert("response_json_bytes".into(), json!(json_byte_len(response)));
        if let Some(error) = response.get("error") {
            insert_structured_metric(&mut record, error, "code", "rpc_error_code");
            if let Some(message) = error.get("message") {
                record.insert(
                    "rpc_error_message".into(),
                    sanitize_log_value(message, Some("message")),
                );
            }
            if let Some(data) = error.get("data") {
                for field in ["stage", "reason", "retryable", "suggestion"] {
                    if let Some(value) = data.get(field) {
                        record.insert(
                            format!("rpc_error_{field}"),
                            sanitize_log_value(value, Some(field)),
                        );
                    }
                }
                if data.get("reason").and_then(Value::as_str) == Some("unknown_tool") {
                    record.insert("error_code".into(), json!("UNKNOWN_TOOL"));
                    record.insert("error_category".into(), json!("catalog"));
                    record.insert("error_retryable".into(), Value::Bool(true));
                }
            }
        }

        if let Some(result) = response.get("result") {
            record.insert("result_json_bytes".into(), json!(json_byte_len(result)));
            record.insert(
                "response_text_bytes".into(),
                json!(mcp_text_content_bytes(result)),
            );
            record.insert(
                "content_item_count".into(),
                json!(result
                    .get("content")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)),
            );
            insert_structured_metric(&mut record, result, "isError", "is_error");

            if let Some(structured) = result.get("structuredContent") {
                record.insert(
                    "structured_content_json_bytes".into(),
                    json!(json_byte_len(structured)),
                );
                if let Some(object) = structured.as_object() {
                    record.insert("structured_field_count".into(), json!(object.len()));
                }
                record.insert(
                    "tool_inline_fast".into(),
                    Value::Bool(
                        structured.get("execution_lane").and_then(Value::as_str)
                            == Some("inline_fast"),
                    ),
                );
                for field in [
                    "ok",
                    "transport_ok",
                    "execution_ok",
                    "command_ok",
                    "verification_ok",
                    "status",
                    "termination_reason",
                    "exit_code",
                    "process_exit_code",
                    "process_still_running",
                    "process_timed_out",
                    "request_timed_out",
                    "recoverable",
                    "task_required",
                    "truncated",
                    "stdout_truncated",
                    "stderr_truncated",
                    "has_more_output",
                    "cursor_expired",
                    "cursor",
                    "next_cursor",
                    "latest_cursor",
                    "output_mode",
                    "operation_id",
                    "session_id",
                    "harness_mode",
                    "execution_mode",
                    "execution_lane",
                    "resumed_execution_lane",
                    "blocking_queue_wait_ms",
                    "admission_lane",
                    "admission_limit",
                    "global_admission_limit",
                    "admission_scope",
                    "workspace_admission_wait_ms",
                    "global_admission_wait_ms",
                    "admission_queue_wait_ms",
                    "resource_class",
                    "io_heavy_admission_limit",
                    "io_heavy_admission_wait_ms",
                    "workspace_lock_scope",
                    "workspace_lock_groups",
                    "workspace_lock_wait_ms",
                    "operation_lock_wait_ms",
                    "resource_lock_wait_ms",
                    "resource_lock_group",
                    "resource_lock_target",
                    "session_registry_wait_ms",
                    "actual_wait_ms",
                    "snapshot_ms",
                    "active_session_limit",
                    "active_session_slots_available",
                    "execution_boundary",
                    "sandbox_enforced",
                    "program",
                    "shell",
                    "resolved_cwd",
                    "child_process",
                    "interactive",
                    "stdin_open",
                    "elapsed_ms",
                    "returned_count",
                    "total_matches",
                    "scanned_files",
                    "total_matches_exact",
                    "calculate_total",
                    "exact_total_fast_tail",
                    "exact_total_fast_tail_skipped_match_details",
                    "knowledge_canary_id",
                    "knowledge_canary_implementation",
                    "knowledge_canary_eligible",
                    "knowledge_canary_stage",
                    "knowledge_canary_selected",
                    "knowledge_canary_applied",
                    "knowledge_canary_bucket",
                    "matched_files",
                    "files_considered",
                    "scan_completed",
                    "early_stop_reason",
                    "skipped_large_files",
                    "bytes_read",
                    "total_bytes",
                    "total_lines",
                    "total_stream_bytes",
                    "total_retained_bytes",
                    "retained_start_offset",
                    "requested_offset",
                    "offset",
                    "limit",
                    "clean",
                    "applied",
                    "dry_run",
                    "transaction_stage",
                    "selected_path_count",
                    "staged_path_count_before",
                    "staged_path_count",
                    "index_clean_before",
                    "staged_by_tool",
                    "index_restored",
                    "proposal_ttl_seconds",
                    "candidate_start_line",
                    "candidate_end_line",
                    "proposal_apply_format",
                    "preferred_format",
                    "preferred_format_reason",
                    "replacement_bytes",
                    "proposed_content_bytes",
                    "proposed_content_included",
                    "next_action",
                    "failed_command_count",
                    "skipped_command_count",
                    "batch_summary",
                    "mode",
                    "requested_mode",
                    "auto_selected",
                    "parallel_decision_source",
                    "parallel_confidence",
                    "parallel_history_samples",
                    "parallel_blocked_pair_count",
                    "recommended_max_parallel",
                    "inferred_lock_group_count",
                    "parallel_observation_count",
                    "parallelism_observation_truncated",
                    "wait_timeout_ms",
                    "effective_wait_ms",
                    "heartbeat_ms",
                    "heartbeat",
                    "deduplicated",
                    "attached_to_session_id",
                    "detached",
                    "operation_id",
                    "command_fingerprint",
                    "process_id",
                    "process_tree_contained",
                    "process_tree_control",
                    "resolved_by",
                    "retention_seconds",
                    "wait_until",
                ] {
                    insert_structured_metric(&mut record, structured, field, field);
                }
                if input.tool_name == "exec_many" {
                    if let Some(observations) = structured
                        .get("parallelism_observations")
                        .and_then(Value::as_array)
                    {
                        record.insert(
                            "parallelism_observations".into(),
                            Value::Array(
                                observations
                                    .iter()
                                    .take(128)
                                    .map(|observation| {
                                        sanitize_log_value(
                                            observation,
                                            Some("parallelism_observations"),
                                        )
                                    })
                                    .collect(),
                            ),
                        );
                    }
                    if let Some(reasons) = structured
                        .get("parallel_decision_reasons")
                        .and_then(Value::as_array)
                    {
                        record.insert(
                            "parallel_decision_reasons".into(),
                            Value::Array(
                                reasons
                                    .iter()
                                    .take(16)
                                    .map(|reason| {
                                        sanitize_log_value(
                                            reason,
                                            Some("parallel_decision_reasons"),
                                        )
                                    })
                                    .collect(),
                            ),
                        );
                    }
                }
                if input.tool_name == "format_files" {
                    for (source, destination) in [
                        ("mode", "format_mode"),
                        ("scope", "format_scope"),
                        ("files_requested", "format_files_requested"),
                        ("files_supported", "format_files_supported"),
                        ("files_changed_count", "format_files_changed_count"),
                        ("files_unchanged_count", "format_files_unchanged_count"),
                        ("files_skipped_count", "format_files_skipped_count"),
                        ("formatter_group_count", "format_formatter_group_count"),
                        (
                            "custom_formatter_group_count",
                            "format_custom_formatter_group_count",
                        ),
                        ("diff_bytes", "format_diff_bytes"),
                        ("diff_truncated", "format_diff_truncated"),
                        ("applied", "format_applied"),
                    ] {
                        insert_structured_metric(&mut record, structured, source, destination);
                    }
                    insert_array_count(
                        &mut record,
                        structured,
                        "unavailable_adapters",
                        "format_unavailable_adapter_count",
                    );
                    insert_array_count(
                        &mut record,
                        structured,
                        "unexpected_changes",
                        "format_unexpected_change_count",
                    );
                }
                if let Some(phases) = structured.get("phase_durations_ms") {
                    for (source, destination) in [
                        ("preflight_ms", "phase_preflight_ms"),
                        ("plan_ms", "phase_plan_ms"),
                        ("commit_ms", "phase_commit_ms"),
                        ("total_ms", "phase_total_ms"),
                    ] {
                        insert_structured_metric(&mut record, phases, source, destination);
                    }
                }
                if let Some(error) = structured.get("error") {
                    insert_structured_metric(&mut record, error, "code", "error_code");
                    insert_structured_metric(&mut record, error, "category", "error_category");
                    insert_structured_metric(&mut record, error, "retryable", "error_retryable");
                    if let Some(details) = error.get("details") {
                        for field in [
                            "stage",
                            "reason",
                            "suggestion",
                            "recommended_tool",
                            "recommended_format",
                            "patch_bytes",
                            "replacement_bytes",
                            "path",
                            "file_index",
                            "edit_index",
                            "start_line",
                            "end_line",
                            "actual_sha256",
                            "expected_sha256",
                            "actual_occurrences",
                            "expected_occurrences",
                            "recovery_reason",
                            "transaction_stage",
                            "selected_path_count",
                            "staged_path_count_before",
                            "staged_path_count",
                            "index_clean_before",
                            "staged_by_tool",
                            "index_restored",
                        ] {
                            if let Some(value) = details.get(field) {
                                record.insert(
                                    format!("error_{field}"),
                                    sanitize_log_value(value, Some(field)),
                                );
                            }
                        }
                        insert_array_count(
                            &mut record,
                            details,
                            "recovery_actions",
                            "recovery_action_count",
                        );
                    }
                }
                for field in [
                    "failure_id",
                    "recovery_of_operation_id_hash",
                    "recovery_action_id",
                    "recovery_attempt",
                    "recovery_succeeded",
                ] {
                    if let Some(value) = structured.get(field) {
                        record.insert(field.to_string(), sanitize_log_value(value, Some(field)));
                    }
                }
                if let Some(value) = structured.get("retry_of_call_sequence") {
                    record.insert("retry_of_call_sequence".into(), value.clone());
                }
                if let Some(duration) = structured.get("duration_ms") {
                    record.insert("tool_reported_duration_ms".into(), duration.clone());
                }
                for field in ["suggestion", "recovery_hint"] {
                    if let Some(value) = structured.get(field) {
                        record.insert(field.to_string(), sanitize_log_value(value, Some(field)));
                    }
                }
                if let Some(stdout) = structured.get("stdout").and_then(Value::as_str) {
                    record.insert("stdout_bytes".into(), json!(stdout.len()));
                }
                if let Some(stderr) = structured.get("stderr").and_then(Value::as_str) {
                    record.insert("stderr_bytes".into(), json!(stderr.len()));
                }
                for (source, destination) in [
                    ("warnings", "warning_count"),
                    ("next_actions", "next_action_count"),
                    ("recovery_actions", "recovery_action_count"),
                    ("failed_command_ids", "failed_command_id_count"),
                    ("skipped_command_ids", "skipped_command_id_count"),
                    ("events", "event_count"),
                    ("affected_files", "affected_file_count"),
                    ("entries", "entry_count"),
                    ("matches", "match_count"),
                    ("commits", "commit_count"),
                    ("files", "file_count"),
                    ("would_create", "would_create_count"),
                    ("would_modify", "would_modify_count"),
                    ("would_delete", "would_delete_count"),
                ] {
                    insert_array_count(&mut record, structured, source, destination);
                }
                insert_warning_severity_counts(&mut record, structured);
            }
        }
    }

    if let Some(error) = input.worker_error {
        record.insert(
            "worker_error".into(),
            sanitize_log_value(&Value::String(error.to_string()), Some("error")),
        );
    }
    record.insert(
        "outcome_class".into(),
        json!(classify_outcome(&record, input.outcome)),
    );

    Value::Object(record)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}...[TRUNCATED]", &value[..boundary])
}

fn classify_outcome(record: &Map<String, Value>, outcome: &str) -> &'static str {
    let code = record
        .get("error_code")
        .or_else(|| record.get("rpc_error_code"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let category = record
        .get("error_category")
        .and_then(Value::as_str)
        .unwrap_or("");
    if record.get("request_timed_out").and_then(Value::as_bool) == Some(true)
        || record.get("process_timed_out").and_then(Value::as_bool) == Some(true)
    {
        "timeout"
    } else if record.get("command_ok").and_then(Value::as_bool) == Some(false) {
        if record.get("verification_ok").and_then(Value::as_bool) == Some(false) {
            "verification_failure"
        } else if record
            .get("process_exit_code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
        {
            "process_failure"
        } else {
            "command_failure"
        }
    } else if outcome == "rpc_error" {
        "transport_error"
    } else if outcome == "success" {
        "success"
    } else if code == "UNKNOWN_TOOL" {
        "catalog_mismatch"
    } else if matches!(category, "policy" | "permission" | "security")
        || code.contains("POLICY")
        || code.contains("PERMISSION")
        || code.starts_with("DANGEROUS_OPERATION_")
        || code.starts_with("PROTECTED_")
    {
        "policy_rejection"
    } else if code == "GIT_REPO_TARGET_MISMATCH"
        || category == "workspace_routing"
        || code.contains("ROUTING")
    {
        "routing_error"
    } else if matches!(
        code,
        "EDIT_MATCH_COUNT_MISMATCH"
            | "PATCH_CONTEXT_AMBIGUOUS"
            | "PATCH_CONTEXT_NOT_FOUND"
            | "PATCH_HUNK_COUNT_MISMATCH"
            | "NOT_FOUND"
            | "NOT_GIT_REPOSITORY"
            | "EDIT_PROPOSAL_NOT_FOUND"
    ) || category == "not_found"
    {
        "target_resolution_error"
    } else if matches!(
        code,
        "FILE_VERSION_MISMATCH"
            | "EDIT_EXPECTED_TEXT_MISMATCH"
            | "GIT_HEAD_MISMATCH"
            | "GIT_INDEX_NOT_CLEAN"
            | "BASELINE_STALE"
            | "EXPECTED_HEAD_MISMATCH"
    ) || category == "conflict"
    {
        "state_conflict"
    } else if code.contains("TIMEOUT") {
        "timeout"
    } else if code.contains("CANCEL") {
        "cancelled"
    } else if code.contains("BUSY") || code.contains("LIMIT_REACHED") {
        "admission_rejected"
    } else if record
        .get("process_exit_code")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0)
    {
        "process_failure"
    } else if category == "validation"
        || matches!(code, "INVALID_ARGUMENT" | "EDIT_CONTRACT_INVALID")
    {
        "caller_argument_error"
    } else {
        "internal_error"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{
        annotate_repeated_failure, begin_tool_request, build_async_session_record,
        build_tool_usage_record, classify_command_text, classify_outcome, complete_tool_request,
        format_log_value, format_request_log_value, recovery_event_fields, AsyncSessionTelemetry,
        ToolRequestTiming, ToolUsageInput,
    };

    #[test]
    fn request_timing_tracks_gaps_and_splits_idle_bursts() {
        let profile = format!("timing-test-{}", uuid::Uuid::new_v4());
        let first = begin_tool_request(&profile, 1_000);
        assert_eq!(first.orchestration_gap_ms, None);
        assert_eq!(first.activity_burst_id, 1);
        assert_eq!(first.activity_burst_sequence, 1);
        complete_tool_request(&profile, 1_100);

        let second = begin_tool_request(&profile, 2_000);
        assert_eq!(second.orchestration_gap_ms, Some(900));
        assert_eq!(second.activity_burst_id, 1);
        assert_eq!(second.activity_burst_sequence, 2);
        complete_tool_request(&profile, 2_100);

        let after_idle = begin_tool_request(&profile, 200_000);
        assert_eq!(after_idle.orchestration_gap_ms, Some(197_900));
        assert_eq!(after_idle.activity_burst_id, 2);
        assert_eq!(after_idle.activity_burst_sequence, 1);
        complete_tool_request(&profile, 200_100);
    }

    #[test]
    fn concurrent_requests_do_not_create_fake_orchestration_gaps() {
        let profile = format!("concurrent-timing-test-{}", uuid::Uuid::new_v4());
        let first = begin_tool_request(&profile, 1_000);
        let concurrent = begin_tool_request(&profile, 1_050);

        assert!(!first.concurrent_request);
        assert!(concurrent.concurrent_request);
        assert_eq!(concurrent.orchestration_gap_ms, None);
        assert_eq!(concurrent.activity_burst_id, first.activity_burst_id);

        complete_tool_request(&profile, 1_100);
        complete_tool_request(&profile, 1_150);
    }

    #[test]
    fn repeated_failure_detection_resets_at_safe_boundaries() {
        let profile = format!("repeat-failure-test-{}", uuid::Uuid::new_v4());
        let failure = |arguments_sha256: &str, burst_id: u64, concurrent: bool| {
            json!({
                "outcome": "tool_error",
                "is_error": true,
                "tool": "edit_file",
                "arguments_sha256": arguments_sha256,
                "error_code": "EDIT_MATCH_COUNT_MISMATCH",
                "error_category": "validation",
                "error_path": "main.txt",
                "error_edit_index": 0,
                "error_actual_occurrences": 0,
                "error_expected_occurrences": 1,
                "activity_burst_id": burst_id,
                "concurrent_request": concurrent
            })
        };

        let mut first = failure(&"a".repeat(64), 7, false);
        annotate_repeated_failure(&profile, &mut first);
        let mut second = failure(&"a".repeat(64), 7, false);
        annotate_repeated_failure(&profile, &mut second);
        assert_eq!(first["failure_signature"].as_str().unwrap().len(), 64);
        assert_eq!(first["repeat_failure_count"], 1);
        assert_eq!(first["repeated_failure"], false);
        assert_eq!(second["failure_signature"], first["failure_signature"]);
        assert_eq!(second["repeat_failure_count"], 2);
        assert_eq!(second["repeated_failure"], true);
        assert_eq!(second["retry_without_change"], true);

        let mut changed = failure(&"b".repeat(64), 7, false);
        annotate_repeated_failure(&profile, &mut changed);
        assert_eq!(changed["repeat_failure_count"], 1);
        let mut new_burst = failure(&"b".repeat(64), 8, false);
        annotate_repeated_failure(&profile, &mut new_burst);
        assert_eq!(new_burst["repeat_failure_count"], 1);
        let mut concurrent = failure(&"b".repeat(64), 8, true);
        annotate_repeated_failure(&profile, &mut concurrent);
        assert_eq!(concurrent["repeat_failure_count"], 1);
        assert_eq!(concurrent["repeated_failure"], false);
        let mut after_concurrent = failure(&"b".repeat(64), 8, false);
        annotate_repeated_failure(&profile, &mut after_concurrent);
        assert_eq!(after_concurrent["repeat_failure_count"], 1);

        let mut success = json!({
            "outcome": "success",
            "is_error": false,
            "activity_burst_id": 8,
            "concurrent_request": false
        });
        annotate_repeated_failure(&profile, &mut success);
        assert!(success.get("failure_signature").is_none());
        let mut after_success = failure(&"b".repeat(64), 8, false);
        annotate_repeated_failure(&profile, &mut after_success);
        assert_eq!(after_success["repeat_failure_count"], 1);
    }

    #[test]
    fn async_session_record_captures_real_child_process_lifetime() {
        let record = build_async_session_record(
            &AsyncSessionTelemetry {
                profile_id: "workspace",
                session_id: "session-1",
                command_kind: "cargo_test",
                started_ts_ms: 1_000,
                child_process_total_ms: 5_250,
                first_output_ms: Some(420),
                exit_code: Some(0),
                termination_reason: "exited",
                stdout_bytes: 123,
                stderr_bytes: 7,
            },
            6_250,
        );

        assert_eq!(record["schema_version"], 7);
        assert_eq!(record["event"], "async_session_finalized");
        assert_eq!(record["session_id"], "session-1");
        assert_eq!(record["command_kind"], "cargo_test");
        assert_eq!(record["started_ts_ms"], 1_000);
        assert_eq!(record["completed_ts_ms"], 6_250);
        assert_eq!(record["timestamp_ms"], 1_000);
        assert_eq!(record["conversation_operation_id"], Value::Null);
        assert_eq!(record["child_process_total_ms"], 5_250);
        assert_eq!(record["first_output_ms"], 420);
        assert_eq!(record["termination_reason"], "exited");
    }

    #[test]
    fn command_kind_distinguishes_waiting_testing_and_log_queries() {
        assert_eq!(classify_command_text("cargo test --lib"), "cargo_test");
        assert_eq!(
            classify_command_text("Get-Process cargo | Wait-Process"),
            "wait_poll"
        );
        assert_eq!(
            classify_command_text("cat mcp-tool-usage.jsonl"),
            "log_query"
        );
    }

    #[test]
    fn command_text_secrets_are_redacted() {
        let value = json!({
            "cmd": "curl --token super-secret -H 'Authorization: Bearer abc.def.ghi' API_KEY=hidden"
        });

        let logged = format_log_value(&value);

        assert!(logged.contains("--token [REDACTED]"));
        assert!(logged.contains("Authorization: [REDACTED]"));
        assert!(logged.contains("API_KEY=[REDACTED]"));
        assert!(!logged.contains("super-secret"));
        assert!(!logged.contains("abc.def.ghi"));
        assert!(!logged.contains("hidden"));
    }

    #[test]
    fn large_request_log_uses_a_digest_and_bounded_preview() {
        let value = json!({
            "path": "src/main.rs",
            "patch": "x".repeat(32 * 1024)
        });

        let logged = format_request_log_value(&value);
        let summary: serde_json::Value = serde_json::from_str(&logged).expect("request summary");
        assert_eq!(summary["payload_omitted"], true);
        assert_eq!(summary["path"], "src/main.rs");
        assert_eq!(summary["sha256"].as_str().expect("digest").len(), 64);
        assert!(logged.len() < 4 * 1024);
    }

    #[test]
    fn successful_transport_does_not_hide_command_failure() {
        let mut record = serde_json::Map::new();
        record.insert("command_ok".into(), json!(false));
        record.insert("verification_ok".into(), json!(true));
        record.insert("process_exit_code".into(), json!(2));
        assert_eq!(classify_outcome(&record, "success"), "process_failure");

        record.insert("verification_ok".into(), json!(false));
        assert_eq!(classify_outcome(&record, "success"), "verification_failure");
    }

    #[test]
    fn tool_outcome_taxonomy_separates_recoverable_failures_from_internal_errors() {
        let cases = [
            (
                "EDIT_MATCH_COUNT_MISMATCH",
                "validation",
                "target_resolution_error",
            ),
            (
                "PATCH_CONTEXT_AMBIGUOUS",
                "validation",
                "target_resolution_error",
            ),
            ("FILE_VERSION_MISMATCH", "conflict", "state_conflict"),
            ("GIT_REPO_TARGET_MISMATCH", "conflict", "routing_error"),
            (
                "EDIT_CONTRACT_INVALID",
                "validation",
                "caller_argument_error",
            ),
            ("PROTECTED_PATH", "security", "policy_rejection"),
            ("E_FAIL", "runtime", "internal_error"),
        ];

        for (code, category, expected) in cases {
            let mut record = serde_json::Map::new();
            record.insert("error_code".into(), json!(code));
            record.insert("error_category".into(), json!(category));
            assert_eq!(classify_outcome(&record, "tool_error"), expected, "{code}");
        }
    }

    #[test]
    fn rpc_errors_are_classified_as_transport_failures() {
        let record = serde_json::Map::new();
        assert_eq!(classify_outcome(&record, "rpc_error"), "transport_error");
    }

    #[test]
    fn usage_record_contains_payload_and_result_metrics() {
        let arguments = json!({
            "cmd": "cargo test --token hidden-value",
            "reason": "verify changes",
            "stdin": "do-not-log"
        });
        let response = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {
                "content": [{"type": "text", "text": "done"}],
                "structuredContent": {
                    "ok": true,
                    "status": "exited",
                    "duration_ms": 12,
                    "phase_durations_ms": {
                        "preflight_ms": 4,
                        "plan_ms": 2,
                        "commit_ms": 5,
                        "total_ms": 11
                    },
                    "operation_lock_wait_ms": 7,
                    "resource_lock_wait_ms": 13,
                    "resource_lock_group": "cargo-target:fixture",
                    "deduplicated": true,
                    "attached_to_session_id": "session-1",
                    "heartbeat": true,
                    "heartbeat_ms": 10000,
                    "stdout": "done\n",
                    "stderr": "",
                    "warnings": ["example"],
                    "returned_count": 3
                },
                "isError": false
            }
        });
        let request_timing = ToolRequestTiming {
            previous_response_completed_ts_ms: Some(250),
            orchestration_gap_ms: Some(750),
            activity_burst_id: 3,
            activity_burst_sequence: 4,
            concurrent_request: false,
        };

        let record = build_tool_usage_record(&ToolUsageInput {
            profile_id: "workspace",
            transport_mode: "streamable-http",
            protocol_version: "2025-11-25",
            request_id: &json!(7),
            method: "tools/call",
            tool_name: "exec_command",
            arguments: &arguments,
            request_json_bytes: 123,
            rpc_fast_path: false,
            request_timing: &request_timing,
            started_ts_ms: 1_000,
            duration_ms: 25,
            outcome: "success",
            response: Some(&response),
            worker_error: None,
            host_session_key: None,
            redact_telemetry: true,
        });

        assert_eq!(record["schema_version"], 7);
        assert_eq!(record["diagnostic_schema_version"], 1);
        assert_eq!(record["event_type"], "tool_call");
        assert_eq!(record["host_kind"], "rust_desktop");
        assert_eq!(record["host_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(record["severity"], "info");
        assert_eq!(record["failure_domain"], "none");
        assert!(record["runtime_boot_id"].as_str().is_some());
        assert_eq!(record["rpc_fast_path"], false);
        assert_eq!(record["tool_family"], "process");
        assert_eq!(record["mutating_tool"], true);
        assert_eq!(record["deprecated_tool"], false);
        assert!(record["call_sequence"].as_u64().is_some());
        assert_eq!(record["duration_ms"], 25);
        assert_eq!(record["phase_preflight_ms"], 4);
        assert_eq!(record["phase_plan_ms"], 2);
        assert_eq!(record["phase_commit_ms"], 5);
        assert_eq!(record["phase_total_ms"], 11);
        assert_eq!(record["operation_lock_wait_ms"], 7);
        assert_eq!(record["resource_lock_wait_ms"], 13);
        assert_eq!(record["resource_lock_group"], "cargo-target:fixture");
        assert_eq!(record["deduplicated"], true);
        assert_eq!(record["attached_to_session_id"], "session-1");
        assert_eq!(record["heartbeat"], true);
        assert_eq!(record["heartbeat_ms"], 10_000);
        assert_eq!(record["previous_response_completed_ts_ms"], 250);
        assert_eq!(record["orchestration_gap_ms"], 750);
        assert_eq!(record["activity_burst_id"], 3);
        assert_eq!(record["activity_burst_sequence"], 4);
        assert_eq!(record["concurrent_request"], false);
        assert_eq!(record["command_kind"], "cargo_test");
        assert_eq!(record["request_json_bytes"], 123);
        assert_eq!(record["response_text_bytes"], 4);
        assert_eq!(record["stdout_bytes"], 5);
        assert_eq!(record["warning_count"], 1);
        assert_eq!(record["notice_count"], 1);
        assert_eq!(record["deprecation_count"], 0);
        assert_eq!(record["returned_count"], 3);
        assert_eq!(record["schema_version"], 7);
        assert_eq!(record["arguments"]["stdin"], "[REDACTED]");
        assert!(record["command_preview"]
            .as_str()
            .expect("command preview")
            .contains("--token [REDACTED]"));
        assert!(!record.to_string().contains("hidden-value"));
        assert!(!record.to_string().contains("do-not-log"));
    }

    #[test]
    fn rpc_error_tool_usage_is_a_transport_event() {
        let request_timing = ToolRequestTiming {
            previous_response_completed_ts_ms: None,
            orchestration_gap_ms: None,
            activity_burst_id: 9,
            activity_burst_sequence: 1,
            concurrent_request: false,
        };
        let arguments = json!({});
        let response = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "error": { "code": -32603, "message": "internal error" }
        });
        let record = build_tool_usage_record(&ToolUsageInput {
            profile_id: "workspace",
            transport_mode: "streamable-http",
            protocol_version: "2026-07-28",
            request_id: &json!(9),
            method: "tools/call",
            tool_name: "server_info",
            arguments: &arguments,
            request_json_bytes: 10,
            rpc_fast_path: false,
            request_timing: &request_timing,
            started_ts_ms: 3_000,
            duration_ms: 2,
            outcome: "rpc_error",
            response: Some(&response),
            worker_error: None,
            host_session_key: None,
            redact_telemetry: true,
        });
        assert_eq!(record["event"], "tool_call");
        assert_eq!(record["event_type"], "transport_event");
        assert_eq!(record["failure_domain"], "transport");
        assert_eq!(record["severity"], "error");
        assert_eq!(record["timestamp_ms"], 3_000);
        assert_eq!(record["conversation_operation_id"], Value::Null);
    }

    #[test]
    fn tool_usage_correlates_system_harness_operation_without_user_operation_id() {
        let request_timing = ToolRequestTiming::default();
        let arguments = json!({ "operation_id": "user-supplied-id" });
        let response = json!({
            "jsonrpc": "2.0",
            "id": 10,
            "result": {
                "structuredContent": {
                    "ok": true,
                    "harness_operation_id": "harness-op-1",
                    "resolved_workspace_id": "folder-a"
                }
            }
        });
        let record = build_tool_usage_record(&ToolUsageInput {
            profile_id: "workspace",
            transport_mode: "streamable-http",
            protocol_version: "2026-07-28",
            request_id: &json!(10),
            method: "tools/call",
            tool_name: "server_info",
            arguments: &arguments,
            request_json_bytes: 10,
            rpc_fast_path: false,
            request_timing: &request_timing,
            started_ts_ms: 4_000,
            duration_ms: 2,
            outcome: "success",
            response: Some(&response),
            worker_error: None,
            host_session_key: None,
            redact_telemetry: true,
        });
        assert_eq!(record["timestamp_ms"], 4_000);
        assert_eq!(record["conversation_operation_id"], "harness-op-1");
        assert_eq!(record["selected_workspace_id"], "folder-a");
        assert_ne!(record["conversation_operation_id"], "user-supplied-id");
    }

    #[test]
    fn recovery_event_payload_links_without_copying_tool_payloads() {
        let record = json!({
            "recovery_attempt": true,
            "retry_of_call_sequence": 7,
            "recovery_action_id": "candidate-1",
            "arguments": { "path": "do-not-copy.txt" },
            "stdout": "do-not-copy-output"
        });
        let fields = recovery_event_fields(&record, "event-1", "edit_file", "success")
            .expect("recovery fields");
        assert_eq!(fields["linked_diagnostic_event_id"], "event-1");
        assert_eq!(fields["tool"], "edit_file");
        assert_eq!(fields["retry_of_call_sequence"], 7);
        assert_eq!(fields["recovery_action_id"], "candidate-1");
        assert_eq!(fields["recovery_succeeded"], true);
        assert!(fields.get("arguments").is_none());
        assert!(fields.get("stdout").is_none());
        assert!(!fields.to_string().contains("do-not-copy"));
    }

    #[test]
    fn exec_many_usage_record_keeps_bounded_parallel_statistics() {
        let request_timing = ToolRequestTiming {
            previous_response_completed_ts_ms: None,
            orchestration_gap_ms: None,
            activity_burst_id: 8,
            activity_burst_sequence: 1,
            concurrent_request: false,
        };
        let arguments = json!({"mode": "auto", "commands": [{"program": "cargo"}]});
        let response = json!({
            "result": {
                "content": [{"type": "text", "text": "batch complete"}],
                "structuredContent": {
                    "ok": true,
                    "mode": "parallel",
                    "requested_mode": "auto",
                    "auto_selected": true,
                    "parallel_decision_source": "historical_statistics",
                    "parallel_confidence": 0.812,
                    "parallel_history_samples": 12,
                    "parallel_blocked_pair_count": 0,
                    "recommended_max_parallel": 4,
                    "parallel_observation_count": 1,
                    "parallelism_observation_truncated": false,
                    "parallel_decision_reasons": ["statistically supported"],
                    "parallelism_observations": [{
                        "pair": "cargo:test@abc|node:test@def",
                        "left": "cargo:test@abc",
                        "right": "node:test@def",
                        "outcome": "success",
                        "overlap_ms": 500,
                        "lock_wait_ms": 0,
                        "same_lock_group": false
                    }]
                },
                "isError": false
            }
        });
        let record = build_tool_usage_record(&ToolUsageInput {
            profile_id: "workspace",
            transport_mode: "streamable-http",
            protocol_version: "2025-11-25",
            request_id: &json!(8),
            method: "tools/call",
            tool_name: "exec_many",
            arguments: &arguments,
            request_json_bytes: 100,
            rpc_fast_path: false,
            request_timing: &request_timing,
            started_ts_ms: 2_000,
            duration_ms: 600,
            outcome: "success",
            response: Some(&response),
            worker_error: None,
            host_session_key: None,
            redact_telemetry: true,
        });
        assert_eq!(record["schema_version"], 7);
        assert_eq!(record["mode"], "parallel");
        assert_eq!(record["parallel_decision_source"], "historical_statistics");
        assert_eq!(record["parallel_history_samples"], 12);
        assert_eq!(record["parallelism_observations"][0]["outcome"], "success");
        assert_eq!(
            record["parallel_decision_reasons"][0],
            "statistically supported"
        );
    }

    #[test]
    fn usage_record_extracts_format_metrics_and_mutation_intent() {
        let request_timing = ToolRequestTiming {
            previous_response_completed_ts_ms: None,
            orchestration_gap_ms: None,
            activity_burst_id: 1,
            activity_burst_sequence: 1,
            concurrent_request: false,
        };
        let response = json!({
            "result": {
                "content": [{"type": "text", "text": "formatted"}],
                "structuredContent": {
                    "ok": true,
                    "status": "checked",
                    "mode": "check",
                    "scope": "changed",
                    "files_requested": 5,
                    "files_supported": 4,
                    "files_changed_count": 2,
                    "files_unchanged_count": 2,
                    "files_skipped_count": 1,
                    "formatter_group_count": 3,
                    "custom_formatter_group_count": 1,
                    "unavailable_adapters": ["black"],
                    "unexpected_changes": [],
                    "diff_bytes": 120,
                    "diff_truncated": false,
                    "applied": false
                },
                "isError": false
            }
        });
        let check_arguments = json!({"scope": "changed", "mode": "check"});
        let check = build_tool_usage_record(&ToolUsageInput {
            profile_id: "workspace",
            transport_mode: "streamable-http",
            protocol_version: "2025-11-25",
            request_id: &json!(1),
            method: "tools/call",
            tool_name: "format_files",
            arguments: &check_arguments,
            request_json_bytes: 32,
            rpc_fast_path: false,
            request_timing: &request_timing,
            started_ts_ms: 1,
            duration_ms: 10,
            outcome: "success",
            response: Some(&response),
            worker_error: None,
            host_session_key: None,
            redact_telemetry: true,
        });
        assert_eq!(check["schema_version"], 7);
        assert_eq!(check["tool_family"], "quality");
        assert_eq!(check["mutating_tool"], false);
        assert_eq!(check["format_mode"], "check");
        assert_eq!(check["format_scope"], "changed");
        assert_eq!(check["format_files_changed_count"], 2);
        assert_eq!(check["format_files_skipped_count"], 1);
        assert_eq!(check["format_custom_formatter_group_count"], 1);
        assert_eq!(check["format_unavailable_adapter_count"], 1);
        assert_eq!(check["format_unexpected_change_count"], 0);
        assert_eq!(check["format_diff_bytes"], 120);

        let apply_arguments = json!({"paths": ["src/lib.rs"], "mode": "apply"});
        let apply = build_tool_usage_record(&ToolUsageInput {
            profile_id: "workspace",
            transport_mode: "streamable-http",
            protocol_version: "2025-11-25",
            request_id: &json!(2),
            method: "tools/call",
            tool_name: "format_files",
            arguments: &apply_arguments,
            request_json_bytes: 32,
            rpc_fast_path: false,
            request_timing: &request_timing,
            started_ts_ms: 1,
            duration_ms: 10,
            outcome: "success",
            response: None,
            worker_error: None,
            host_session_key: None,
            redact_telemetry: true,
        });
        assert_eq!(apply["mutating_tool"], true);
    }
}
