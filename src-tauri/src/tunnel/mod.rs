mod access;
mod builtin;
mod cloudflare;
mod download;
mod frp;
mod software;
mod supervisor;

use std::path::{Path, PathBuf};

use crate::settings::AppSettings;
use crate::workspace::WorkspaceProfile;

pub use access::{
    cleanup_orphan_for_runtime, drop_workspace, maybe_start_for_runtime, public_mcp_security_error,
    stop_for_runtime, supervisor, sync_managed_runtime_routes,
};

#[allow(unused_imports)]
pub use cloudflare::{
    cloudflare_token_tunnel_id, extract_trycloudflare_url, normalize_cloudflare_token,
    resolve_cloudflared, spawn_cloudflare_tunnel, stop_child,
};
#[allow(unused_imports)]
pub use frp::{actions_frp_snippet, mcp_frp_snippet};
#[allow(unused_imports)]
pub use software::{install_software, list_software, uninstall_software, SoftwareStatus};
#[allow(unused_imports)]
pub use supervisor::{
    append_profile_log, append_profile_log_buffered, append_profile_log_rotating,
    append_profile_log_rotating_checked, log_dir_for_profile, TunnelServiceKind, TunnelStatus,
    TunnelSupervisor,
};

pub(crate) use builtin::behavioral_parity_fixture as builtin_behavioral_parity_fixture;

/// Working directory for tunnel helper processes (cloudflared, frpc).
///
/// The global MCP connection introduced in 0.1.142 has no project directory,
/// so its `path` is empty. Windows rejects an empty or otherwise invalid
/// current directory in `CreateProcessW` with os error 123
/// ("The filename, directory name, or volume label syntax is incorrect").
/// The helpers do not read anything from their working directory, so use the
/// candidate only when it is an existing directory and otherwise fall back to
/// a directory the caller has already created (the profile log directory).
pub(crate) fn process_working_dir(candidate: &str, fallback: &Path) -> PathBuf {
    let trimmed = candidate.trim();
    if !trimmed.is_empty() {
        let path = Path::new(trimmed);
        if path.is_dir() {
            return path.to_path_buf();
        }
    }
    fallback.to_path_buf()
}

pub fn frp_snippet(profile: &WorkspaceProfile, kind: TunnelServiceKind) -> String {
    let settings = AppSettings::load_or_default();
    frp::frp_snippet(profile, kind, &settings)
}

#[cfg(test)]
mod process_working_dir_tests {
    use super::process_working_dir;

    #[test]
    fn empty_global_connection_path_uses_fallback() {
        let fallback = tempfile::tempdir().unwrap();
        assert_eq!(process_working_dir("", fallback.path()), fallback.path());
        assert_eq!(process_working_dir("   ", fallback.path()), fallback.path());
    }

    #[test]
    fn missing_or_file_path_uses_fallback() {
        let root = tempfile::tempdir().unwrap();
        let fallback = root.path().join("logs");
        std::fs::create_dir_all(&fallback).unwrap();
        let missing = root.path().join("missing");
        assert_eq!(
            process_working_dir(missing.to_str().unwrap(), &fallback),
            fallback
        );
        let file = root.path().join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            process_working_dir(file.to_str().unwrap(), &fallback),
            fallback
        );
    }

    #[test]
    fn existing_project_directory_is_kept() {
        let project = tempfile::tempdir().unwrap();
        let fallback = tempfile::tempdir().unwrap();
        assert_eq!(
            process_working_dir(project.path().to_str().unwrap(), fallback.path()),
            project.path()
        );
    }

    #[test]
    fn spawn_with_fallback_directory_succeeds_for_empty_path() {
        let fallback = tempfile::tempdir().unwrap();
        let cwd = process_working_dir("", fallback.path());
        #[cfg(windows)]
        let mut command = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "exit", "0"]);
            c
        };
        #[cfg(not(windows))]
        let mut command = std::process::Command::new("true");
        let status = command.current_dir(&cwd).status().unwrap();
        assert!(status.success());
    }
}
