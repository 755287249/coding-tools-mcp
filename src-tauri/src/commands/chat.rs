use crate::{
    app_state::AppState,
    error::{AppError, AppResult},
};
use serde_json::Value;
use tauri::State;
#[tauri::command(async)]
pub fn local_chat(
    state: State<'_, AppState>,
    id: String,
    folder_id: String,
    args: Value,
) -> AppResult<Value> {
    let root = state.with_workspaces(|store| {
        store
            .get(&id)
            .and_then(|profile| profile.folders.iter().find(|f| f.id == folder_id))
            .map(|folder| std::path::PathBuf::from(&folder.path))
            .ok_or_else(|| AppError::Message("Select a configured folder".into()))
    })?;
    crate::tools::chat::ui(&root, &args).map_err(|e| AppError::Message(e.message()))
}
