use tauri::State;

use crate::app_state::AppState;
use crate::error::AppResult;
use crate::settings::DownloadConfig;
use crate::tunnel::{
    install_software as install_binary, list_software as list_binaries,
    uninstall_software as uninstall_binary, SoftwareStatus,
};

/// List install status for tunnel clients and sandbox CLIs.
#[tauri::command(async)]
pub fn list_software() -> AppResult<Vec<SoftwareStatus>> {
    Ok(list_binaries())
}

/// Install the requested tool ("frpc" | "cloudflared" | "sbx" | "wslc").
#[tauri::command]
pub async fn install_software(kind: String) -> AppResult<SoftwareStatus> {
    install_binary(&kind).await
}

/// Remove a cache-managed binary ("frpc" | "cloudflared"). Sandbox CLIs are not cache-managed.
#[tauri::command(async)]
pub fn uninstall_software(kind: String) -> AppResult<SoftwareStatus> {
    uninstall_binary(&kind)
}

/// Read the download config (mirror + proxy).
#[tauri::command(async)]
pub fn get_download_config(state: State<'_, AppState>) -> AppResult<DownloadConfig> {
    state.with_settings(|store| Ok(store.settings().download.clone()))
}

/// Persist the download config (mirror + proxy).
#[tauri::command(async)]
pub fn set_download_config(state: State<'_, AppState>, config: DownloadConfig) -> AppResult<()> {
    state.with_settings(|store| {
        let mut settings = store.settings();
        settings.download = config;
        store.update_settings(settings)
    })
}
