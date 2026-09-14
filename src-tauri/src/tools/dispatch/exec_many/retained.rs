use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::tools::context::SharedToolContext;
use crate::tools::session;
use crate::tools::workspace::{tool_err, tool_ok, WorkspaceError};

use super::{
    exec_many_mode, exec_many_result_mode, execute_exec_many_async, parse_exec_batch_commands,
    shape_exec_many_results, validate_exec_batch_dag,
};

const RETAINED_GRAPH_MAX: usize = 128;
const RETAINED_GRAPH_RETENTION_MS: u64 = 60 * 60_000;
const EXEC_MANY_REQUESTED_YIELD_MAX_MS: u64 = 300_000;
pub(super) const EXEC_MANY_TRANSPORT_SAFE_YIELD_MS: u64 = 20_000;

#[derive(Default)]
struct RetainedExecManyState {
    final_result: Option<Value>,
    completed_at_ms: Option<u64>,
    cancel_requested_at_ms: Option<u64>,
    cancel_reason: Option<String>,
    active_session_ids: HashSet<String>,
}

pub(super) struct RetainedExecManyGraph {
    id: String,
    fingerprint: String,
    created_at_ms: u64,
    command_ids: Vec<String>,
    state: Mutex<RetainedExecManyState>,
    notify: Notify,
}

impl RetainedExecManyGraph {
    fn new(id: String, fingerprint: String, command_ids: Vec<String>) -> Self {
        Self {
            id,
            fingerprint,
            created_at_ms: unix_timestamp_ms(),
            command_ids,
            state: Mutex::new(RetainedExecManyState::default()),
            notify: Notify::new(),
        }
    }

    fn is_completed(&self) -> bool {
        self.state
            .lock()
            .expect("retained exec_many graph state")
            .completed_at_ms
            .is_some()
    }

    fn completed_at_ms(&self) -> Option<u64> {
        self.state
            .lock()
            .expect("retained exec_many graph state")
            .completed_at_ms
    }

    fn complete(&self, result: Value) {
        let mut state = self.state.lock().expect("retained exec_many graph state");
        if state.completed_at_ms.is_some() {
            return;
        }
        state.final_result = Some(result);
        state.completed_at_ms = Some(unix_timestamp_ms());
        state.active_session_ids.clear();
        drop(state);
        self.notify.notify_waiters();
    }

    pub(super) fn register_session(&self, session_id: &str) -> bool {
        let mut state = self.state.lock().expect("retained exec_many graph state");
        state.active_session_ids.insert(session_id.to_string());
        state.cancel_requested_at_ms.is_some()
    }

    pub(super) fn unregister_session(&self, session_id: &str) {
        self.state
            .lock()
            .expect("retained exec_many graph state")
            .active_session_ids
            .remove(session_id);
    }

    fn request_cancel(&self, reason: String) -> Vec<String> {
        let mut state = self.state.lock().expect("retained exec_many graph state");
        if state.cancel_requested_at_ms.is_none() {
            state.cancel_requested_at_ms = Some(unix_timestamp_ms());
            state.cancel_reason = if reason.is_empty() {
                None
            } else {
                Some(reason)
            };
        }
        state.active_session_ids.iter().cloned().collect()
    }

    pub(super) fn cancel_requested(&self) -> bool {
        self.state
            .lock()
            .expect("retained exec_many graph state")
            .cancel_requested_at_ms
            .is_some()
    }
}

type ProfileGraphs = HashMap<String, Arc<RetainedExecManyGraph>>;
static RETAINED_GRAPHS: OnceLock<Mutex<HashMap<String, ProfileGraphs>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, ProfileGraphs>> {
    RETAINED_GRAPHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn prune_profile_graphs(profile_id: &str) -> usize {
    let now = unix_timestamp_ms();
    let mut registry = registry().lock().expect("retained exec_many registry");
    let Some(graphs) = registry.get_mut(profile_id) else {
        return 0;
    };
    let before = graphs.len();
    graphs.retain(|_, graph| {
        graph
            .completed_at_ms()
            .map(|completed| now.saturating_sub(completed) <= RETAINED_GRAPH_RETENTION_MS)
            .unwrap_or(true)
    });
    let removed = before.saturating_sub(graphs.len());
    if graphs.is_empty() {
        registry.remove(profile_id);
    }
    removed
}

fn graph_count(profile_id: &str) -> usize {
    registry()
        .lock()
        .expect("retained exec_many registry")
        .get(profile_id)
        .map(HashMap::len)
        .unwrap_or(0)
}

fn lookup_graph(profile_id: &str, operation_id: &str) -> Option<Arc<RetainedExecManyGraph>> {
    registry()
        .lock()
        .expect("retained exec_many registry")
        .get(profile_id)
        .and_then(|graphs| graphs.get(operation_id).cloned())
}

fn insert_graph(profile_id: &str, graph: Arc<RetainedExecManyGraph>) -> Result<(), WorkspaceError> {
    let mut registry = registry().lock().expect("retained exec_many registry");
    let graphs = registry.entry(profile_id.to_string()).or_default();
    if graphs.len() >= RETAINED_GRAPH_MAX {
        return Err(WorkspaceError::ToolDetails {
            code: "COMMAND_GRAPH_LIMIT_REACHED",
            message: "Too many retained exec_many graph operations are active or awaiting expiry."
                .into(),
            category: "resource",
            retryable: true,
            details: json!({
                "retained_graph_count": graphs.len(),
                "retained_graph_limit": RETAINED_GRAPH_MAX,
                "retention_seconds": RETAINED_GRAPH_RETENTION_MS / 1000
            }),
        });
    }
    if graphs.contains_key(&graph.id) {
        return Err(WorkspaceError::ToolDetails {
            code: "OPERATION_ID_CONFLICT",
            message: "operation_id is already bound to another exec_many graph.".into(),
            category: "conflict",
            retryable: false,
            details: json!({"operation_id": graph.id}),
        });
    }
    graphs.insert(graph.id.clone(), graph);
    Ok(())
}

fn remove_graph(profile_id: &str, operation_id: &str) -> bool {
    let mut registry = registry().lock().expect("retained exec_many registry");
    let removed = registry
        .get_mut(profile_id)
        .and_then(|graphs| graphs.remove(operation_id))
        .is_some();
    if registry
        .get(profile_id)
        .map(HashMap::is_empty)
        .unwrap_or(false)
    {
        registry.remove(profile_id);
    }
    removed
}

fn semantic_fingerprint(args: &Value) -> String {
    let mut semantic = args.clone();
    if let Some(object) = semantic.as_object_mut() {
        for key in [
            "operation_id",
            "action",
            "yield_time_ms",
            "result_mode",
            "reason",
        ] {
            object.remove(key);
        }
    }
    let bytes = serde_json::to_vec(&semantic).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn graph_action(args: &Value) -> Result<&str, WorkspaceError> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("run");
    match action {
        "run" | "status" | "cancel" | "forget" => Ok(action),
        _ => Err(WorkspaceError::invalid_argument(
            "exec_many action must be run, status, cancel, or forget",
        )),
    }
}

fn requested_operation_id(args: &Value) -> Option<String> {
    args.get("operation_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn yield_budget(args: &Value) -> Result<(u64, u64), WorkspaceError> {
    let requested = match args.get("yield_time_ms") {
        Some(value) => value.as_u64().ok_or_else(|| {
            WorkspaceError::invalid_argument("yield-time_ms must be a non-negative integer")
        })?,
        None => EXEC_MANY_TRANSPORT_SAFE_YIELD_MS,
    };
    if requested > EXEC_MANY_REQUESTED_YIELD_MAX_MS {
        return Err(WorkspaceError::invalid_argument(
            "yield-time_ms must be <= 300000",
        ));
    }
    Ok((requested, requested.min(EXEC_MANY_TRANSPORT_SAFE_YIELD_MS)))
}

fn result_mode(args: &Value, completed: bool) -> Result<Option<&str>, WorkspaceError> {
    let requested = exec_many_result_mode(args)?;
    if completed {
        Ok(requested)
    } else {
        Ok(requested.or(Some("summary")))
    }
}

fn project_final_result(mut full: Value, requested_mode: Option<&str>) -> Value {
    let results = full
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Ok(shape) = shape_exec_many_results(&results, requested_mode) else {
        return full;
    };
    if let Some(object) = full.as_object_mut() {
        object.insert("result_mode".into(), json!(shape.mode));
        object.insert("result_mode_reason".into(), json!(shape.reason));
        object.insert(
            "auto_compacted_output_bytes".into(),
            json!(shape.auto_compacted_output_bytes),
        );
        object.insert("results_included".into(), json!(shape.mode != "none"));
        object.insert("result_output_included".into(), json!(shape.mode == "full"));
        object.insert(
            "results_omitted_count".into(),
            json!(shape.results_omitted_count),
        );
        object.insert("results".into(), Value::Array(shape.results));
    }
    full
}

async fn wait_for_graph(graph: &RetainedExecManyGraph, effective_yield_ms: u64) -> u128 {
    if graph.is_completed() || effective_yield_ms == 0 {
        return 0;
    }
    let started = Instant::now();
    let notified = graph.notify.notified();
    if graph.is_completed() {
        return 0;
    }
    let _ = tokio::time::timeout(Duration::from_millis(effective_yield_ms), notified).await;
    started.elapsed().as_millis()
}

fn graph_snapshot(
    profile_id: &str,
    graph: &RetainedExecManyGraph,
    action: &str,
    reattached: bool,
    requested_yield_ms: u64,
    effective_yield_ms: u64,
    wait_ms: u128,
    requested_result_mode: Option<&str>,
) -> Value {
    let state = graph.state.lock().expect("retained exec_many graph state");
    let completed = state.completed_at_ms.is_some();
    let cancel_requested = state.cancel_requested_at_ms.is_some();
    let graph_status = if completed && cancel_requested {
        "cancelled"
    } else if completed {
        "completed"
    } else if cancel_requested {
        "cancelling"
    } else {
        "running"
    };
    let completed_at_ms = state.completed_at_ms;
    let cancel_requested_at_ms = state.cancel_requested_at_ms;
    let cancel_reason = state.cancel_reason.clone();
    let active_session_count = state.active_session_ids.len();
    let final_result = state.final_result.clone();
    drop(state);

    let mode = if completed {
        requested_result_mode
    } else {
        requested_result_mode.or(Some("summary"))
    };
    let mut output = if let Some(result) = final_result {
        project_final_result(result, mode)
    } else {
        tool_ok(json!({
            "command_ok": Value::Null,
            "all_commands_ok": Value::Null,
            "graph_execution_ok": Value::Null,
            "graph_progress_ok": true,
            "commands_requested": graph.command_ids.len(),
            "commands_executed": active_session_count,
            "completed_command_count": 0,
            "running_command_count": active_session_count,
            "pending_command_count": graph.command_ids.len().saturating_sub(active_session_count),
            "failed_command_count": 0,
            "skipped_command_count": 0,
            "completed_command_ids": [],
            "running_command_ids": [],
            "pending_command_ids": graph.command_ids,
            "failed_command_ids": [],
            "skipped_command_ids": [],
            "first_failure": Value::Null,
            "scheduler_error": Value::Null,
            "recovery_actions": [],
            "result_mode": mode.unwrap_or("summary"),
            "result_mode_reason": if requested_result_mode.is_some() { "explicit" } else { "running_default" },
            "auto_compacted_output_bytes": 0,
            "results_included": mode != Some("none"),
            "result_output_included": mode == Some("full"),
            "results_omitted_count": if mode == Some("none") { graph.command_ids.len() } else { 0 },
            "results": []
        }))
    };

    let retention_expires_at_ms = completed_at_ms.map(|value| value + RETAINED_GRAPH_RETENTION_MS);
    if let Some(object) = output.as_object_mut() {
        object.insert("operation_id".into(), json!(graph.id));
        object.insert("graph_operation_id".into(), json!(graph.id));
        object.insert("graph_action".into(), json!(action));
        object.insert("graph_status".into(), json!(graph_status));
        object.insert("graph_completed".into(), json!(completed));
        object.insert("terminal".into(), json!(completed));
        object.insert("detached".into(), json!(!completed));
        object.insert("reattached".into(), json!(reattached));
        object.insert("cancel_requested".into(), json!(cancel_requested));
        object.insert(
            "cancel_requested_at_ms".into(),
            json!(cancel_requested_at_ms),
        );
        object.insert("cancel_reason".into(), json!(cancel_reason));
        object.insert("graph_created_ts_ms".into(), json!(graph.created_at_ms));
        object.insert("graph_completed_ts_ms".into(), json!(completed_at_ms));
        object.insert(
            "retention_seconds".into(),
            json!(RETAINED_GRAPH_RETENTION_MS / 1000),
        );
        object.insert(
            "retention_expires_ts_ms".into(),
            json!(retention_expires_at_ms),
        );
        object.insert(
            "retention_remaining_ms".into(),
            json!(
                retention_expires_at_ms.map(|expires| expires.saturating_sub(unix_timestamp_ms()))
            ),
        );
        object.insert("graph_requested_yield_ms".into(), json!(requested_yield_ms));
        object.insert("graph_yield_ms".into(), json!(effective_yield_ms));
        object.insert("graph_wait_ms".into(), json!(wait_ms));
        object.insert(
            "retained_graph_count".into(),
            json!(graph_count(profile_id)),
        );
        object.insert(
            "next_actions".into(),
            if completed {
                json!([])
            } else {
                json!([{
                    "tool": "exec_many",
                    "arguments": {
                        "operation_id": graph.id,
                        "yield_time_ms": EXEC_MANY_TRANSPORT_SAFE_YIELD_MS,
                        "result_mode": "summary"
                    }
                }])
            },
        );
        if action != "run" {
            object.insert("control_ok".into(), json!(true));
        }
    }
    output
}

fn cancelled_result(graph: &RetainedExecManyGraph) -> Value {
    tool_ok(json!({
        "command_ok": false,
        "all_commands_ok": false,
        "graph_execution_ok": false,
        "graph_progress_ok": false,
        "commands_requested": graph.command_ids.len(),
        "commands_executed": 0,
        "successful_command_count": 0,
        "failed_command_count": 0,
        "failed_command_ids": [],
        "skipped_command_count": 0,
        "skipped_command_ids": [],
        "first_failed_command": Value::Null,
        "batch_summary": "exec_many graph cancelled",
        "stopped_early": true,
        "outcome_class": "cancelled",
        "recovery_actions": [],
        "result_mode": "full",
        "result_mode_reason": "retained_cancelled",
        "auto_compacted_output_bytes": 0,
        "results_included": true,
        "result_output_included": true,
        "results_omitted_count": 0,
        "results": []
    }))
}

async fn cancel_graph_sessions(ctx: &SharedToolContext, session_ids: Vec<String>) {
    let mut tasks = tokio::task::JoinSet::new();
    for session_id in session_ids {
        let sessions = ctx.sessions.clone();
        tasks.spawn(async move {
            let _ = session::kill_session_async(
                &sessions,
                &json!({"session_id": session_id, "signal": "TERM", "wait_ms": 5_000}),
            )
            .await;
        });
    }
    while tasks.join_next().await.is_some() {}
}

pub(super) async fn call_exec_many_retained_async(ctx: SharedToolContext, args: &Value) -> Value {
    let action = match graph_action(args) {
        Ok(value) => value,
        Err(error) => return tool_err(error),
    };
    let (requested_yield_ms, effective_yield_ms) = match yield_budget(args) {
        Ok(value) => value,
        Err(error) => return tool_err(error),
    };
    if let Err(error) = exec_many_result_mode(args) {
        return tool_err(error);
    }
    prune_profile_graphs(&ctx.profile_id);

    let operation_id = requested_operation_id(args);
    let commands_present = args
        .get("commands")
        .and_then(Value::as_array)
        .map(|commands| !commands.is_empty())
        .unwrap_or(false);

    if action != "run" {
        if commands_present {
            return tool_err(WorkspaceError::invalid_argument(format!(
                "exec_many action={action} does not accept commands"
            )));
        }
        let Some(operation_id) = operation_id else {
            return tool_err(WorkspaceError::invalid_argument(format!(
                "exec_many action={action} requires a retained operation_id"
            )));
        };
        let Some(graph) = lookup_graph(&ctx.profile_id, &operation_id) else {
            return tool_err(WorkspaceError::ToolDetails {
                code: "COMMAND_GRAPH_OPERATION_NOT_FOUND",
                message: "Retained exec_many graph operation was not found or expired.".into(),
                category: "not_found",
                retryable: false,
                details: json!({
                    "operation_id": operation_id,
                    "retention_seconds": RETAINED_GRAPH_RETENTION_MS / 1000
                }),
            });
        };
        if action == "status" {
            let mode = match result_mode(args, graph.is_completed()) {
                Ok(mode) => mode,
                Err(error) => return tool_err(error),
            };
            return graph_snapshot(
                &ctx.profile_id,
                graph.as_ref(),
                action,
                true,
                requested_yield_ms,
                0,
                0,
                mode,
            );
        }
        if action == "forget" {
            if !graph.is_completed() {
                return tool_err(WorkspaceError::ToolDetails {
                    code: "COMMAND_GRAPH_STILL_RUNNING",
                    message: "Running exec_many graphs must be cancelled or completed before they can be forgotten.".into(),
                    category: "conflict",
                    retryable: true,
                    details: json!({"operation_id": operation_id}),
                });
            }
            remove_graph(&ctx.profile_id, &operation_id);
            return tool_ok(json!({
                "operation_id": operation_id,
                "graph_operation_id": operation_id,
                "graph_action": "forget",
                "graph_status": "forgotten",
                "graph_completed": true,
                "terminal": true,
                "forgotten": true,
                "retained_graph_count": graph_count(&ctx.profile_id),
                "next_actions": []
            }));
        }

        let active_sessions = graph.request_cancel(
            args.get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
        );
        cancel_graph_sessions(&ctx, active_sessions).await;
        let wait_ms = wait_for_graph(graph.as_ref(), effective_yield_ms).await;
        let mode = match result_mode(args, graph.is_completed()) {
            Ok(mode) => mode,
            Err(error) => return tool_err(error),
        };
        return graph_snapshot(
            &ctx.profile_id,
            graph.as_ref(),
            action,
            true,
            requested_yield_ms,
            effective_yield_ms,
            wait_ms,
            mode,
        );
    }

    if let Some(operation_id) = operation_id.as_deref() {
        if let Some(existing) = lookup_graph(&ctx.profile_id, operation_id) {
            if commands_present && semantic_fingerprint(args) != existing.fingerprint {
                return tool_err(WorkspaceError::ToolDetails {
                    code: "OPERATION_ID_CONFLICT",
                    message: "operation_id is already bound to a different exec_many graph.".into(),
                    category: "conflict",
                    retryable: false,
                    details: json!({"operation_id": operation_id}),
                });
            }
            let wait_ms = wait_for_graph(existing.as_ref(), effective_yield_ms).await;
            let mode = match result_mode(args, existing.is_completed()) {
                Ok(mode) => mode,
                Err(error) => return tool_err(error),
            };
            return graph_snapshot(
                &ctx.profile_id,
                existing.as_ref(),
                action,
                true,
                requested_yield_ms,
                effective_yield_ms,
                wait_ms,
                mode,
            );
        }
        if !commands_present {
            return tool_err(WorkspaceError::ToolDetails {
                code: "COMMAND_GRAPH_OPERATION_NOT_FOUND",
                message: "Retained exec_many graph operation was not found or expired.".into(),
                category: "not_found",
                retryable: false,
                details: json!({
                    "operation_id": operation_id,
                    "retention_seconds": RETAINED_GRAPH_RETENTION_MS / 1000
                }),
            });
        }
    }

    let Some(raw_commands) = args.get("commands").and_then(Value::as_array) else {
        return tool_err(WorkspaceError::invalid_argument(
            "commands or a retained operation_id is required",
        ));
    };
    let commands = match parse_exec_batch_commands(raw_commands) {
        Ok(commands) => commands,
        Err(error) => return tool_err(error),
    };
    let requested_mode = match exec_many_mode(args) {
        Ok(mode) => mode,
        Err(error) => return tool_err(error),
    };
    if requested_mode == "dag" {
        if let Err(error) = validate_exec_batch_dag(&commands) {
            return tool_err(error);
        }
    }

    let operation_id = operation_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let graph = Arc::new(RetainedExecManyGraph::new(
        operation_id.clone(),
        semantic_fingerprint(args),
        commands.iter().map(|command| command.id.clone()).collect(),
    ));
    if let Err(error) = insert_graph(&ctx.profile_id, graph.clone()) {
        return tool_err(error);
    }

    let mut execution_args = args.clone();
    if let Some(object) = execution_args.as_object_mut() {
        object.remove("operation_id");
        object.remove("action");
        object.remove("yield-time_ms");
        object.remove("reason");
        object.insert("result_mode".into(), json!("full"));
    }
    let execution_ctx = ctx.clone();
    let worker_graph = graph.clone();
    let worker = tokio::spawn(async move {
        execute_exec_many_async(execution_ctx, &execution_args, Some(worker_graph)).await
    });
    let monitor_graph = graph.clone();
    tokio::spawn(async move {
        let result = match worker.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => cancelled_result(monitor_graph.as_ref()),
            Err(error) => tool_err(WorkspaceError::ToolDetails {
                code: "BATCH_WORKER_FAILED",
                message: error.to_string(),
                category: "runtime",
                retryable: true,
                details: json!({"stage": "exec_many_retained_join"}),
            }),
        };
        monitor_graph.complete(result);
    });

    let wait_ms = wait_for_graph(graph.as_ref(), effective_yield_ms).await;
    let mode = match result_mode(args, graph.is_completed()) {
        Ok(mode) => mode,
        Err(error) => return tool_err(error),
    };
    graph_snapshot(
        &ctx.profile_id,
        graph.as_ref(),
        action,
        false,
        requested_yield_ms,
        effective_yield_ms,
        wait_ms,
        mode,
    )
}
