use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::OwnedMutexGuard;

use crate::mcp::classify_command_text;
use crate::tools::context::ToolContext;
use crate::tools::session::{
    EventDetail, ExecSession, OutputMode, OutputOptions, DETACHED_SESSION_GRACE,
};
use crate::tools::workspace::WorkspaceError;

use super::backend::{start_exec_process, CommandExecutionBackend};
use super::identity::ExecutionIdentity;
use super::post_check::run_post_checks;
use super::result::merge_exec_result;
use super::runner::CommandIoMode;
use super::spec::{ExecSpec, PostCheckSpec};

struct RequestCancellationGuard {
    session: Option<Arc<ExecSession>>,
}

impl RequestCancellationGuard {
    fn new(session: Arc<ExecSession>) -> Self {
        Self {
            session: Some(session),
        }
    }

    fn disarm(&mut self) {
        self.session = None;
    }
}

impl Drop for RequestCancellationGuard {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let detached_generation = session.mark_detached();
        crate::task_runtime::spawn(async move {
            tokio::time::sleep(DETACHED_SESSION_GRACE).await;
            if session.is_finalized() || !session.is_still_detached(detached_generation) {
                return;
            }
            session.mark_termination_reason("detached_timeout");
            if session.is_running().await {
                session.kill_and_wait().await;
            }
            // The lifecycle monitor owns sandbox cleanup, post-check completion and
            // finalization. Do not publish a false finalized state from the detached
            // timeout path while those steps are still running.
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_command(
    ctx: &ToolContext,
    backend: CommandExecutionBackend,
    sandbox_prepare_ms: Option<u128>,
    spec: &ExecSpec,
    cwd: &Path,
    limit: Duration,
    yield_time: Duration,
    output_options: OutputOptions,
    tty: bool,
    stdin_text: &str,
    post_checks: Vec<PostCheckSpec>,
    sensitive_output: bool,
    identity: ExecutionIdentity,
    operation_lock_wait_ms: u128,
    operation_guard: Option<OwnedMutexGuard<()>>,
) -> Result<Value, WorkspaceError> {
    let resource_lock_started = Instant::now();
    let resource_guard = if let Some(group) = identity.resource_lock_group.as_deref() {
        Some(ctx.resource_lock(group).lock_owned().await)
    } else {
        None
    };
    let resource_lock_wait_ms = resource_lock_started.elapsed().as_millis();
    let start = Instant::now();

    let active_slot = ctx.sessions.acquire_active_slot().await?;
    let sandboxed = matches!(&backend, CommandExecutionBackend::Sandbox(_));
    let sandbox_startup_started = sandboxed.then(Instant::now);
    let started = start_exec_process(&backend, spec, cwd, CommandIoMode::Session).await?;
    let sandbox_startup_ms =
        sandbox_startup_started.map(|started_at| started_at.elapsed().as_millis());
    let startup_diagnostics = started.diagnostics;
    let session = ctx.sessions.insert(
        ExecSession::new_with_mode_and_checks(started.child, tty, !post_checks.is_empty())
            .with_active_slot(active_slot)
            .with_sensitive_output(sensitive_output)
            .with_telemetry(&ctx.profile_id, classify_command_text(&spec.display))
            .with_sandbox_phase_durations(sandbox_prepare_ms, sandbox_startup_ms)
            .with_process_timeout(
                identity.process_timeout.clone().unwrap_or_else(|| {
                    crate::tools::execution_timeout::ProcessTimeoutContract {
                        execution_mode: "command",
                        requested_timeout_ms: None,
                        effective_timeout_ms: limit.as_millis() as u64,
                        limit_ms: crate::tools::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
                    }
                }),
                start,
            )
            .with_execution_identity(
                identity.operation_id.clone(),
                identity.command_fingerprint.clone(),
                identity.resource_lock_group.clone(),
                identity.resource_lock_target.clone(),
                operation_lock_wait_ms,
                resource_lock_wait_ms,
            ),
    );
    let mut cancellation_guard = RequestCancellationGuard::new(session.clone());
    let initial_cursor = session.latest_cursor();
    session.spawn_readers().await;
    session.spawn_exit_waiter();
    drop(started.startup_guard);

    let deadline = start + limit;
    spawn_lifecycle_monitor(
        session.clone(),
        deadline,
        post_checks,
        cwd.to_path_buf(),
        backend,
        resource_guard,
    );
    drop(operation_guard);

    if !tty && !stdin_text.is_empty() {
        let mut stdin_guard = session.stdin.lock().await;
        if let Some(stdin) = stdin_guard.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(stdin_text.as_bytes())
                .await
                .map_err(|_| WorkspaceError::Tool {
                    code: "SESSION_CLOSED",
                    message: "Failed to write stdin.".into(),
                    category: "runtime",
                    retryable: false,
                })?;
            let _ = stdin.shutdown().await;
        }
        *stdin_guard = None;
        session.mark_stdin_closed();
    }

    if yield_time.is_zero() || tty {
        let snapshot = session.snapshot_with_options(output_options);
        cancellation_guard.disarm();
        return Ok(merge_exec_result(
            snapshot,
            start,
            spec,
            cwd,
            true,
            Some(&startup_diagnostics),
        ));
    }

    let changed = session
        .wait_for_change(initial_cursor, yield_time, "output_or_exit")
        .await;
    session.refresh_status().await;
    if changed && !session.has_exited() {
        let remaining_yield = yield_time.saturating_sub(start.elapsed());
        let quick_exit_grace = remaining_yield.min(Duration::from_millis(500));
        if !quick_exit_grace.is_zero() {
            let _ = session
                .wait_for_change(session.latest_cursor(), quick_exit_grace, "exit")
                .await;
            session.refresh_status().await;
        }
    }
    if session.has_exited() && !session.is_finalized() {
        let remaining_yield = yield_time.saturating_sub(start.elapsed());
        if !remaining_yield.is_zero() {
            let _ = session
                .wait_for_change(session.latest_cursor(), remaining_yield, "finalized")
                .await;
        }
    }
    if session.is_finalized() {
        let snapshot = session.snapshot_with_options(output_options);
        cancellation_guard.disarm();
        Ok(merge_exec_result(
            snapshot,
            start,
            spec,
            cwd,
            false,
            Some(&startup_diagnostics),
        ))
    } else {
        let snapshot = session.snapshot_with_options(output_options);
        cancellation_guard.disarm();
        Ok(merge_exec_result(
            snapshot,
            start,
            spec,
            cwd,
            true,
            Some(&startup_diagnostics),
        ))
    }
}

fn spawn_lifecycle_monitor(
    session: Arc<ExecSession>,
    deadline: Instant,
    post_checks: Vec<PostCheckSpec>,
    cwd: std::path::PathBuf,
    backend: CommandExecutionBackend,
    resource_guard: Option<OwnedMutexGuard<()>>,
) {
    tokio::spawn(async move {
        let _resource_guard = resource_guard;
        let sandboxed = matches!(&backend, CommandExecutionBackend::Sandbox(_));
        tokio::select! {
            _ = session.wait_until_exited() => {}
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                if !session.has_exited() {
                    session.mark_termination_reason("process_timeout");
                    session.kill_and_wait().await;
                }
            }
        }

        session.wait_for_readers().await;
        if post_checks.is_empty() {
            let cleanup_started = sandboxed.then(Instant::now);
            session.release_backend_lifetimes().await;
            drop(backend);
            if let Some(started_at) = cleanup_started {
                session.set_sandbox_cleanup_ms(started_at.elapsed().as_millis());
            }
            session.mark_finalized();
            return;
        }

        let main = session.snapshot_with_options(OutputOptions {
            mode: OutputMode::None,
            event_detail: EventDetail::Full,
            cursor: session.latest_cursor(),
            max_output_bytes: 1,
            tail_lines: 1,
        });
        if main.get("execution_ok").and_then(Value::as_bool) != Some(true) {
            let result = json!({
                "ok": false,
                "configured": post_checks.len(),
                "executed": 0,
                "skipped": true,
                "reason": "main_command_failed",
                "results": []
            });
            let cleanup_started = sandboxed.then(Instant::now);
            session.release_backend_lifetimes().await;
            drop(backend);
            if let Some(started_at) = cleanup_started {
                session.set_sandbox_cleanup_ms(started_at.elapsed().as_millis());
            }
            session.complete_post_checks(result);
            return;
        }

        let post_check_result = run_post_checks(post_checks, &cwd, &backend).await;
        let cleanup_started = sandboxed.then(Instant::now);
        session.release_backend_lifetimes().await;
        drop(backend);
        if let Some(started_at) = cleanup_started {
            session.set_sandbox_cleanup_ms(started_at.elapsed().as_millis());
        }
        session.complete_post_checks(post_check_result);
    });
}
