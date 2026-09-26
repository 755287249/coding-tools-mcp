use std::sync::OnceLock;

use serde_json::{json, Value};
use tauri::State;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::mcp::activity;

fn profile_id(state: &AppState, id: &str) -> AppResult<String> {
    state.with_workspaces(|store| {
        store
            .get(id)
            .map(|profile| profile.id.clone())
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })
}

/// Live activity feed for the task panel: events newer than `since_rev`
/// (without diff bodies), aggregate stats and the current task plan.
#[tauri::command(async)]
pub fn read_workspace_activity(
    state: State<'_, AppState>,
    id: String,
    since_rev: Option<u64>,
) -> AppResult<Value> {
    let profile_id = profile_id(&state, &id)?;
    Ok(json!(activity::snapshot(
        &profile_id,
        since_rev.unwrap_or(0)
    )))
}

/// A single activity event including its unified diff.
#[tauri::command(async)]
pub fn read_workspace_activity_detail(
    state: State<'_, AppState>,
    id: String,
    seq: u64,
) -> AppResult<Value> {
    let profile_id = profile_id(&state, &id)?;
    Ok(activity::detail(&profile_id, seq)
        .map(|event| json!(event))
        .unwrap_or(Value::Null))
}

#[tauri::command(async)]
pub fn clear_workspace_activity(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let profile_id = profile_id(&state, &id)?;
    activity::clear(&profile_id);
    Ok(())
}

fn parse_windows_build(text: &str) -> Option<u32> {
    let start = text.find("10.0.")? + "10.0.".len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(windows)]
fn detect_windows_build() -> Option<u32> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("cmd")
        .args(["/d", "/c", "ver"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    parse_windows_build(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(windows))]
fn detect_windows_build() -> Option<u32> {
    None
}

/// Windows build number (e.g. 22631), used to pick a window effect that does
/// not lag while dragging. `null` on other platforms or when unknown.
#[tauri::command(async)]
pub fn get_windows_build() -> Option<u32> {
    static BUILD: OnceLock<Option<u32>> = OnceLock::new();
    *BUILD.get_or_init(detect_windows_build)
}

#[cfg(test)]
mod tests {
    use super::parse_windows_build;

    #[test]
    fn parses_ver_output() {
        assert_eq!(
            parse_windows_build("\r\nMicrosoft Windows [Version 10.0.22631.4317]\r\n"),
            Some(22631)
        );
        assert_eq!(
            parse_windows_build("Microsoft Windows [版本 10.0.19045.3803]"),
            Some(19045)
        );
        assert_eq!(parse_windows_build("nothing"), None);
    }
}
