use std::path::PathBuf;

use serde_json::Value;
use tauri::State;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::workspace::WorkspaceProfile;

fn history_root(profile: &WorkspaceProfile, folder_id: Option<&str>) -> AppResult<PathBuf> {
    if let Some(folder_id) = folder_id {
        let folder = profile
            .folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .ok_or_else(|| AppError::Message(format!("folder not found: {folder_id}")))?;
        return Ok(PathBuf::from(&folder.path));
    }
    if let Some(folder) = profile.active_folder() {
        return Ok(PathBuf::from(&folder.path));
    }
    if !profile.path.trim().is_empty() {
        return Ok(PathBuf::from(&profile.path));
    }
    Err(AppError::Message(
        "workspace has no configured folder".into(),
    ))
}

fn workspace_profile(state: &AppState, id: &str) -> AppResult<WorkspaceProfile> {
    state.with_workspaces(|store| {
        store
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })
}

#[tauri::command(async)]
pub fn list_history_sessions(
    state: State<'_, AppState>,
    id: String,
    folder_id: Option<String>,
) -> AppResult<Value> {
    let profile = workspace_profile(&state, &id)?;
    let root = history_root(&profile, folder_id.as_deref())?;
    crate::tools::history::list_for_ui(&root, Some(&profile.id))
        .map_err(|error| AppError::Message(error.message()))
}

#[tauri::command(async)]
pub fn read_history_session(
    state: State<'_, AppState>,
    id: String,
    number: u64,
    folder_id: Option<String>,
) -> AppResult<Value> {
    let profile = workspace_profile(&state, &id)?;
    let root = history_root(&profile, folder_id.as_deref())?;
    crate::tools::history::read_for_ui(&root, number)
        .map_err(|error| AppError::Message(error.message()))
}
