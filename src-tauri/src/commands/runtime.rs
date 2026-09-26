use tauri::State;

use crate::app_state::AppState;
use crate::application::runtime as core_runtime;
use crate::error::AppResult;
use crate::workspace::RuntimeStatusDto;

#[tauri::command]
pub async fn start_runtime(state: State<'_, AppState>, id: String) -> AppResult<RuntimeStatusDto> {
    core_runtime::start_mcp_runtime(&state, &id).await
}

#[tauri::command]
pub async fn stop_runtime(state: State<'_, AppState>, id: String) -> AppResult<RuntimeStatusDto> {
    core_runtime::stop_mcp_runtime(&state, &id).await
}

#[tauri::command(async)]
pub fn get_runtime_status(state: State<'_, AppState>, id: String) -> AppResult<RuntimeStatusDto> {
    core_runtime::mcp_runtime_status(&state, &id)
}

#[tauri::command]
pub async fn start_actions_runtime(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<RuntimeStatusDto> {
    core_runtime::start_actions_runtime(&state, &id).await
}

#[tauri::command]
pub async fn stop_actions_runtime(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<RuntimeStatusDto> {
    core_runtime::stop_actions_runtime(&state, &id).await
}

#[tauri::command(async)]
pub fn get_actions_runtime_status(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<RuntimeStatusDto> {
    core_runtime::actions_runtime_status(&state, &id)
}

#[tauri::command(async)]
pub fn restart_runtime(state: State<'_, AppState>, id: String) -> AppResult<RuntimeStatusDto> {
    core_runtime::restart_mcp_runtime(&state, &id)
}

#[tauri::command(async)]
pub fn restart_actions_runtime(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<RuntimeStatusDto> {
    core_runtime::restart_actions_runtime(&state, &id)
}
