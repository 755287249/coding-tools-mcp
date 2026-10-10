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
    if matches!(args["action"].as_str(),Some("prepare_compat" | "revoke_compat")) {
        if args["action"]=="prepare_compat" {
            let ctx=crate::tools::hub::resolve_profile_folder_context(&id,&folder_id).map_err(AppError::Message)?;
            crate::tools::chat::compat::check_policy(&ctx).map_err(|e|AppError::Message(e.message()))?;
        }
        return crate::tools::chat::compat::management(&root,&id,&folder_id,&args).map_err(|e|AppError::Message(e.message()));
    }
    crate::tools::chat::ui(&root, &args).map_err(|e| AppError::Message(e.message()))
}
