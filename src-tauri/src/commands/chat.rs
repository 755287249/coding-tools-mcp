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

/// Runs independently of browser windows and AI transport availability.
pub fn start_schedule_worker(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || loop {
        let roots = app.state::<AppState>().with_workspaces(|store| {
            Ok(store
                .list()
                .iter()
                .flat_map(|p| p.folders.iter().map(|f| std::path::PathBuf::from(&f.path)))
                .collect::<std::collections::HashSet<_>>())
        });
        if let Ok(roots) = roots {
            for root in roots {
                if let Err(error) = crate::tools::chat::schedules::tick(&root) {
                    eprintln!("Scheduled chat delivery: {}", error.message());
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    });
}
