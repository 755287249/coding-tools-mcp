use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use tauri::State;

use crate::app_state::{bootstrap_workspace, teardown_workspace, AppState};
use crate::error::{AppError, AppResult};
use crate::platform::open_path_in_file_manager;
use crate::tunnel::drop_workspace as drop_tunnel_workspace;
use crate::workspace::resources::{
    assign_free_workspace_ports, validate_workspace_resources_update,
};
use crate::workspace::{compare_wsl_paths, wsl_unc_path, WorkspaceFolder, WorkspaceProfile};
use serde_json::Value;

#[tauri::command(async)]
pub fn list_workspaces(state: State<'_, AppState>) -> AppResult<Vec<WorkspaceProfile>> {
    state.with_workspaces(|store| Ok(store.list().to_vec()))
}

#[tauri::command(async)]
pub fn create_workspace(
    state: State<'_, AppState>,
    path: String,
    name: Option<String>,
) -> AppResult<WorkspaceProfile> {
    validate_folder_path(&path)?;
    create_workspace_inner(&state, path, name)
}

#[tauri::command(async)]
pub fn export_workspace_pack(state: State<'_, AppState>, id: String) -> AppResult<Value> {
    state.with_data(|store| store.export_workspace_pack(&id))
}

#[tauri::command(async)]
pub fn export_shared_workspace(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<std::path::PathBuf> {
    state.with_data(|store| store.export_shared_workspace(&id))
}

#[tauri::command(async)]
pub fn open_shared_workspace(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<WorkspaceProfile> {
    state.with_workspaces(|store| store.open_shared_workspace(&id))
}

#[tauri::command(async)]
pub fn import_workspace_pack(
    state: State<'_, AppState>,
    pack: Value,
) -> AppResult<WorkspaceProfile> {
    state.with_workspaces(|store| store.import_workspace_pack(pack))
}

fn create_workspace_inner(
    state: &AppState,
    path: String,
    name: Option<String>,
) -> AppResult<WorkspaceProfile> {
    state.with_workspaces(|store| {
        let mut profile = WorkspaceProfile::new(path, name);
        // Create should not fail just because default ports are already claimed.
        // Pick free ports now; start/update still enforce conflict checks.
        assign_free_workspace_ports(store.list(), &mut profile)?;
        bootstrap_workspace(store, &profile.id)?;
        store.add(profile.clone())?;
        Ok(profile)
    })
}

#[tauri::command(async)]
pub fn list_wsl_distributions() -> AppResult<Vec<String>> {
    #[cfg(not(windows))]
    {
        return Ok(Vec::new());
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = Command::new("wsl.exe")
            .args(["--list", "--quiet"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| AppError::Message(format!("無法執行 WSL：{error}")))?;
        if !output.status.success() {
            let message = decode_wsl_output(&output.stderr);
            return Err(AppError::Message(if message.trim().is_empty() {
                "WSL 尚未安裝或無法使用。".into()
            } else {
                format!("無法取得 WSL distributions：{}", message.trim())
            }));
        }
        let mut distributions = decode_wsl_output(&output.stdout)
            .lines()
            .map(|line| line.trim().trim_matches('\0'))
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        distributions.sort_by_key(|name| name.to_ascii_lowercase());
        distributions.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        Ok(distributions)
    }
}

fn validate_wsl_location(distro: &str, linux_path: &str) -> AppResult<()> {
    if distro.is_empty()
        || distro.contains(['/', '\\', '\0'])
        || distro.chars().any(char::is_control)
    {
        return Err(AppError::Message("WSL distribution 名稱無效。".into()));
    }
    if !linux_path.starts_with('/')
        || linux_path.contains(['\\', '\0'])
        || linux_path.chars().any(char::is_control)
    {
        return Err(AppError::Message(
            "WSL 資料夾必須是使用 / 分隔的絕對 Linux 路徑。".into(),
        ));
    }
    if linux_path.split('/').any(|segment| segment == "..") {
        return Err(AppError::Message(
            "WSL 資料夾不可包含 .. 父目錄片段。".into(),
        ));
    }
    #[cfg(not(windows))]
    {
        return Err(AppError::Message(
            "只有 Windows client 支援 WSL workspace。".into(),
        ));
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = Command::new("wsl.exe")
            .args(["--distribution", distro, "--exec", "test", "-d", linux_path])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| AppError::Message(format!("無法啟動 WSL：{error}")))?;
        if !output.status.success() {
            let message = decode_wsl_output(&output.stderr);
            return Err(AppError::Message(if message.trim().is_empty() {
                format!("WSL 資料夾不存在或無法存取：{distro}:{linux_path}")
            } else {
                format!("WSL 資料夾無法存取：{}", message.trim())
            }));
        }
        Ok(())
    }
}

fn decode_wsl_output(bytes: &[u8]) -> String {
    let looks_utf16 = bytes.len() >= 2
        && bytes.len() % 2 == 0
        && bytes.chunks_exact(2).filter(|pair| pair[1] == 0).count() > bytes.len() / 8;
    if looks_utf16 {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_string()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[tauri::command]
pub async fn update_workspace(
    state: State<'_, AppState>,
    mut profile: WorkspaceProfile,
) -> AppResult<()> {
    profile.normalize_folders();
    profile.normalize_bind_addresses();
    profile.normalize_sandbox();
    profile
        .validate_bind_addresses()
        .map_err(AppError::Message)?;
    let current = state.with_workspaces(|store| {
        store
            .get(&profile.id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {}", profile.id)))
    })?;
    state.with_workspaces(|store| {
        validate_workspace_resources_update(store.list(), &current, &profile)
    })?;
    if crate::tunnel::public_mcp_security_error(&profile).is_some() {
        crate::tunnel::stop_for_runtime(&current, crate::tunnel::TunnelServiceKind::Mcp).await?;
    }
    crate::tools::hub::preflight_live_hub(&profile).map_err(AppError::Message)?;
    let updated = state.with_workspaces(|store| {
        store.update(profile.clone())?;
        Ok(profile.clone())
    })?;
    crate::tools::hub::sync_live_hub_after_preflight(&updated).map_err(AppError::Message)?;
    Ok(())
}

#[tauri::command(async)]
pub fn add_workspace_folder(
    state: State<'_, AppState>,
    id: String,
    path: String,
    name: Option<String>,
) -> AppResult<WorkspaceProfile> {
    validate_folder_path(&path)?;
    add_workspace_folder_inner(&state, id, path, name)
}

#[tauri::command(async)]
pub fn add_wsl_workspace_folder(
    state: State<'_, AppState>,
    id: String,
    distro: String,
    linux_path: String,
    name: Option<String>,
) -> AppResult<WorkspaceProfile> {
    let distro = distro.trim();
    let linux_path = linux_path.trim();
    validate_wsl_location(distro, linux_path)?;
    let path = wsl_unc_path(distro, linux_path);
    validate_folder_path(&path)?;
    add_workspace_folder_inner(&state, id, path, name)
}

fn add_workspace_folder_inner(
    state: &AppState,
    id: String,
    path: String,
    name: Option<String>,
) -> AppResult<WorkspaceProfile> {
    let updated = state.with_workspaces(|store| {
        let mut profile = store
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))?;
        if profile
            .folders
            .iter()
            .any(|folder| same_folder_path(&folder.path, &path))
        {
            return Err(AppError::Message("此資料夾已在工具區清單中。".into()));
        }
        profile.folders.push(WorkspaceFolder::new(path, name));
        profile.normalize_folders();
        store.update(profile.clone())?;
        Ok(profile)
    })?;
    crate::tools::hub::sync_live_hub(&updated).map_err(AppError::Message)?;
    Ok(updated)
}

#[tauri::command(async)]
pub fn remove_workspace_folder(
    state: State<'_, AppState>,
    id: String,
    folder_id: String,
) -> AppResult<WorkspaceProfile> {
    let updated = state.with_workspaces(|store| {
        let mut profile = store
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))?;
        if profile.folders.len() <= 1 {
            return Err(AppError::Message("工具區至少需要保留一個資料夾。".into()));
        }
        let before = profile.folders.len();
        profile.folders.retain(|folder| folder.id != folder_id);
        if profile.folders.len() == before {
            return Err(AppError::Message(format!("folder not found: {folder_id}")));
        }
        if profile.active_folder_id == folder_id {
            let next = profile
                .folders
                .first()
                .cloned()
                .ok_or_else(|| AppError::Message("工具區至少需要保留一個資料夾。".into()))?;
            profile.active_folder_id = next.id;
            profile.path = next.path;
        }
        profile.normalize_folders();
        store.update(profile.clone())?;
        Ok(profile)
    })?;
    crate::tools::hub::sync_live_hub(&updated).map_err(AppError::Message)?;
    Ok(updated)
}

fn validate_folder_path(path: &str) -> AppResult<()> {
    let path = PathBuf::from(path.trim());
    if path.as_os_str().is_empty() {
        return Err(AppError::Message("資料夾路徑不可為空。".into()));
    }
    let metadata = std::fs::metadata(&path)
        .map_err(|_| AppError::Message(format!("資料夾不存在：{}", path.display())))?;
    if !metadata.is_dir() {
        return Err(AppError::Message(format!(
            "路徑不是資料夾：{}",
            path.display()
        )));
    }
    Ok(())
}

fn same_folder_path(left: &str, right: &str) -> bool {
    if let Some(equal) = compare_wsl_paths(left, right) {
        return equal;
    }
    let left = left.trim_end_matches(['\\', '/']).replace('\\', "/");
    let right = right.trim_end_matches(['\\', '/']).replace('\\', "/");
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(&right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[tauri::command(async)]
pub fn open_workspace_directory(path: String) -> AppResult<()> {
    let path = PathBuf::from(path.trim());
    open_path_in_file_manager(&path)
}

#[tauri::command(async)]
pub fn delete_workspace(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let profile = state.with_workspaces(|store| {
        store
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })?;
    tauri::async_runtime::block_on(drop_tunnel_workspace(&id))?;
    state.with_runtime(|runtime| {
        runtime.drop_workspace(&profile);
        Ok(())
    })?;
    crate::tools::hub::remove_live_hub(&id);
    state.with_workspaces(|store| {
        if store.remove(&id)?.is_some() {
            teardown_workspace(store, &id)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{decode_wsl_output, same_folder_path, validate_wsl_location};

    #[test]
    fn decodes_utf8_and_utf16_wsl_output() {
        assert_eq!(
            decode_wsl_output(b"Ubuntu\r\nDebian\r\n"),
            "Ubuntu\r\nDebian\r\n"
        );

        let utf16 = "\u{feff}Ubuntu-24.04\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(decode_wsl_output(&utf16), "Ubuntu-24.04\r\n");
    }

    #[test]
    fn rejects_ambiguous_wsl_workspace_paths_before_launch() {
        assert!(validate_wsl_location("Ubuntu", r"/opt\src").is_err());
        assert!(validate_wsl_location("Ubuntu", "/opt/../etc").is_err());
        assert!(validate_wsl_location("Ubuntu\nInjected", "/opt/src").is_err());
        assert!(validate_wsl_location("Ubuntu", "/opt/src\nInjected").is_err());
    }

    #[test]
    fn wsl_folder_comparison_preserves_linux_path_case() {
        assert!(same_folder_path(
            r"\\wsl$\Ubuntu-24.04\opt\src\SampleProject",
            r"\\wsl.localhost\ubuntu-24.04\opt\src\SampleProject"
        ));
        assert!(!same_folder_path(
            r"\\wsl.localhost\Ubuntu-24.04\opt\src\SampleProject",
            r"\\wsl.localhost\Ubuntu-24.04\opt\src\sampleproject"
        ));
    }
}
