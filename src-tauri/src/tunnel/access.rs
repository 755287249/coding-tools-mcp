use std::sync::LazyLock;

use std::collections::HashSet;

use tokio::sync::Mutex;

use crate::data::DataStore;
use crate::error::AppResult;
use crate::platform::platform;
use crate::settings::AppSettings;
use crate::workspace::WorkspaceProfile;

use super::{TunnelServiceKind, TunnelSupervisor};

static TUNNEL_SUPERVISOR: LazyLock<Mutex<TunnelSupervisor>> =
    LazyLock::new(|| Mutex::new(TunnelSupervisor::new()));

pub fn supervisor() -> &'static Mutex<TunnelSupervisor> {
    &TUNNEL_SUPERVISOR
}

fn tunnel_type_for(profile: &WorkspaceProfile, kind: TunnelServiceKind) -> &str {
    match kind {
        TunnelServiceKind::Mcp => profile.tunnel.tunnel_type.as_str(),
        TunnelServiceKind::Actions => profile.actions.tunnel_type.as_str(),
    }
}

pub fn public_mcp_security_error(profile: &WorkspaceProfile) -> Option<String> {
    let tunnel_type = tunnel_type_for(profile, TunnelServiceKind::Mcp).trim();
    if tunnel_type.is_empty() || tunnel_type == "none" {
        return None;
    }
    super::supervisor::validate_public_mcp_security(profile, TunnelServiceKind::Mcp)
        .err()
        .map(|error| error.to_string())
}

pub async fn maybe_start_for_runtime(
    profile: &WorkspaceProfile,
    kind: TunnelServiceKind,
) -> AppResult<Option<String>> {
    let tunnel_type = tunnel_type_for(profile, kind);
    if tunnel_type.is_empty() || tunnel_type == "none" {
        return Ok(None);
    }
    let settings = AppSettings::load_or_default();
    let mut guard = supervisor().lock().await;
    let status = guard.start(profile, kind, &settings).await?;
    Ok(Some(status.public_url))
}

pub async fn stop_for_runtime(
    profile: &WorkspaceProfile,
    kind: TunnelServiceKind,
) -> AppResult<()> {
    let settings = AppSettings::load_or_default();
    let mut guard = supervisor().lock().await;
    guard.stop(profile, kind, &settings).await
}

pub async fn drop_workspace(workspace_id: &str) -> AppResult<()> {
    // Bound contention without cancelling cleanup after it has taken ownership of
    // child handles or FRP routes. Builtin/child shutdown have their own deadlines.
    let mut guard = tokio::time::timeout(std::time::Duration::from_secs(10), supervisor().lock())
        .await.map_err(|_| crate::error::AppError::Message(
            "Tunnel supervisor is busy. Services are stopped; retry workspace deletion shortly.".into()
        ))?;
    guard.drop_workspace(workspace_id).await
}

pub async fn sync_managed_runtime_routes(
    active_runtime_keys: HashSet<(String, TunnelServiceKind)>,
) -> AppResult<()> {
    let settings = AppSettings::load_or_default();
    let profiles = DataStore::read_file(|data| Ok(data.profiles.clone()))?;
    let mut guard = supervisor().lock().await;
    guard.restore_active_frp_routes(&profiles, &active_runtime_keys, &settings);
    Ok(())
}

pub async fn cleanup_orphan_for_runtime(
    profile: &WorkspaceProfile,
    kind: TunnelServiceKind,
    runtime_listening: bool,
) -> AppResult<()> {
    let port = match kind {
        TunnelServiceKind::Mcp => profile.runtime.local_port,
        TunnelServiceKind::Actions => profile.actions.local_port,
    };
    if runtime_listening || platform().find_pid_listening_on_port(port)?.is_some() {
        return Ok(());
    }
    let mut guard = supervisor().lock().await;
    // 等待 supervisor 锁期间 runtime 可能已经恢复，再确认一次才允许删除 route。
    if platform().find_pid_listening_on_port(port)?.is_some() {
        return Ok(());
    }
    guard.cleanup_orphan(profile, kind, false).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_mcp_security_error_is_none_for_any_permission_mode() {
        let mut profile = WorkspaceProfile::new("C:/workspace/security".into(), None);
        profile.tunnel.tunnel_type = "builtin".into();
        profile.tunnel.public_url = "https://mcp.example.com/clients/device/mcp".into();
        profile.runtime.sandbox.enabled = false;

        for permission_mode in ["guarded", "trusted", "dangerous"] {
            profile.runtime.permission_mode = permission_mode.into();
            assert_eq!(public_mcp_security_error(&profile), None);
        }

        profile.runtime.sandbox.enabled = true;
        assert_eq!(public_mcp_security_error(&profile), None);
    }
}
