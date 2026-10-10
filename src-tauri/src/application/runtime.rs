use std::time::Duration;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::platform::platform;
use crate::runtime::{
    await_listener_shutdown, port_busy_message, try_reclaim_previous_macos_app_port,
    wait_for_port_free, ServiceKind,
};
use crate::tunnel::{
    maybe_start_for_runtime, stop_for_runtime, sync_managed_runtime_routes, TunnelServiceKind,
};
use crate::workspace::resources::{validate_service_start, WorkspaceService};
use crate::workspace::{RuntimeStatusDto, WorkspaceProfile};

fn profile_by_id(state: &AppState, id: &str) -> AppResult<WorkspaceProfile> {
    state.with_workspaces(|store| {
        store
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Message(format!("workspace not found: {id}")))
    })
}

fn validate_start_resources(
    state: &AppState,
    id: &str,
    service: WorkspaceService,
) -> AppResult<()> {
    state.with_workspaces(|store| validate_service_start(store.list(), id, service))
}

fn persist_mcp_runtime_enabled(state: &AppState, id: &str, enabled: bool) -> AppResult<()> {
    state.with_workspaces(|store| store.set_mcp_runtime_enabled(id, enabled))
}

fn persist_actions_runtime_enabled(state: &AppState, id: &str, enabled: bool) -> AppResult<()> {
    state.with_workspaces(|store| store.set_actions_runtime_enabled(id, enabled))
}

fn persist_tunnel_url(
    state: &AppState,
    id: &str,
    kind: TunnelServiceKind,
    url: &str,
) -> AppResult<()> {
    if url.is_empty() {
        return Ok(());
    }

    state.with_workspaces(|store| {
        let Some(mut profile) = store.get(id).cloned() else {
            return Ok(());
        };

        match kind {
            TunnelServiceKind::Mcp => profile.tunnel.public_url = url.to_string(),
            TunnelServiceKind::Actions => profile.actions.public_url = url.to_string(),
        }

        store.update(profile)?;
        Ok(())
    })
}

async fn sync_tunnel_routes_from_runtime(state: &AppState) -> AppResult<()> {
    let active_keys = state.with_runtime(|runtime| Ok(runtime.active_tunnel_service_keys()))?;
    sync_managed_runtime_routes(active_keys).await
}

#[allow(clippy::collapsible_if)]
async fn ensure_port_available(
    port: u16,
    service_label: &str,
    managed_running: bool,
) -> AppResult<()> {
    // Only the supervisor entry for this workspace/service may reuse its port.
    // PID ownership alone could also refer to a different service in this app.
    if managed_running {
        return Ok(());
    }
    let Some(pid) = platform().find_pid_listening_on_port(port)? else {
        return Ok(());
    };

    if crate::runtime::is_own_process(pid) {
        if wait_for_port_free(port, Duration::from_secs(3)).await {
            return Ok(());
        }
    }

    if try_reclaim_previous_macos_app_port(port) {
        return Ok(());
    }

    if let Some(pid) = platform().find_pid_listening_on_port(port)? {
        return Err(AppError::Message(port_busy_message(
            port,
            service_label,
            pid,
        )));
    }

    Ok(())
}

pub async fn start_mcp_runtime(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    validate_start_resources(state, id, WorkspaceService::Mcp)?;
    let profile = profile_by_id(state, id)?;

    let running = state.with_runtime(|runtime| Ok(runtime.is_running(id, ServiceKind::Mcp)))?;
    ensure_port_available(profile.runtime.local_port, "本地 MCP", running).await?;

    state.with_runtime(|runtime| runtime.start_mcp(&profile))?;
    // The local listener is already running even if tunnel startup below fails,
    // so remember MCP as enabled at this point.
    persist_mcp_runtime_enabled(state, id, true)?;
    sync_tunnel_routes_from_runtime(state).await?;

    match maybe_start_for_runtime(&profile, TunnelServiceKind::Mcp).await {
        Ok(Some(url)) => persist_tunnel_url(state, id, TunnelServiceKind::Mcp, &url)?,
        Ok(None) => {}
        Err(error) => {
            return Err(AppError::Message(format!(
                "本地 MCP 已啟動，但公網隧道啟動失敗：{error}"
            )));
        }
    }

    let profile = profile_by_id(state, id)?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    state.with_runtime(|runtime| {
        runtime.refresh_mcp(&profile);
        Ok(runtime.mcp_status(&profile))
    })
}

pub async fn stop_mcp_runtime(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    let profile = profile_by_id(state, id)?;
    // Explicit stop is the durable user intent, even if cleanup later reports
    // an error.
    persist_mcp_runtime_enabled(state, id, false)?;
    let port = profile.runtime.local_port;
    let handle = state.with_runtime(|runtime| Ok(runtime.begin_stop(id, ServiceKind::Mcp)))?;

    await_listener_shutdown(handle, port).await;

    state.with_runtime(|runtime| {
        runtime.finish_stop(id, ServiceKind::Mcp);
        Ok(runtime.mcp_status(&profile))
    })?;
    stop_for_runtime(&profile, TunnelServiceKind::Mcp).await?;
    sync_tunnel_routes_from_runtime(state).await?;
    state.with_runtime(|runtime| Ok(runtime.mcp_status(&profile)))
}

pub fn mcp_runtime_status(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    let profile = profile_by_id(state, id)?;
    state.with_runtime(|runtime| {
        runtime.refresh_mcp(&profile);
        Ok(runtime.mcp_status(&profile))
    })
}

pub async fn start_actions_runtime(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    validate_start_resources(state, id, WorkspaceService::Actions)?;
    let profile = profile_by_id(state, id)?;

    let running = state.with_runtime(|runtime| Ok(runtime.is_running(id, ServiceKind::Actions)))?;
    ensure_port_available(profile.actions.local_port, "本地 Actions", running).await?;

    state.with_runtime(|runtime| runtime.start_actions(&profile))?;
    persist_actions_runtime_enabled(state, id, true)?;
    sync_tunnel_routes_from_runtime(state).await?;

    match maybe_start_for_runtime(&profile, TunnelServiceKind::Actions).await {
        Ok(Some(url)) => persist_tunnel_url(state, id, TunnelServiceKind::Actions, &url)?,
        Ok(None) => {}
        Err(error) => {
            return Err(AppError::Message(format!(
                "本地 Actions 已啟動，但公網隧道啟動失敗：{error}"
            )));
        }
    }

    let profile = profile_by_id(state, id)?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    state.with_runtime(|runtime| {
        runtime.refresh_actions(&profile);
        Ok(runtime.actions_status(&profile))
    })
}

pub async fn stop_actions_runtime(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    let profile = profile_by_id(state, id)?;
    persist_actions_runtime_enabled(state, id, false)?;
    let port = profile.actions.local_port;
    let handle = state.with_runtime(|runtime| Ok(runtime.begin_stop(id, ServiceKind::Actions)))?;

    await_listener_shutdown(handle, port).await;

    state.with_runtime(|runtime| {
        runtime.finish_stop(id, ServiceKind::Actions);
        Ok(runtime.actions_status(&profile))
    })?;
    stop_for_runtime(&profile, TunnelServiceKind::Actions).await?;
    sync_tunnel_routes_from_runtime(state).await?;
    state.with_runtime(|runtime| Ok(runtime.actions_status(&profile)))
}

pub fn actions_runtime_status(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    let profile = profile_by_id(state, id)?;
    state.with_runtime(|runtime| {
        runtime.refresh_actions(&profile);
        Ok(runtime.actions_status(&profile))
    })
}

pub fn restart_mcp_runtime(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    validate_start_resources(state, id, WorkspaceService::Mcp)?;
    let profile = profile_by_id(state, id)?;
    let status = state.with_runtime(|runtime| runtime.restart_mcp(&profile))?;
    persist_mcp_runtime_enabled(state, id, true)?;
    Ok(status)
}

pub fn restart_actions_runtime(state: &AppState, id: &str) -> AppResult<RuntimeStatusDto> {
    validate_start_resources(state, id, WorkspaceService::Actions)?;
    let profile = profile_by_id(state, id)?;
    let status = state.with_runtime(|runtime| runtime.restart_actions(&profile))?;
    persist_actions_runtime_enabled(state, id, true)?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn running_managed_service_reuses_its_occupied_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            platform().find_pid_listening_on_port(port).unwrap(),
            Some(std::process::id())
        );
        ensure_port_available(port, "test MCP", true).await.unwrap();
        assert!(ensure_port_available(port, "test MCP", false)
            .await
            .is_err());
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
    }
}
