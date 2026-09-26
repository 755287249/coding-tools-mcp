use tauri::{Manager, State};

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};

const ALLOWED_KEYS: &[&str] = &[
    "oauth_client_secret",
    "oauth_password",
    "oauth_token_secret",
    "bearer_token",
    "cloudflare_token",
    "actions_cloudflare_token",
    "actions_api_key",
    "actions_oauth_client_secret",
    "actions_oauth_password",
    "actions_oauth_token_secret",
    "frp_token",
    "actions_frp_token",
    "builtin_tunnel_enrollment_url",
];

fn ensure_workspace_exists(state: &AppState, id: &str) -> AppResult<()> {
    state.with_workspaces(|store| {
        if store.get(id).is_some() {
            Ok(())
        } else {
            Err(AppError::Message(format!("workspace not found: {id}")))
        }
    })
}

fn validate_key(key: &str) -> AppResult<()> {
    if ALLOWED_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(AppError::Message(format!("invalid secret key: {key}")))
    }
}

#[tauri::command(async)]
pub fn get_workspace_secret(
    state: State<'_, AppState>,
    id: String,
    key: String,
) -> AppResult<Option<String>> {
    validate_key(&key)?;
    ensure_workspace_exists(&state, &id)?;
    state.with_data(|store| store.get_workspace_secret(&id, &key))
}

#[tauri::command(async)]
pub fn set_workspace_secret(
    state: State<'_, AppState>,
    id: String,
    key: String,
    value: String,
) -> AppResult<()> {
    validate_key(&key)?;
    ensure_workspace_exists(&state, &id)?;
    // Accept the full `cloudflared service install <token>` command from the dashboard.
    let value = if matches!(
        key.as_str(),
        "cloudflare_token" | "actions_cloudflare_token"
    ) {
        crate::tunnel::normalize_cloudflare_token(&value)
    } else {
        value
    };
    state.with_data(|store| store.set_workspace_secret(&id, &key, &value))
}

#[tauri::command(async)]
pub fn regenerate_workspace_secret(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    key: String,
) -> AppResult<String> {
    validate_key(&key)?;
    ensure_workspace_exists(&state, &id)?;
    let value = state.with_data(|store| store.regenerate_workspace_secret(&id, &key))?;
    let profile = state.with_workspaces(|store| {
        store
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })?;

    schedule_running_services_restart(app, vec![profile], key, false);
    Ok(value)
}

const SHARED_KEYS: &[&str] = &[
    "oauth_client_id",
    "bearer_token",
    "oauth_client_secret",
    "oauth_password",
    "oauth_token_secret",
    "actions_api_key",
    "actions_oauth_client_secret",
    "actions_oauth_password",
    "actions_oauth_token_secret",
];

const MCP_SHARED_KEYS: &[&str] = &[
    "oauth_client_id",
    "bearer_token",
    "oauth_client_secret",
    "oauth_password",
    "oauth_token_secret",
];

const ACTIONS_SHARED_KEYS: &[&str] = &[
    "actions_api_key",
    "actions_oauth_client_secret",
    "actions_oauth_password",
    "actions_oauth_token_secret",
];

#[tauri::command(async)]
pub fn get_shared_secret(state: State<'_, AppState>, key: String) -> AppResult<Option<String>> {
    if !SHARED_KEYS.contains(&key.as_str()) {
        return Err(AppError::Message(format!("invalid shared key: {key}")));
    }
    state.with_data(|store| Ok(store.get_shared_secret(&key)))
}

#[tauri::command(async)]
pub fn set_shared_secret(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> AppResult<()> {
    if !SHARED_KEYS.contains(&key.as_str()) {
        return Err(AppError::Message(format!("invalid shared key: {key}")));
    }
    if value.is_empty() {
        return Err(AppError::Message("密钥不能为空。".into()));
    }
    let changed = state.with_data(|store| {
        if store.get_shared_secret(&key).as_deref() == Some(value.as_str()) {
            return Ok(false);
        }
        store.set_shared_secret(&key, &value)?;
        Ok(true)
    })?;
    if changed {
        let workspaces = state.with_workspaces(|store| Ok(store.list().to_vec()))?;
        schedule_running_services_restart(app, workspaces, key, true);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn regenerate_shared_secret(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    key: String,
) -> AppResult<String> {
    if !SHARED_KEYS.contains(&key.as_str()) {
        return Err(AppError::Message(format!("invalid shared key: {key}")));
    }
    let value = state.with_data(|store| store.regenerate_shared_secret(&key))?;

    let workspaces = state.with_workspaces(|store| Ok(store.list().to_vec()))?;
    schedule_running_services_restart(app, workspaces, key, true);

    Ok(value)
}

fn schedule_running_services_restart(
    app: tauri::AppHandle,
    profiles: Vec<crate::workspace::WorkspaceProfile>,
    key: String,
    shared: bool,
) {
    let targets = restart_targets(&profiles, &key, shared);
    if targets.is_empty() {
        return;
    }
    let should_spawn = {
        let mut pending = pending_restarts()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.targets.extend(targets);
        pending.generation = pending.generation.wrapping_add(1);
        if pending.scheduled {
            false
        } else {
            pending.scheduled = true;
            true
        }
    };
    if !should_spawn {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let mut observed_generation = pending_restarts()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation;
        loop {
            // Multi-field saves arrive as a burst. One debounce window turns them
            // into a single stop/start per workspace service.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let targets = {
                let mut pending = pending_restarts()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if pending.generation != observed_generation {
                    observed_generation = pending.generation;
                    continue;
                }
                std::mem::take(&mut pending.targets)
            };
            let app_for_restart = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                restart_running_services(&app_for_restart, targets);
            })
            .await;

            let mut pending = pending_restarts()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if pending.targets.is_empty() {
                pending.scheduled = false;
                break;
            }
            observed_generation = pending.generation;
        }
    });
}

/// 仅重启当前确实在运行、且使用了这组密钥的服务。
///
/// 密钥命令是桌面端和设置页共用的入口，因此重启必须放在后端统一处理。
/// 前端不再额外调用 restart_*，避免同一次密钥变更触发两次停止/启动竞态。
fn restart_running_services(app: &tauri::AppHandle, targets: HashSet<RestartTarget>) {
    let state = app.state::<AppState>();
    for target in targets {
        let profile = match state.with_workspaces(|store| {
            store.get(&target.workspace_id).cloned().ok_or_else(|| {
                AppError::Message(format!("workspace not found: {}", target.workspace_id))
            })
        }) {
            Ok(profile) => profile,
            Err(error) => {
                eprintln!("secret restart skipped: {error}");
                continue;
            }
        };
        let result = state.with_runtime(|runtime| {
            if !runtime.is_running(&profile.id, target.kind) {
                return Ok(());
            }
            let restart = match target.kind {
                crate::runtime::ServiceKind::Mcp => runtime.restart_mcp(&profile),
                crate::runtime::ServiceKind::Actions => runtime.restart_actions(&profile),
            };
            if let Err(error) = restart {
                eprintln!(
                    "{} restart after secret change failed for {}: {error}",
                    service_label(target.kind),
                    profile.id
                );
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!(
                "runtime state unavailable after secret change for {}: {error}",
                profile.id
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RestartTarget {
    workspace_id: String,
    kind: crate::runtime::ServiceKind,
}

#[derive(Default)]
struct PendingRestarts {
    scheduled: bool,
    generation: u64,
    targets: HashSet<RestartTarget>,
}

fn pending_restarts() -> &'static Mutex<PendingRestarts> {
    static PENDING: OnceLock<Mutex<PendingRestarts>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(PendingRestarts::default()))
}

fn restart_targets(
    profiles: &[crate::workspace::WorkspaceProfile],
    key: &str,
    shared: bool,
) -> HashSet<RestartTarget> {
    let mut targets = HashSet::new();
    for profile in profiles {
        if MCP_SHARED_KEYS.contains(&key) && profile.auth.use_shared_secrets == shared {
            targets.insert(RestartTarget {
                workspace_id: profile.id.clone(),
                kind: crate::runtime::ServiceKind::Mcp,
            });
        }
        if ACTIONS_SHARED_KEYS.contains(&key) && profile.actions.use_shared_secrets == shared {
            targets.insert(RestartTarget {
                workspace_id: profile.id.clone(),
                kind: crate::runtime::ServiceKind::Actions,
            });
        }
    }
    targets
}

fn service_label(kind: crate::runtime::ServiceKind) -> &'static str {
    match kind {
        crate::runtime::ServiceKind::Mcp => "MCP",
        crate::runtime::ServiceKind::Actions => "Actions",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_targets_are_deduplicated_by_workspace_and_service() {
        let mut profile = crate::workspace::WorkspaceProfile::new(
            "C:/workspace/restart-dedupe".into(),
            Some("dedupe".into()),
        );
        profile.auth.use_shared_secrets = true;
        let profiles = vec![profile.clone(), profile];

        let targets = restart_targets(&profiles, "oauth_password", true);

        assert_eq!(targets.len(), 1);
        assert!(targets.iter().any(|target| {
            target.kind == crate::runtime::ServiceKind::Mcp && target.workspace_id == profiles[0].id
        }));
    }

    #[test]
    fn restart_targets_respect_service_and_secret_scope() {
        let mut profile = crate::workspace::WorkspaceProfile::new(
            "C:/workspace/restart-scope".into(),
            Some("scope".into()),
        );
        profile.auth.use_shared_secrets = true;
        profile.actions.use_shared_secrets = false;

        assert_eq!(
            restart_targets(std::slice::from_ref(&profile), "bearer_token", true).len(),
            1
        );
        assert_eq!(
            restart_targets(std::slice::from_ref(&profile), "actions_api_key", true).len(),
            0
        );
        assert_eq!(
            restart_targets(&[profile], "actions_api_key", false).len(),
            1
        );
    }
}
