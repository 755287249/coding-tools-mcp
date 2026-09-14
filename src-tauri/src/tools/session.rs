mod attachment;
mod construction;
mod control;
mod lifecycle;
mod output;
mod process_lifecycle;
mod registry;
mod snapshot;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use tokio::sync::{watch, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

use crate::harness::{model::OperationRecord, Harness};
use crate::tools::process_child::{ProcessChild, ProcessKillHook, ProcessStdin};
use crate::tools::workspace::WorkspaceError;

pub use output::{EventDetail, OutputMode, OutputOptions};
use output::{OutputEvent, ProcessOutputSnapshot, ProcessOutputStream};

#[cfg(test)]
use output::{
    align_output_start, bounded_output_end, complete_output_boundary, decode_output_event,
    decode_process_output, decode_process_output_with_encoding, process_output_prefix,
    summarize_stream, trim_process_buffer, truncate_tail, ProcessOutputEncoding,
};

const SESSION_EVENT_BYTES: usize = 1_048_576;
pub const DEFAULT_ACTIVE_SESSION_LIMIT: usize = 512;
pub(crate) const WAIT_COMMAND_TRANSPORT_SAFE_MS: u64 = 20_000;
pub(crate) const WAIT_COMMAND_TIMEOUT_DEFAULT_MS: u64 = WAIT_COMMAND_TRANSPORT_SAFE_MS;
pub(crate) const WAIT_COMMAND_TIMEOUT_MAX_MS: u64 = 60 * 60_000;
const MAX_RETAINED_FINALIZED_SESSIONS: usize = 128;
const FINALIZED_SESSION_RETENTION: Duration = Duration::from_secs(60 * 60);
#[cfg(not(test))]
pub const DETACHED_SESSION_GRACE: Duration = Duration::from_secs(90);
#[cfg(test)]
pub const DETACHED_SESSION_GRACE: Duration = Duration::from_millis(250);

#[derive(Default)]
struct SessionRegistry {
    sessions: HashMap<String, Arc<ExecSession>>,
    operation_index: HashMap<String, String>,
    fingerprint_index: HashMap<String, String>,
}

pub struct SessionStore {
    registry: Mutex<SessionRegistry>,
    active_slots: Arc<Semaphore>,
    active_session_limit: usize,
}

#[derive(Debug)]
struct EventState {
    next_sequence: u64,
    retained_bytes: usize,
    events: VecDeque<OutputEvent>,
}

impl Default for EventState {
    fn default() -> Self {
        Self {
            next_sequence: 1,
            retained_bytes: 0,
            events: VecDeque::new(),
        }
    }
}

#[derive(Clone)]
struct HarnessOperationTracking {
    harness: Harness,
    operation: OperationRecord,
}

pub struct ExecSession {
    pub session_id: String,
    pub(crate) child: AsyncMutex<ProcessChild>,
    process_id: Option<u32>,
    process_tree_contained: bool,
    kill_hook: Option<ProcessKillHook>,
    pub stdin: AsyncMutex<Option<ProcessStdin>>,
    stdin_open: Mutex<bool>,
    interactive: bool,
    stdout: Mutex<ProcessOutputStream>,
    stderr: Mutex<ProcessOutputStream>,
    events: Mutex<EventState>,
    change_generation: AtomicU64,
    change_tx: watch::Sender<u64>,
    exit_waiter_started: AtomicBool,
    pub started_at: Instant,
    first_output_at: Mutex<Option<Instant>>,
    sensitive_output: AtomicBool,
    pub exit_code: Mutex<Option<i32>>,
    exited: AtomicBool,
    termination_reason: Mutex<Option<String>>,
    reader_tasks: AsyncMutex<Vec<tokio::task::JoinHandle<()>>>,
    post_checks_pending: AtomicBool,
    post_check_result: Mutex<Option<Value>>,
    finalized: AtomicBool,
    finalized_at: Mutex<Option<Instant>>,
    active_slot: Mutex<Option<OwnedSemaphorePermit>>,
    telemetry_profile_id: Option<String>,
    telemetry_command_kind: String,
    started_ts_ms: u64,
    process_timeout: Option<crate::tools::execution_timeout::ProcessTimeoutContract>,
    process_deadline_ts_ms: Option<u64>,
    process_deadline: Option<Instant>,
    operation_id: Option<String>,
    harness_operations: Mutex<Vec<HarnessOperationTracking>>,
    harness_operation_recorded: Mutex<HashSet<String>>,
    command_fingerprint: Option<String>,
    resource_lock_group: Option<String>,
    resource_lock_target: Option<String>,
    operation_lock_wait_ms: u128,
    resource_lock_wait_ms: u128,
    sandbox_prepare_ms: Option<u128>,
    sandbox_startup_ms: Option<u128>,
    sandbox_cleanup_ms: Mutex<Option<u128>>,
    attachment_generation: AtomicU64,
    detached_generation: AtomicU64,
}

impl ExecSession {
    fn notify_change(&self) {
        let generation = self.change_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.change_tx.send_replace(generation);
    }

    fn has_sensitive_output(&self) -> bool {
        self.sensitive_output.load(Ordering::Acquire)
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub fn is_finalized(&self) -> bool {
        self.finalized.load(Ordering::Acquire)
    }

    fn finalized_at(&self) -> Option<Instant> {
        *self.finalized_at.lock().expect("finalized_at lock")
    }

    pub fn post_checks_pending(&self) -> bool {
        self.post_checks_pending.load(Ordering::Acquire)
    }

    pub fn mark_termination_reason(&self, reason: &str) {
        *self.termination_reason.lock().expect("termination lock") = Some(reason.to_string());
        self.notify_change();
    }

    pub fn complete_post_checks(&self, result: Value) {
        *self.post_check_result.lock().expect("post check lock") = Some(result);
        self.post_checks_pending.store(false, Ordering::Release);
        lifecycle::finish_session(self);
    }

    pub fn mark_finalized(&self) {
        lifecycle::finish_session(self);
    }

    pub(crate) fn mark_stdin_closed(&self) {
        *self.stdin_open.lock().expect("stdin_open lock") = false;
    }

    pub(crate) fn set_sandbox_cleanup_ms(&self, duration_ms: u128) {
        *self
            .sandbox_cleanup_ms
            .lock()
            .expect("sandbox cleanup timing lock") = Some(duration_ms);
        self.notify_change();
    }

    pub(crate) async fn release_backend_lifetimes(&self) {
        self.child.lock().await.release_backend_lifetimes();
    }

    pub async fn is_running(&self) -> bool {
        self.refresh_status().await;
        !self.has_exited()
    }

    pub fn latest_cursor(&self) -> u64 {
        let state = self.events.lock().expect("events lock");
        state.next_sequence.saturating_sub(1)
    }

    fn stream_snapshot(&self, stream: &str) -> ProcessOutputSnapshot {
        snapshot::capture_stream_snapshot(self, stream)
    }

    pub fn retained_stream_bytes(&self, stream: &str) -> (Vec<u8>, usize) {
        snapshot::read_retained_stream_bytes(self, stream)
    }

    pub fn summary(&self) -> Value {
        snapshot::build_summary(self)
    }

    pub fn snapshot(&self, max_output_bytes: usize) -> Value {
        snapshot::build_snapshot(self, max_output_bytes)
    }

    pub fn snapshot_with_options(&self, options: OutputOptions) -> Value {
        snapshot::build_snapshot_with_options(self, options)
    }
}

pub fn read_output(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_read_output(store, args)
}

pub async fn read_output_async(
    store: &SessionStore,
    args: &Value,
) -> Result<Value, WorkspaceError> {
    control::run_read_output_async(store, args).await
}

pub fn resolve_operation(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_resolve_operation(store, args)
}

pub async fn resolve_operation_async(
    store: &SessionStore,
    args: &Value,
) -> Result<Value, WorkspaceError> {
    control::run_resolve_operation_async(store, args).await
}

pub fn list_sessions(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_list_sessions(store, args)
}

pub fn wait_command(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_wait_command(store, args)
}

pub async fn wait_command_async(
    store: &SessionStore,
    args: &Value,
) -> Result<Value, WorkspaceError> {
    control::run_wait_command_async(store, args).await
}

pub fn send_input(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_send_input(store, args)
}

pub async fn send_input_async(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_send_input_async(store, args).await
}

pub fn kill_session(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    control::run_kill_session(store, args)
}

pub async fn kill_session_async(
    store: &SessionStore,
    args: &Value,
) -> Result<Value, WorkspaceError> {
    control::run_kill_session_async(store, args).await
}

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;
