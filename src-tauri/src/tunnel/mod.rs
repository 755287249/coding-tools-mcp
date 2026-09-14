mod access;
mod builtin;
mod cloudflare;
mod download;
mod frp;
mod software;
mod supervisor;

use crate::settings::AppSettings;
use crate::workspace::WorkspaceProfile;

pub use access::{
    cleanup_orphan_for_runtime, drop_workspace, maybe_start_for_runtime, public_mcp_security_error,
    stop_for_runtime, supervisor, sync_managed_runtime_routes,
};

#[allow(unused_imports)]
pub use cloudflare::{
    extract_trycloudflare_url, resolve_cloudflared, spawn_cloudflare_tunnel, stop_child,
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

pub fn frp_snippet(profile: &WorkspaceProfile, kind: TunnelServiceKind) -> String {
    let settings = AppSettings::load_or_default();
    frp::frp_snippet(profile, kind, &settings)
}
