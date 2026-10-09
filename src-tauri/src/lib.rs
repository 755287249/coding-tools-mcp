#![cfg_attr(target_os = "windows", allow(linker_messages))]

mod actions;
mod app_state;
mod application;
mod auth;
#[cfg(feature = "desktop")]
mod commands;
mod data;
mod error;
pub mod harness;
mod health;
pub mod knowledge;
mod mcp;
mod platform;
mod runtime;
mod secret;
mod settings;
mod task_runtime;
pub mod tools;
mod tunnel;
#[cfg(feature = "desktop")]
mod window_corners;
mod workspace;
mod workspace_features;

/// Stable, UI-independent types for future headless and cross-platform hosts.
///
/// The desktop application remains the default feature. Consumers that only
/// need the core can build this library with `--no-default-features` without
/// pulling Tauri, GTK, or WebView dependencies into their binary.
pub fn run_protect_cli() -> i32 {
    match crate::data::protection_cli() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

pub mod core {
    pub use crate::app_state::{bootstrap_workspace, teardown_workspace, AppState};
    pub use crate::application::runtime::{
        actions_runtime_status, mcp_runtime_status, restart_actions_runtime, restart_mcp_runtime,
        start_actions_runtime, start_mcp_runtime, stop_actions_runtime, stop_mcp_runtime,
    };
    pub use crate::data::{AppData, DataStore};
    pub use crate::error::{AppError, AppResult};
    pub use crate::runtime::{RuntimeSupervisor, ServiceKind};
    pub use crate::workspace::{
        ActionsConfig, AuthConfig, ExecutionTarget, McpPublicEndpoint, RuntimeConfig,
        RuntimeStatusDto, SandboxConfig, WorkspaceFolder, WorkspaceProfile, WslLocation,
    };
}

#[doc(hidden)]
pub fn run_appcontainer_acl_helper_if_requested() -> Option<i32> {
    #[cfg(windows)]
    {
        return tools::sandbox::run_appcontainer_acl_helper_if_requested();
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[doc(hidden)]
pub fn export_behavioral_parity_fixtures() -> serde_json::Value {
    let limits = tools::ExecutionLimits::default();
    serde_json::json!({
        "execution_limits": {
            "blocking_admission": limits.blocking_admission,
            "process_admission": limits.process_admission,
            "global_blocking_admission": limits.global_blocking_admission,
            "global_process_admission": limits.global_process_admission,
            "active_sessions": limits.active_sessions,
            "command_timeout_default_ms": tools::DEFAULT_COMMAND_TIMEOUT_MAX_MS,
            "command_timeout_absolute_max_ms": tools::ABSOLUTE_COMMAND_TIMEOUT_MAX_MS,
            "job_timeout_default_max_ms": tools::execution_timeout::DEFAULT_JOB_TIMEOUT_MAX_MS,
            "job_timeout_absolute_max_ms": tools::execution_timeout::ABSOLUTE_JOB_TIMEOUT_MAX_MS
        },
        "workspace": tools::hub::behavioral_parity_fixture(),
        "process_start": tools::process_start_behavioral_parity_fixture(),
        "mcp_transport": mcp::behavioral_parity_fixture(),
        "tunnel": tunnel::builtin_behavioral_parity_fixture()
    })
}

#[cfg(feature = "desktop")]
use app_state::AppState;
#[cfg(feature = "desktop")]
use commands::{
    add_workspace_folder, add_wsl_workspace_folder, create_workspace, delete_frp_profile,
    delete_workspace, export_shared_workspace, export_workspace_pack, get_actions_runtime_status,
    get_app_settings, get_download_config, get_frp_snippet, get_last_workspace_id, get_proxy,
    get_runtime_status, get_shared_secret, get_workspace_extensions, get_workspace_secret,
    get_workspace_skills, import_workspace_pack, install_software, list_frp_profiles,
    local_chat, list_history_sessions, list_sandbox_backends, list_software, list_workspaces,
    list_wsl_distributions, open_shared_workspace, open_workspace_directory, read_history_session,
    read_workspace_logs, read_workspace_telemetry, regenerate_shared_secret,
    clear_workspace_activity, get_windows_build, read_workspace_activity,
    read_workspace_activity_detail,
    regenerate_workspace_secret, remove_workspace_folder, restart_actions_runtime, restart_runtime,
    restart_tunnel, run_health_checks, save_frp_profile, set_download_config, set_last_workspace,
    set_proxy, set_shared_secret, set_workspace_extension_active, set_workspace_extension_enabled,
    set_workspace_secret, set_workspace_skill_enabled, set_workspace_skills_active,
    start_actions_runtime, start_runtime, start_tunnel, stop_actions_runtime, stop_runtime,
    stop_tunnel, test_tunnel, uninstall_software, update_workspace,
};
#[cfg(feature = "desktop")]
use tauri::Manager;

#[cfg(feature = "desktop")]
fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(all(feature = "desktop", target_os = "windows"))]
fn acquire_single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    // 保持 mutex HANDLE 到进程退出，由 Windows 自动回收。第二个实例必须在
    // cleanup_managed_frpc_instances 之前退出，否则会清理第一个实例的 frpc。
    let Ok(handle) = (unsafe {
        CreateMutexW(
            None,
            false,
            w!("Local\\CodingToolsMcpDesktop-SingleInstance"),
        )
    }) else {
        eprintln!("创建应用单实例锁失败，为避免误清理其他实例的 frpc，本次启动已取消");
        return false;
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        return false;
    }
    true
}

#[cfg(all(feature = "desktop", not(target_os = "windows")))]
fn acquire_single_instance() -> bool {
    true
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if !acquire_single_instance() {
        return;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            use tauri::menu::MenuBuilder;
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            let tray_menu = MenuBuilder::new(app)
                .text("show", "顯示主視窗")
                .text("quit", "結束程式")
                .build()?;
            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .tooltip("Coding Tools MCP")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            tray_builder.build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                window_corners::init(window);
            }

            let state = AppState::new().expect("failed to load app state");
            if let Err(error) =
                state.with_workspaces(|store| store.consume_runtime_handoff_state().map(|_| ()))
            {
                eprintln!("匯入版本切換 runtime 狀態失敗：{error}");
            }
            let (mcp_auto_start_ids, actions_auto_start_ids) = state
                .with_workspaces(|store| {
                    Ok((
                        store.mcp_auto_start_workspace_ids(),
                        store.actions_auto_start_workspace_ids(),
                    ))
                })
                .expect("failed to load runtime auto-start state");
            app.manage(state);
            if !mcp_auto_start_ids.is_empty() || !actions_auto_start_ids.is_empty() {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    for id in mcp_auto_start_ids {
                        let state = app_handle.state::<AppState>();
                        if let Err(error) =
                            application::runtime::start_mcp_runtime(&state, &id).await
                        {
                            eprintln!("自動恢復 workspace {id} 的 MCP 失敗：{error}");
                        }
                    }
                    for id in actions_auto_start_ids {
                        let state = app_handle.state::<AppState>();
                        if let Err(error) =
                            application::runtime::start_actions_runtime(&state, &id).await
                        {
                            eprintln!("自動恢復 workspace {id} 的 Actions 失敗：{error}");
                        }
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if matches!(
                event,
                tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. }
            ) {
                window_corners::refresh(window);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.hide().is_ok() {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_workspaces,
            local_chat,
            list_history_sessions,
            read_history_session,
            read_workspace_telemetry,
            read_workspace_activity,
            read_workspace_activity_detail,
            clear_workspace_activity,
            get_windows_build,
            create_workspace,
            export_workspace_pack,
            export_shared_workspace,
            import_workspace_pack,
            open_shared_workspace,
            list_wsl_distributions,
            list_sandbox_backends,
            update_workspace,
            get_workspace_skills,
            set_workspace_skills_active,
            set_workspace_skill_enabled,
            get_workspace_extensions,
            set_workspace_extension_active,
            set_workspace_extension_enabled,
            add_workspace_folder,
            add_wsl_workspace_folder,
            remove_workspace_folder,
            open_workspace_directory,
            delete_workspace,
            start_runtime,
            stop_runtime,
            get_runtime_status,
            start_actions_runtime,
            stop_actions_runtime,
            get_actions_runtime_status,
            restart_runtime,
            restart_actions_runtime,
            get_frp_snippet,
            start_tunnel,
            stop_tunnel,
            run_health_checks,
            get_workspace_secret,
            set_workspace_secret,
            regenerate_workspace_secret,
            get_shared_secret,
            set_shared_secret,
            regenerate_shared_secret,
            read_workspace_logs,
            list_frp_profiles,
            save_frp_profile,
            delete_frp_profile,
            get_app_settings,
            restart_tunnel,
            test_tunnel,
            set_last_workspace,
            get_last_workspace_id,
            list_software,
            install_software,
            uninstall_software,
            get_download_config,
            set_download_config,
            get_proxy,
            set_proxy,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
