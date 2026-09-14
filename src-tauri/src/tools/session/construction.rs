use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::process::Child;
use tokio::sync::{watch, Mutex as AsyncMutex, OwnedSemaphorePermit};
use uuid::Uuid;

use crate::tools::process_child::ProcessChild;

use super::output::ProcessOutputStream;
use super::{EventState, ExecSession};

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

impl ExecSession {
    pub fn new(child: Child) -> Self {
        Self::new_with_mode_and_checks(ProcessChild::from_tokio(child), false, false)
    }

    pub fn new_with_mode(child: Child, interactive: bool) -> Self {
        Self::new_with_mode_and_checks(ProcessChild::from_tokio(child), interactive, false)
    }

    pub(crate) fn new_with_mode_and_checks(
        mut child: ProcessChild,
        interactive: bool,
        has_post_checks: bool,
    ) -> Self {
        let session_id = Uuid::new_v4().to_string();
        let process_id = child.id();
        let process_tree_contained = child.process_tree_contained();
        let kill_hook = child.kill_hook();
        let stdin = child.take_stdin();
        let stdin_open = stdin.is_some();
        let (change_tx, _) = watch::channel(0u64);
        Self {
            session_id,
            child: AsyncMutex::new(child),
            process_id,
            process_tree_contained,
            kill_hook,
            stdin: AsyncMutex::new(stdin),
            stdin_open: Mutex::new(stdin_open),
            interactive,
            stdout: Mutex::new(ProcessOutputStream::default()),
            stderr: Mutex::new(ProcessOutputStream::default()),
            events: Mutex::new(EventState::default()),
            change_generation: AtomicU64::new(0),
            change_tx,
            exit_waiter_started: AtomicBool::new(false),
            started_at: Instant::now(),
            first_output_at: Mutex::new(None),
            sensitive_output: AtomicBool::new(false),
            exit_code: Mutex::new(None),
            exited: AtomicBool::new(false),
            termination_reason: Mutex::new(None),
            reader_tasks: AsyncMutex::new(Vec::new()),
            post_checks_pending: AtomicBool::new(has_post_checks),
            post_check_result: Mutex::new(None),
            finalized: AtomicBool::new(false),
            finalized_at: Mutex::new(None),
            active_slot: Mutex::new(None),
            telemetry_profile_id: None,
            telemetry_command_kind: "process".to_string(),
            started_ts_ms: unix_timestamp_ms(),
            process_timeout: None,
            process_deadline_ts_ms: None,
            process_deadline: None,
            operation_id: None,
            harness_operations: Mutex::new(Vec::new()),
            harness_operation_recorded: Mutex::new(HashSet::new()),
            command_fingerprint: None,
            resource_lock_group: None,
            resource_lock_target: None,
            operation_lock_wait_ms: 0,
            resource_lock_wait_ms: 0,
            sandbox_prepare_ms: None,
            sandbox_startup_ms: None,
            sandbox_cleanup_ms: Mutex::new(None),
            attachment_generation: AtomicU64::new(1),
            detached_generation: AtomicU64::new(0),
        }
    }

    pub(crate) fn with_process_timeout(
        mut self,
        contract: crate::tools::execution_timeout::ProcessTimeoutContract,
        start: Instant,
    ) -> Self {
        self.started_at = start;
        self.started_ts_ms = unix_timestamp_ms().saturating_sub(start.elapsed().as_millis() as u64);
        self.process_deadline_ts_ms = Some(
            self.started_ts_ms
                .saturating_add(contract.effective_timeout_ms),
        );
        self.process_deadline =
            Some(start + std::time::Duration::from_millis(contract.effective_timeout_ms));
        self.process_timeout = Some(contract);
        self
    }

    pub fn with_execution_identity(
        mut self,
        operation_id: Option<String>,
        command_fingerprint: String,
        resource_lock_group: Option<String>,
        resource_lock_target: Option<String>,
        operation_lock_wait_ms: u128,
        resource_lock_wait_ms: u128,
    ) -> Self {
        self.operation_id = operation_id;
        self.command_fingerprint = Some(command_fingerprint);
        self.resource_lock_group = resource_lock_group;
        self.resource_lock_target = resource_lock_target;
        self.operation_lock_wait_ms = operation_lock_wait_ms;
        self.resource_lock_wait_ms = resource_lock_wait_ms;
        self
    }

    pub(crate) fn with_sandbox_phase_durations(
        mut self,
        prepare_ms: Option<u128>,
        startup_ms: Option<u128>,
    ) -> Self {
        self.sandbox_prepare_ms = prepare_ms;
        self.sandbox_startup_ms = startup_ms;
        self
    }

    pub fn with_active_slot(self, permit: OwnedSemaphorePermit) -> Self {
        *self.active_slot.lock().expect("active slot lock") = Some(permit);
        self
    }

    pub fn with_sensitive_output(self, sensitive: bool) -> Self {
        self.sensitive_output.store(sensitive, Ordering::Release);
        self
    }

    pub fn with_telemetry(mut self, profile_id: &str, command_kind: &str) -> Self {
        self.telemetry_profile_id = Some(profile_id.to_string());
        self.telemetry_command_kind = command_kind.to_string();
        self
    }
}
