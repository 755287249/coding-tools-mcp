use serde_json::{json, Value};
use tauri::State;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};

fn profile_id(state: &AppState, id: &str) -> AppResult<String> {
    state.with_workspaces(|store| {
        store
            .get(id)
            .map(|profile| profile.id.clone())
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })
}

/// Return redacted, aggregated MCP operation telemetry for the desktop UI.
/// Payloads remain omitted; the shared query implementation only exposes the
/// sanitized previews and metrics already used by the MCP query tool.
#[tauri::command(async)]
pub fn read_workspace_telemetry(
    state: State<'_, AppState>,
    id: String,
    limit: Option<u64>,
    errors_only: Option<bool>,
    min_duration_ms: Option<u64>,
    since_ts_ms: Option<u64>,
) -> AppResult<Value> {
    let profile_id = profile_id(&state, &id)?;
    let args = json!({
        "limit": limit.unwrap_or(100).clamp(1, 200),
        "top": 20,
        "include_records": true,
        "include_payloads": false,
        "aggregate": true,
        "include_slowest": true,
        "include_largest": false,
        "include_performance": true,
        "include_bursts": true,
        "include_async_sessions": true,
        "errors_only": errors_only.unwrap_or(false),
        "min_duration_ms": min_duration_ms.unwrap_or(0),
        "since_ts_ms": since_ts_ms.unwrap_or(0),
    });
    crate::tools::tool_usage::query_tool_usage_for_profile(&profile_id, &args)
        .map_err(|error| AppError::Message(error.message()))
}

