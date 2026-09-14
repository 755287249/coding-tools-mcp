use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread;

use serde_json::{json, Value};

use crate::tunnel::append_profile_log_rotating_checked;

const DIAGNOSTICS_LOG_FILE: &str = "diagnostics.jsonl";
const TOOL_USAGE_LOG_FILE: &str = "mcp-tool-usage.jsonl";
pub(super) const LEGACY_COMPAT_WRITE_ENV: &str = "CODING_TOOLS_DIAGNOSTICS_LEGACY_WRITE";
const TOOL_USAGE_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;
const TOOL_USAGE_LOG_RETAINED_FILES: usize = 5;
const TOOL_USAGE_LOG_QUEUE_CAPACITY: usize = 1_024;

type ToolUsageLogEntry = (String, String);

static TOOL_USAGE_LOG_SENDER: OnceLock<Option<SyncSender<ToolUsageLogEntry>>> = OnceLock::new();
static TOOL_USAGE_LOG_DROPPED: AtomicU64 = AtomicU64::new(0);
static TOOL_USAGE_LOG_WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);
static LEGACY_LOG_WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);
static TOOL_USAGE_LOG_LAST_SUCCESS_TS_MS: AtomicU64 = AtomicU64::new(0);
static TOOL_USAGE_LOG_LAST_FAILURE_TS_MS: AtomicU64 = AtomicU64::new(0);

fn tool_usage_log_sender() -> Option<&'static SyncSender<ToolUsageLogEntry>> {
    TOOL_USAGE_LOG_SENDER
        .get_or_init(|| {
            let (sender, receiver) =
                sync_channel::<ToolUsageLogEntry>(TOOL_USAGE_LOG_QUEUE_CAPACITY);
            let worker = thread::Builder::new()
                .name("mcp-tool-usage-log".into())
                .spawn(move || {
                    while let Ok((profile_id, line)) = receiver.recv() {
                        write_log_entry(&profile_id, &line);
                    }
                });
            worker.ok().map(|_| sender)
        })
        .as_ref()
}

fn record_write_failure() {
    TOOL_USAGE_LOG_WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
    TOOL_USAGE_LOG_LAST_FAILURE_TS_MS.store(super::unix_timestamp_ms(), Ordering::Relaxed);
}

fn record_write_result(result: std::io::Result<()>) {
    if result.is_ok() {
        TOOL_USAGE_LOG_LAST_SUCCESS_TS_MS.store(super::unix_timestamp_ms(), Ordering::Relaxed);
    } else {
        record_write_failure();
    }
}

fn legacy_compat_write_enabled_from(value: Option<&str>) -> bool {
    let normalized = value.map(str::trim).map(str::to_ascii_lowercase);
    !matches!(normalized.as_deref(), Some("0" | "false" | "no" | "off"))
}

pub(super) fn legacy_compat_write_enabled() -> bool {
    let value = std::env::var(LEGACY_COMPAT_WRITE_ENV).ok();
    legacy_compat_write_enabled_from(value.as_deref())
}

fn write_log_entry(profile_id: &str, line: &str) {
    record_write_result(append_profile_log_rotating_checked(
        profile_id,
        DIAGNOSTICS_LOG_FILE,
        line,
        TOOL_USAGE_LOG_MAX_BYTES,
        TOOL_USAGE_LOG_RETAINED_FILES,
    ));
    if legacy_compat_write_enabled()
        && append_profile_log_rotating_checked(
            profile_id,
            TOOL_USAGE_LOG_FILE,
            line,
            TOOL_USAGE_LOG_MAX_BYTES,
            TOOL_USAGE_LOG_RETAINED_FILES,
        )
        .is_err()
    {
        LEGACY_LOG_WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn tool_usage_log_health() -> Value {
    let last_success = TOOL_USAGE_LOG_LAST_SUCCESS_TS_MS.load(Ordering::Relaxed);
    let last_failure = TOOL_USAGE_LOG_LAST_FAILURE_TS_MS.load(Ordering::Relaxed);
    let write_error_present = last_failure > last_success;
    let writer_state = if write_error_present {
        "degraded"
    } else {
        match TOOL_USAGE_LOG_SENDER.get() {
            None => "not_initialized",
            Some(Some(_)) => "ready",
            Some(None) => "fallback",
        }
    };
    json!({
        "queue_capacity": TOOL_USAGE_LOG_QUEUE_CAPACITY,
        "pending_records": Value::Null,
        "pending_dropped_records": TOOL_USAGE_LOG_DROPPED.load(Ordering::Relaxed),
        "write_failures": TOOL_USAGE_LOG_WRITE_FAILURES.load(Ordering::Relaxed),
        "legacy_compat_write_failures": LEGACY_LOG_WRITE_FAILURES.load(Ordering::Relaxed),
        "write_error_present": write_error_present,
        "last_successful_write_ts_ms": (last_success > 0).then_some(last_success),
        "last_write_failure_ts_ms": (last_failure > 0).then_some(last_failure),
        "writer_state": writer_state
    })
}

pub(super) fn append_tool_usage_log(profile_id: &str, mut record: Value) {
    if let Some(object) = record.as_object_mut() {
        object
            .entry("diagnostic_event_id")
            .or_insert_with(|| json!(uuid::Uuid::new_v4().to_string()));
    }
    let dropped_before = TOOL_USAGE_LOG_DROPPED.swap(0, Ordering::Relaxed);
    if dropped_before > 0 {
        if let Some(object) = record.as_object_mut() {
            object.insert("telemetry_dropped_before".into(), json!(dropped_before));
        }
    }

    let Ok(line) = serde_json::to_string(&record) else {
        TOOL_USAGE_LOG_DROPPED.fetch_add(dropped_before.saturating_add(1), Ordering::Relaxed);
        record_write_failure();
        return;
    };

    let Some(sender) = tool_usage_log_sender() else {
        write_log_entry(profile_id, &line);
        return;
    };

    match sender.try_send((profile_id.to_string(), line)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            TOOL_USAGE_LOG_DROPPED.fetch_add(dropped_before.saturating_add(1), Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected((profile_id, line))) => {
            write_log_entry(&profile_id, &line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::legacy_compat_write_enabled_from;

    #[test]
    fn legacy_compat_write_gate_defaults_on_and_accepts_common_false_values() {
        for value in [
            None,
            Some(""),
            Some("1"),
            Some("true"),
            Some("yes"),
            Some("on"),
        ] {
            assert!(legacy_compat_write_enabled_from(value), "{value:?}");
        }
        for value in [
            Some("0"),
            Some("false"),
            Some("FALSE"),
            Some(" no "),
            Some("off"),
        ] {
            assert!(!legacy_compat_write_enabled_from(value), "{value:?}");
        }
    }
}
