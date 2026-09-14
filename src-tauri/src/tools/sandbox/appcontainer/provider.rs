use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::{Command, ExitStatus};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, HLOCAL};
use windows::Win32::Security::Authorization::{
    BuildTrusteeWithSidW, ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW,
    SetNamedSecurityInfoW, SetSecurityInfo, EXPLICIT_ACCESS_W, GRANT_ACCESS, REVOKE_ACCESS,
    SET_ACCESS, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    AddAce, DeriveCapabilitySidsFromName, EqualSid, GetAce, GetSecurityDescriptorControl,
    InitializeAcl, InitializeSecurityDescriptor, SetFileSecurityW, SetSecurityDescriptorDacl,
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACE_REVISION, ACL, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, NO_INHERITANCE, OBJECT_INHERIT_ACE,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_DESCRIPTOR,
    SE_DACL_PROTECTED, UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

use crate::platform::platform;
use crate::tools::workspace::{Workspace, WorkspaceError, WorkspaceResult};
use crate::workspace::{SandboxPathAccess, SandboxPathGrant};

use super::{sid_string, AppContainerProfile};
use crate::tools::process_spec::ProcessLaunchSpec;

static APPCONTAINER_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static APPCONTAINER_RUNTIME_GRANT_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static APPCONTAINER_RUNTIME_HELPER_INVOCATIONS: AtomicU64 = AtomicU64::new(0);
const APPCONTAINER_ACL_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const APPCONTAINER_RUNTIME_ACL_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const APPCONTAINER_ACL_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const APPCONTAINER_ACL_KILL_GRACE: Duration = Duration::from_millis(500);
const APPCONTAINER_ACL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const APPCONTAINER_ACL_HELPER_SWITCH: &str = "--ctmcp-appcontainer-acl-helper";
const APPCONTAINER_ACL_HELPER_EXE_ENV: &str = "CTMCP_APPCONTAINER_ACL_HELPER_EXE";
const APPCONTAINER_RUNTIME_CAPABILITY_NAME: &str = "CodingToolsMcp.Sandbox.Runtime.ReadExecute.v1";
const APPCONTAINER_RUNTIME_GRANT_MARKER_DIR: &str = "runtime-grants-v1";
const APPCONTAINER_PROTECTED_METADATA_MARKER_DIR: &str = "protected-repository-v1";
const APPCONTAINER_PROTECTED_METADATA_CAPABILITY_PREFIX: &str =
    "CodingToolsMcp.Sandbox.Repository.ReadExecute.v1";
const APPCONTAINER_WORKSPACE_GRANT_MARKER_DIR: &str = "workspace-grants-v1";
const APPCONTAINER_WORKSPACE_MODIFY_CAPABILITY_PREFIX: &str =
    "CodingToolsMcp.Sandbox.Workspace.Modify.v1";

pub(crate) fn run_acl_helper_if_requested() -> Option<i32> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let Some(first) = args.next() else {
        return None;
    };
    if first != std::ffi::OsStr::new(APPCONTAINER_ACL_HELPER_SWITCH) {
        return None;
    }
    Some(match run_acl_helper_command(args.collect()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("AppContainer ACL helper failed: {error}");
            20
        }
    })
}

fn run_acl_helper_command(args: Vec<OsString>) -> WorkspaceResult<()> {
    let mut args = args.into_iter();
    let operation = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| {
            provider_error(
                "SANDBOX_ACL_HELPER_INVALID",
                "ACL helper operation is required.",
            )
        })?;
    let path = args.next().map(PathBuf::from).ok_or_else(|| {
        provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper path is required.")
    })?;
    match operation.as_str() {
        "grant" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            let access = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantAccess::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper grant access is invalid.",
                    )
                })?;
            let inheritance = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantInheritance::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper inheritance is invalid.",
                    )
                })?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper grant received unexpected arguments.",
                ));
            }
            apply_acl_grant_direct(&path, &sid, access, inheritance).map(|_| ())
        }
        "grant_set_file_security" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            let access = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantAccess::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper grant access is invalid.",
                    )
                })?;
            let inheritance = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantInheritance::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper inheritance is invalid.",
                    )
                })?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper SetFileSecurity grant received unexpected arguments.",
                ));
            }
            apply_acl_grant_with_set_file_security_direct(&path, &sid, access, inheritance)
                .map(|_| ())
        }
        "grant_via_handle" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            let access = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantAccess::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper grant access is invalid.",
                    )
                })?;
            let inheritance = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantInheritance::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper inheritance is invalid.",
                    )
                })?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper handle-based grant received unexpected arguments.",
                ));
            }
            apply_acl_grant_via_handle_direct(&path, &sid, access, inheritance).map(|_| ())
        }
        "set_via_handle" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            let access = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantAccess::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper set access is invalid.",
                    )
                })?;
            let inheritance = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| AclGrantInheritance::from_helper_arg(&value))
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper set inheritance is invalid.",
                    )
                })?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper handle-based set received unexpected arguments.",
                ));
            }
            apply_acl_set_via_handle_direct(&path, &sid, access, inheritance).map(|_| ())
        }
        "revoke" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper revoke received unexpected arguments.",
                ));
            }
            revoke_acl_grant_direct(&path, &sid)
        }
        "revoke_via_handle" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper handle-based revoke received unexpected arguments.",
                ));
            }
            revoke_acl_grant_via_handle_direct(&path, &sid)
        }
        "revoke_set_file_security" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper SetFileSecurity revoke received unexpected arguments.",
                ));
            }
            revoke_acl_grant_with_set_file_security_direct(&path, &sid)
        }
        "filter_sid_set_file_security" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper SID filter received unexpected arguments.",
                ));
            }
            filter_allowed_sid_with_set_file_security_direct(&path, &sid).map(|_| ())
        }
        "filter_sid_tree_set_file_security" => {
            let sid_text = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error("SANDBOX_ACL_HELPER_INVALID", "ACL helper SID is required.")
                })?;
            let sid = SharedSid::from_text(&sid_text)?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper SID tree filter received unexpected arguments.",
                ));
            }
            let stats = filter_allowed_sid_tree_with_set_file_security_direct(&path, &sid)?;
            println!(
                "visited={} changed={} skipped_reparse={}",
                stats.visited, stats.changed, stats.skipped_reparse
            );
            Ok(())
        }
        "protect" => {
            let mut sids = Vec::new();
            for value in args {
                let sid_text = value.into_string().map_err(|_| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper protect SID is invalid UTF-16.",
                    )
                })?;
                sids.push(SharedSid::from_text(&sid_text)?);
            }
            if sids.is_empty() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper protect requires at least one SID.",
                ));
            }
            apply_protected_acl_direct(&path, &sids)
        }
        "restore_dacl" => {
            let protected = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| match value.as_str() {
                    "protected" => Some(true),
                    "inheriting" => Some(false),
                    _ => None,
                })
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper restore protection mode is invalid.",
                    )
                })?;
            let encoded = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper restore DACL snapshot is required.",
                    )
                })?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper restore received unexpected arguments.",
                ));
            }
            let dacl = SavedDacl::from_helper_arg(&encoded)?;
            restore_named_dacl(&path, &dacl, protected)
        }
        "restore_dacl_via_handle" => {
            let protected = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| match value.as_str() {
                    "protected" => Some(true),
                    "inheriting" => Some(false),
                    _ => None,
                })
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper restore protection mode is invalid.",
                    )
                })?;
            let encoded = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| {
                    provider_error(
                        "SANDBOX_ACL_HELPER_INVALID",
                        "ACL helper restore DACL snapshot is required.",
                    )
                })?;
            if args.next().is_some() {
                return Err(provider_error(
                    "SANDBOX_ACL_HELPER_INVALID",
                    "ACL helper handle-based restore received unexpected arguments.",
                ));
            }
            let dacl = SavedDacl::from_helper_arg(&encoded)?;
            write_saved_dacl_via_handle(
                &path,
                &dacl,
                protected,
                "SANDBOX_ACL_RESTORE_FAILED",
                "restore DACL without propagation",
            )
        }
        _ => Err(provider_error(
            "SANDBOX_ACL_HELPER_INVALID",
            format!("Unknown ACL helper operation: {operation}"),
        )),
    }
}

fn acl_helper_executable() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os(APPCONTAINER_ACL_HELPER_EXE_ENV) {
        if !configured.is_empty() {
            return Some(PathBuf::from(configured));
        }
    }
    #[cfg(not(test))]
    {
        return std::env::current_exe().ok();
    }
    #[cfg(test)]
    {
        None
    }
}

fn run_acl_helper_process_args(
    operation: &str,
    path: &Path,
    args: &[OsString],
    timeout: Duration,
    timeout_code: &'static str,
    failure_code: &'static str,
) -> WorkspaceResult<()> {
    let executable = acl_helper_executable().ok_or_else(|| {
        provider_error(
            "SANDBOX_ACL_HELPER_UNAVAILABLE",
            "AppContainer ACL helper executable is unavailable.",
        )
    })?;
    let mut command = Command::new(&executable);
    command
        .arg(APPCONTAINER_ACL_HELPER_SWITCH)
        .arg(operation)
        .arg(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let (status, helper_stderr) = command_status_and_stderr_with_timeout(&mut command, timeout)
        .map_err(|error| {
            provider_error(
                if error.kind() == std::io::ErrorKind::TimedOut {
                    timeout_code
                } else {
                    failure_code
                },
                format!(
                    "AppContainer ACL helper '{}' failed for {}: {error}",
                    executable.display(),
                    path.display()
                ),
            )
        })?;
    if !status.success() {
        let detail = helper_stderr.trim();
        return Err(provider_error(
            failure_code,
            if detail.is_empty() {
                format!(
                    "AppContainer ACL helper '{}' exited with {status} for {}.",
                    executable.display(),
                    path.display()
                )
            } else {
                format!(
                    "AppContainer ACL helper '{}' exited with {status} for {}: {detail}",
                    executable.display(),
                    path.display()
                )
            },
        ));
    }
    Ok(())
}

fn run_acl_helper_process(
    operation: &str,
    path: &Path,
    sid: &str,
    access: Option<AclGrantAccess>,
    inheritance: Option<AclGrantInheritance>,
    timeout: Duration,
) -> WorkspaceResult<()> {
    let mut args = vec![OsString::from(sid)];
    if let Some(access) = access {
        args.push(OsString::from(access.helper_arg()));
    }
    if let Some(inheritance) = inheritance {
        args.push(OsString::from(inheritance.helper_arg()));
    }
    run_acl_helper_process_args(
        operation,
        path,
        &args,
        timeout,
        "SANDBOX_ACL_GRANT_TIMEOUT",
        "SANDBOX_ACL_HELPER_FAILED",
    )
}

fn run_persistent_runtime_acl_helper_process(
    path: &Path,
    sid: &Arc<SharedSid>,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<()> {
    #[cfg(test)]
    APPCONTAINER_RUNTIME_HELPER_INVOCATIONS.fetch_add(1, Ordering::Relaxed);
    let args = vec![
        OsString::from(sid_string(sid.sid())?),
        OsString::from(access.helper_arg()),
        OsString::from(inheritance.helper_arg()),
    ];
    run_acl_helper_process_args(
        "set_via_handle",
        path,
        &args,
        APPCONTAINER_RUNTIME_ACL_INSTALL_TIMEOUT,
        "SANDBOX_RUNTIME_GRANT_TIMEOUT",
        "SANDBOX_RUNTIME_GRANT_FAILED",
    )
}
fn run_protected_acl_helper_process(
    path: &Path,
    sids: &[PSID],
    timeout: Duration,
) -> WorkspaceResult<()> {
    let args = sids
        .iter()
        .map(|sid| sid_string(*sid).map(OsString::from))
        .collect::<WorkspaceResult<Vec<_>>>()?;
    run_acl_helper_process_args(
        "protect",
        path,
        &args,
        timeout,
        "SANDBOX_ACL_RESTRICT_TIMEOUT",
        "SANDBOX_ACL_RESTRICT_FAILED",
    )
}

fn command_status_and_stderr_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<(ExitStatus, String)> {
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Ok((status, stderr));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let kill_started = Instant::now();
            while kill_started.elapsed() < APPCONTAINER_ACL_KILL_GRACE {
                if child.try_wait()?.is_some() {
                    break;
                }
                thread::sleep(APPCONTAINER_ACL_POLL_INTERVAL);
            }
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let detail = stderr.trim();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                if detail.is_empty() {
                    format!("command timed out after {} ms", timeout.as_millis())
                } else {
                    format!(
                        "command timed out after {} ms; helper stderr: {detail}",
                        timeout.as_millis()
                    )
                },
            ));
        }
        thread::sleep(APPCONTAINER_ACL_POLL_INTERVAL);
    }
}

fn appcontainer_identity_suffix_at(timestamp_nanos: u128) -> String {
    let sequence = APPCONTAINER_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}.{}.{}", std::process::id(), timestamp_nanos, sequence)
}

fn appcontainer_identity_suffix() -> WorkspaceResult<String> {
    let timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| provider_error("SANDBOX_IDENTITY_FAILED", error.to_string()))?
        .as_nanos();
    Ok(appcontainer_identity_suffix_at(timestamp_nanos))
}
use crate::tools::sandbox::{
    PreparedSandbox, SandboxCommand, SandboxProcessPlan, SandboxStateLayout,
    SandboxStatePersistence, APPCONTAINER_DEFAULT_NETWORK, APPCONTAINER_NETWORK_OPTION_ID,
};

pub(in crate::tools::sandbox) fn prepare(
    workspace: &Workspace,
    external_paths: &[SandboxPathGrant],
    options: &BTreeMap<String, String>,
) -> WorkspaceResult<Box<dyn PreparedSandbox>> {
    let state_root = managed_state_root(workspace)?;
    let runtime_grant_marker_root = managed_runtime_grant_marker_root()?;
    prepare_with_state_root_external_paths_and_marker(
        workspace,
        state_root,
        runtime_grant_marker_root,
        external_paths,
        options,
    )
}

#[cfg(test)]
fn prepare_with_state_root(
    workspace: &Workspace,
    state_root: PathBuf,
) -> WorkspaceResult<Box<dyn PreparedSandbox>> {
    prepare_with_state_root_and_external_paths(workspace, state_root, &[], &BTreeMap::new())
}

#[cfg(test)]
fn prepare_with_state_root_and_external_paths(
    workspace: &Workspace,
    state_root: PathBuf,
    external_paths: &[SandboxPathGrant],
    options: &BTreeMap<String, String>,
) -> WorkspaceResult<Box<dyn PreparedSandbox>> {
    let runtime_grant_marker_root = runtime_grant_marker_root(&state_root);
    prepare_with_state_root_external_paths_and_marker(
        workspace,
        state_root,
        runtime_grant_marker_root,
        external_paths,
        options,
    )
}

fn prepare_with_state_root_external_paths_and_marker(
    workspace: &Workspace,
    state_root: PathBuf,
    runtime_grant_marker_root: PathBuf,
    external_paths: &[SandboxPathGrant],
    options: &BTreeMap<String, String>,
) -> WorkspaceResult<Box<dyn PreparedSandbox>> {
    if workspace.is_wsl() {
        return Err(provider_error(
            "SANDBOX_BACKEND_UNSUPPORTED",
            "Windows AppContainer cannot prepare a WSL execution target. Use Docker Sandboxes or WSL Containers for WSL folders.",
        ));
    }
    let network = selected_network(options)?;

    fs::create_dir_all(&state_root).map_err(|error| {
        provider_error(
            "SANDBOX_STATE_PREPARE_FAILED",
            format!(
                "Failed to create sandbox state {}: {error}",
                state_root.display()
            ),
        )
    })?;
    let state = state_layout(&state_root);
    let environment = state_environment(&state_root)?;

    let identity = appcontainer_identity_suffix()?;
    let capability = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)?;
    let runtime_capability_sid = Arc::new(SharedSid::copy_from(capability.sid())?);
    let workspace_capability =
        DerivedCapability::derive(&workspace_modify_capability_name(workspace.root())?)?;
    let workspace_capability_sid = Arc::new(SharedSid::copy_from(workspace_capability.sid())?);
    let metadata_capability =
        DerivedCapability::derive(&protected_metadata_capability_name(workspace.root())?)?;
    let metadata_capability_sid = Arc::new(SharedSid::copy_from(metadata_capability.sid())?);
    // Repository metadata uses the stable read/execute capability instead of the per-session
    // package SID. Installing this boundary once keeps large .git trees out of the hot prepare
    // path while still preventing the workspace Modify grant below from flowing into them.
    prepare_protected_asset_restrictions(
        workspace.root(),
        &runtime_grant_marker_root,
        &metadata_capability_sid,
    )?;
    ensure_persistent_workspace_grant(
        &runtime_grant_marker_root,
        workspace.root(),
        &workspace_capability_sid,
    )?;
    let moniker = format!("CodingToolsMcp.Sandbox.Workspace.{identity}");
    let profile = Arc::new(AppContainerProfile::create(
        &moniker,
        Some(capability.sid()),
    )?);
    let package_sid = Arc::new(SharedSid::copy_from(profile.sid())?);

    #[cfg(test)]
    if std::env::var_os("CTMCP_TEST_TRACE_WORKSPACE_ACL").is_some() {
        let workspace_ace = allowed_ace_for_sid(workspace.root(), workspace_capability.sid())?;
        let capability_ace = allowed_ace_for_sid(workspace.root(), capability.sid())?;
        eprintln!(
            "workspace-acl workspace_sid={} workspace_ace={workspace_ace:?} capability_sid={} capability_ace={capability_ace:?}",
            sid_string(workspace_capability.sid())?,
            sid_string(capability.sid())?
        );
    }
    let state_grant = TemporaryAclGrant::apply(
        state_root,
        &package_sid,
        AclGrantAccess::Modify,
        AclGrantInheritance::Children,
    )?;
    let external_grants =
        prepare_external_path_grants(external_paths, &package_sid, workspace.root())?;

    let internet = match network {
        AppContainerNetwork::None => None,
        AppContainerNetwork::Internet => Some(DerivedCapability::derive("internetClient")?),
    };

    let lease = Arc::new(AppContainerLease {
        profile,
        capability,
        workspace_capability,
        metadata_capability,
        runtime_capability_sid,
        internet,
        _state_grant: state_grant,
        _external_grants: external_grants,
        runtime_grants: Mutex::new(BTreeSet::new()),
    });

    Ok(Box::new(AppContainerPreparedSandbox {
        lease,
        workspace_root: workspace.root().to_path_buf(),
        state,
        environment,
        runtime_grant_marker_root,
    }))
}

struct AppContainerPreparedSandbox {
    lease: Arc<AppContainerLease>,
    workspace_root: PathBuf,
    state: SandboxStateLayout,
    environment: BTreeMap<String, String>,
    runtime_grant_marker_root: PathBuf,
}

impl PreparedSandbox for AppContainerPreparedSandbox {
    fn backend_id(&self) -> &str {
        "appcontainer"
    }

    fn state_layout(&self) -> Option<&SandboxStateLayout> {
        Some(&self.state)
    }

    fn normalize_logical_command(
        &self,
        command: SandboxCommand,
    ) -> WorkspaceResult<SandboxCommand> {
        self.normalize_logical_runtime(command)
    }

    fn prepare_process(
        &self,
        mut process: ProcessLaunchSpec,
    ) -> WorkspaceResult<SandboxProcessPlan> {
        let grants = self.prepare_concrete_runtime(&mut process)?;
        self.ensure_runtime_grants(grants)?;
        Ok(SandboxProcessPlan {
            backend_id: self.backend_id().into(),
            process,
            environment_overrides: self.environment.clone(),
            state: Some(self.state.clone()),
        })
    }

    fn launch_prepared_process(
        &self,
        plan: SandboxProcessPlan,
    ) -> WorkspaceResult<crate::tools::process_child::ProcessChild> {
        if plan.backend_id != self.backend_id() {
            return Err(provider_error(
                "SANDBOX_PROCESS_PLAN_INVALID",
                format!(
                    "Prepared process backend '{}' does not match '{}'.",
                    plan.backend_id,
                    self.backend_id()
                ),
            ));
        }
        let mut capability_sids = vec![
            self.lease.capability.sid(),
            self.lease.workspace_capability.sid(),
            self.lease.metadata_capability.sid(),
        ];
        if let Some(internet) = &self.lease.internet {
            capability_sids.push(internet.sid());
        }
        let child = super::launch_process(
            &plan.process,
            &plan.environment_overrides,
            self.lease.profile.clone(),
            &capability_sids,
        )?;
        Ok(child.with_backend_lifetime(self.lease.clone()))
    }

    fn environment_overrides(&self) -> WorkspaceResult<BTreeMap<String, String>> {
        Ok(self.environment.clone())
    }
}

impl AppContainerPreparedSandbox {
    fn normalize_logical_runtime(
        &self,
        mut command: SandboxCommand,
    ) -> WorkspaceResult<SandboxCommand> {
        let resolved_executable = command
            .executable
            .canonicalize()
            .unwrap_or_else(|_| command.executable.clone());
        if resolved_executable.starts_with(&self.workspace_root) {
            command.executable = resolved_executable;
            return Ok(command);
        }

        let name = file_name_lowercase(&command.executable);
        if !matches!(name.as_str(), "npm" | "npm.cmd") {
            return Ok(command);
        }

        let node_root = node_runtime_root(&command.executable)?;
        let node_physical_root = resolve_reparse_target(&node_root)?;
        let npm_cli = node_physical_root
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js");
        if !npm_cli.exists() {
            return Err(provider_error(
                "SANDBOX_RUNTIME_NOT_FOUND",
                format!("npm runtime path not found: {}", npm_cli.display()),
            ));
        }
        command.executable = node_physical_root.join("node.exe");
        let mut args = vec![
            "--preserve-symlinks".into(),
            "--preserve-symlinks-main".into(),
            npm_cli.to_string_lossy().into_owned(),
        ];
        args.extend(command.args);
        command.args = args;
        Ok(command)
    }

    fn prepare_concrete_runtime(
        &self,
        process: &mut ProcessLaunchSpec,
    ) -> WorkspaceResult<Vec<RuntimeGrantSpec>> {
        if process.using_wsl {
            return Err(provider_error(
                "SANDBOX_BACKEND_UNSUPPORTED",
                "Windows AppContainer cannot launch a WSL-normalized process.",
            ));
        }

        let resolved_program = process
            .program
            .canonicalize()
            .unwrap_or_else(|_| process.program.clone());
        if resolved_program.starts_with(&self.workspace_root) {
            process.program = resolved_program;
            return Ok(Vec::new());
        }

        if let Some(trusted_program) = trusted_windows_runtime(&process.program)? {
            process.program = trusted_program;
            return Ok(Vec::new());
        }

        let name = file_name_lowercase(&process.program);
        if matches!(name.as_str(), "cargo" | "cargo.exe" | "rustc" | "rustc.exe") {
            let tool = if name.starts_with("cargo") {
                "cargo"
            } else {
                "rustc"
            };
            let executable = rustup_which(tool).ok_or_else(|| {
                provider_error(
                    "SANDBOX_RUNTIME_NOT_FOUND",
                    format!("rustup could not resolve {tool}"),
                )
            })?;
            let rust_bin = executable
                .parent()
                .ok_or_else(|| provider_error("SANDBOX_RUNTIME_INVALID", "Rust tool has no bin"))?
                .to_path_buf();
            let rust_lib = rust_toolchain_root(&executable)
                .ok_or_else(|| {
                    provider_error("SANDBOX_RUNTIME_INVALID", "Rust toolchain root not found")
                })?
                .join("lib");
            process.program = executable;
            return Ok(vec![
                RuntimeGrantSpec::new(
                    rust_bin,
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::Children,
                ),
                RuntimeGrantSpec::new(
                    rust_lib,
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::Children,
                ),
            ]);
        }

        if name == "node" || name == "node.exe" {
            return self.prepare_node_runtime(process);
        }

        if name.starts_with("python") && name.ends_with(".exe") {
            let original_program = process.program.clone();
            let python_root = process
                .program
                .parent()
                .ok_or_else(|| provider_error("SANDBOX_RUNTIME_INVALID", "python has no parent"))?
                .to_path_buf();
            let venv_root = original_program
                .parent()
                .and_then(Path::parent)
                .filter(|root| root.join("pyvenv.cfg").is_file())
                .map(Path::to_path_buf);
            let venv = match venv_root.as_deref() {
                Some(root) => python_venv_config(root)?,
                None => None,
            };
            let physical = resolve_reparse_target(&python_root)?;
            let file_name = process
                .program
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("python.exe"))
                .to_owned();

            if let (Some(venv_root), Some(venv)) = (venv_root.as_deref(), venv.as_ref()) {
                if venv.uv_managed {
                    let base_physical = resolve_reparse_target(&venv.home)?;
                    // Keep uv's venv launcher as the process entrypoint. It is responsible
                    // for preserving sys.prefix/sys.executable and may open/spawn the base
                    // runtime itself; grant that concrete runtime tree instead of bypassing
                    // the launcher and losing virtual-environment identity.
                    process.program = original_program;
                    return Ok(vec![
                        RuntimeGrantSpec::new(
                            venv_root.to_path_buf(),
                            AclGrantAccess::ReadExecute,
                            AclGrantInheritance::None,
                        ),
                        RuntimeGrantSpec::new(
                            venv_root.to_path_buf(),
                            AclGrantAccess::ReadExecute,
                            AclGrantInheritance::Children,
                        ),
                        RuntimeGrantSpec::new(
                            venv.home.clone(),
                            AclGrantAccess::ReadExecute,
                            AclGrantInheritance::None,
                        ),
                        RuntimeGrantSpec::new(
                            base_physical.clone(),
                            AclGrantAccess::ReadExecute,
                            AclGrantInheritance::None,
                        ),
                        RuntimeGrantSpec::new(
                            base_physical,
                            AclGrantAccess::ReadExecute,
                            AclGrantInheritance::Children,
                        ),
                    ]);
                }
            }

            if physical != python_root {
                process.program = physical.join(file_name);
            }
            let mut grants = vec![
                RuntimeGrantSpec::new(
                    python_root,
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::None,
                ),
                RuntimeGrantSpec::new(
                    physical,
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::Children,
                ),
            ];
            if let Some(venv_root) = venv_root.as_deref() {
                grants.push(RuntimeGrantSpec::new(
                    venv_root.to_path_buf(),
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::None,
                ));
                grants.push(RuntimeGrantSpec::new(
                    venv_root.to_path_buf(),
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::Children,
                ));
                if let Some(venv) = venv.as_ref() {
                    let base_physical = resolve_reparse_target(&venv.home)?;
                    grants.push(RuntimeGrantSpec::new(
                        venv.home.clone(),
                        AclGrantAccess::ReadExecute,
                        AclGrantInheritance::None,
                    ));
                    grants.push(RuntimeGrantSpec::new(
                        base_physical.clone(),
                        AclGrantAccess::ReadExecute,
                        AclGrantInheritance::None,
                    ));
                    grants.push(RuntimeGrantSpec::new(
                        base_physical,
                        AclGrantAccess::ReadExecute,
                        AclGrantInheritance::Children,
                    ));
                }
            }
            return Ok(grants);
        }

        Err(provider_error(
            "SANDBOX_RUNTIME_UNSUPPORTED",
            format!(
                "AppContainer runtime adapter is not implemented for concrete program '{}'.",
                process.program.display()
            ),
        ))
    }

    fn prepare_node_runtime(
        &self,
        process: &mut ProcessLaunchSpec,
    ) -> WorkspaceResult<Vec<RuntimeGrantSpec>> {
        let discovered_root = node_runtime_root(&process.program)?;
        let physical = resolve_reparse_target(&discovered_root)?;
        process.program = physical.join("node.exe");

        let mut grants = vec![
            RuntimeGrantSpec::new(
                discovered_root,
                AclGrantAccess::ReadExecute,
                AclGrantInheritance::None,
            ),
            RuntimeGrantSpec::new(
                physical.clone(),
                AclGrantAccess::ReadExecute,
                AclGrantInheritance::Children,
            ),
        ];
        if let Some(npm_cli) = process
            .args
            .iter()
            .map(PathBuf::from)
            .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("npm-cli.js"))
        {
            let npm_root = npm_cli
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| {
                    provider_error("SANDBOX_RUNTIME_INVALID", "npm-cli.js has no npm root")
                })?
                .to_path_buf();
            let node_modules = npm_root
                .parent()
                .ok_or_else(|| {
                    provider_error("SANDBOX_RUNTIME_INVALID", "npm root has no node_modules")
                })?
                .to_path_buf();
            grants.push(RuntimeGrantSpec::new(
                node_modules,
                AclGrantAccess::ReadExecute,
                AclGrantInheritance::None,
            ));
            grants.push(RuntimeGrantSpec::new(
                npm_root,
                AclGrantAccess::ReadExecute,
                AclGrantInheritance::Children,
            ));
        }
        Ok(grants)
    }

    fn ensure_runtime_grants(&self, specs: Vec<RuntimeGrantSpec>) -> WorkspaceResult<()> {
        if specs.is_empty() {
            return Ok(());
        }
        let mut grants = self.lease.runtime_grants.lock().map_err(|_| {
            provider_error(
                "SANDBOX_RUNTIME_GRANT_FAILED",
                "Runtime grant registry lock is poisoned.",
            )
        })?;
        for spec in canonical_runtime_grants(&self.workspace_root, specs)? {
            let key = persistent_runtime_grant_key(
                &spec.path,
                self.lease.runtime_capability_sid.sid(),
                spec.access,
                spec.inheritance,
            )?;
            if grants.contains(&key) {
                continue;
            }
            ensure_persistent_runtime_grant(
                &self.runtime_grant_marker_root,
                &spec.path,
                &self.lease.runtime_capability_sid,
                spec.access,
                spec.inheritance,
                &key,
            )?;
            grants.insert(key);
        }
        Ok(())
    }
}

fn canonical_runtime_grants(
    workspace_root: &Path,
    specs: Vec<RuntimeGrantSpec>,
) -> WorkspaceResult<Vec<RuntimeGrantSpec>> {
    let mut canonical = Vec::with_capacity(specs.len());
    for spec in specs {
        if spec.access != AclGrantAccess::ReadExecute {
            return Err(provider_error(
                "SANDBOX_RUNTIME_GRANT_INVALID",
                "Shared runtime capability grants must remain read/execute-only.",
            ));
        }
        let path = spec.path.canonicalize().map_err(|error| {
            provider_error(
                "SANDBOX_RUNTIME_GRANT_FAILED",
                format!(
                    "Runtime sandbox grant target cannot be resolved: {}: {error}",
                    spec.path.display()
                ),
            )
        })?;
        if protected_repository_metadata_path(workspace_root, &path) {
            return Err(provider_error(
                "SANDBOX_RUNTIME_GRANT_PROTECTED",
                format!(
                    "Runtime sandbox grant cannot target protected repository metadata: {}",
                    path.display()
                ),
            ));
        }
        canonical.push(RuntimeGrantSpec::new(path, spec.access, spec.inheritance));
    }
    Ok(dedupe_grants(canonical))
}
#[cfg(test)]
fn runtime_grant_marker_root(state_root: &Path) -> PathBuf {
    state_root
        .parent()
        .unwrap_or(state_root)
        .join(APPCONTAINER_RUNTIME_GRANT_MARKER_DIR)
}

fn persistent_runtime_grant_key(
    path: &Path,
    sid: PSID,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(APPCONTAINER_RUNTIME_CAPABILITY_NAME.as_bytes());
    hasher.update([0]);
    hasher.update(sid_string(sid)?.as_bytes());
    hasher.update([0]);
    hasher.update(
        acl_win32_path(path)
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_bytes(),
    );
    hasher.update([0]);
    hasher.update(access.helper_arg().as_bytes());
    hasher.update([0]);
    hasher.update(inheritance.helper_arg().as_bytes());
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn persistent_protected_metadata_key(path: &Path, sid: PSID) -> WorkspaceResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(APPCONTAINER_PROTECTED_METADATA_MARKER_DIR.as_bytes());
    hasher.update([0]);
    hasher.update(sid_string(sid)?.as_bytes());
    hasher.update([0]);
    hasher.update(
        acl_win32_path(path)
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_bytes(),
    );
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn workspace_scoped_capability_name(
    prefix: &str,
    workspace_root: &Path,
) -> WorkspaceResult<String> {
    let canonical = workspace_root.canonicalize().map_err(|error| {
        provider_error(
            "SANDBOX_ACL_GRANT_FAILED",
            format!(
                "Workspace capability root cannot be resolved: {}: {error}",
                workspace_root.display()
            ),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(
        acl_win32_path(&canonical)
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_bytes(),
    );
    let digest = hasher.finalize();
    let suffix = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}.{suffix}"))
}

fn protected_metadata_capability_name(workspace_root: &Path) -> WorkspaceResult<String> {
    workspace_scoped_capability_name(
        APPCONTAINER_PROTECTED_METADATA_CAPABILITY_PREFIX,
        workspace_root,
    )
}

fn workspace_modify_capability_name(workspace_root: &Path) -> WorkspaceResult<String> {
    workspace_scoped_capability_name(
        APPCONTAINER_WORKSPACE_MODIFY_CAPABILITY_PREFIX,
        workspace_root,
    )
}

fn runtime_acl_has_required_grant(
    path: &Path,
    sid: PSID,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<bool> {
    let permissions = access.permissions();
    let required_flags = inheritance.flags().0 as u8;
    let aces = allowed_aces_for_sid(path, sid)?;
    if aces.iter().any(|ace| (ace.mask & !permissions) != 0) {
        return Ok(false);
    }
    Ok(aces
        .iter()
        .any(|ace| ace.mask == permissions && (ace.flags & required_flags) == required_flags))
}

fn protected_metadata_acl_has_required_grant(path: &Path, sid: PSID) -> WorkspaceResult<bool> {
    let (_, protected) = snapshot_named_dacl(path)?;
    if !protected {
        return Ok(false);
    }
    runtime_acl_has_required_grant(
        path,
        sid,
        AclGrantAccess::ReadExecute,
        if path.is_dir() {
            AclGrantInheritance::Children
        } else {
            AclGrantInheritance::None
        },
    )
}

fn publish_persistent_acl_marker<F>(
    marker_root: &Path,
    marker: &Path,
    key: &str,
    expected_marker: &str,
    error_code: &'static str,
    grant_label: &str,
    verify: F,
) -> WorkspaceResult<()>
where
    F: FnOnce() -> WorkspaceResult<bool>,
{
    let temporary_marker = marker_root.join(format!(".{key}.{}.tmp", std::process::id()));
    fs::write(&temporary_marker, expected_marker.as_bytes()).map_err(|error| {
        provider_error(
            error_code,
            format!(
                "Failed to write {grant_label} marker {}: {error}",
                temporary_marker.display()
            ),
        )
    })?;
    match fs::rename(&temporary_marker, marker) {
        Ok(()) => Ok(()),
        Err(error) => {
            let published_by_peer = fs::read_to_string(marker)
                .map(|value| value == expected_marker)
                .unwrap_or(false)
                && verify()?;
            let _ = fs::remove_file(&temporary_marker);
            if published_by_peer {
                Ok(())
            } else {
                Err(provider_error(
                    error_code,
                    format!(
                        "Failed to publish {grant_label} marker {}: {error}",
                        marker.display()
                    ),
                ))
            }
        }
    }
}

fn ensure_persistent_protected_metadata_grant(
    marker_root: &Path,
    path: &Path,
    sid: &Arc<SharedSid>,
) -> WorkspaceResult<()> {
    let _guard = APPCONTAINER_RUNTIME_GRANT_LOCK.lock().map_err(|_| {
        provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            "Persistent protected-metadata grant lock is poisoned.",
        )
    })?;
    let marker_root = marker_root.join(APPCONTAINER_PROTECTED_METADATA_MARKER_DIR);
    fs::create_dir_all(&marker_root).map_err(|error| {
        provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Failed to create protected-metadata grant state {}: {error}",
                marker_root.display()
            ),
        )
    })?;
    let key = persistent_protected_metadata_key(path, sid.sid())?;
    let marker = marker_root.join(format!("{key}.ready"));
    let expected_marker = format!("{key}\n");
    let marker_matches = fs::read_to_string(&marker)
        .map(|value| value == expected_marker)
        .unwrap_or(false);
    let acl_matches = protected_metadata_acl_has_required_grant(path, sid.sid())?;
    if marker_matches && acl_matches {
        return Ok(());
    }
    if marker.exists() {
        fs::remove_file(&marker).map_err(|error| {
            provider_error(
                "SANDBOX_ACL_RESTRICT_FAILED",
                format!(
                    "Failed to invalidate protected-metadata grant marker {}: {error}",
                    marker.display()
                ),
            )
        })?;
    }
    if acl_matches {
        return publish_persistent_acl_marker(
            &marker_root,
            &marker,
            &key,
            &expected_marker,
            "SANDBOX_ACL_RESTRICT_FAILED",
            "protected-metadata grant",
            || protected_metadata_acl_has_required_grant(path, sid.sid()),
        );
    }

    if acl_helper_executable().is_some() {
        run_protected_acl_helper_process(
            path,
            &[sid.sid()],
            APPCONTAINER_RUNTIME_ACL_INSTALL_TIMEOUT,
        )?;
    } else {
        apply_protected_acl_direct(path, std::slice::from_ref(sid.as_ref()))?;
    }
    if !protected_metadata_acl_has_required_grant(path, sid.sid())? {
        return Err(provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Persistent protected-metadata ACL verification failed for {}.",
                path.display()
            ),
        ));
    }

    publish_persistent_acl_marker(
        &marker_root,
        &marker,
        &key,
        &expected_marker,
        "SANDBOX_ACL_RESTRICT_FAILED",
        "protected-metadata grant",
        || protected_metadata_acl_has_required_grant(path, sid.sid()),
    )
}

fn persistent_workspace_grant_key(path: &Path, sid: PSID) -> WorkspaceResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(APPCONTAINER_WORKSPACE_GRANT_MARKER_DIR.as_bytes());
    hasher.update([0]);
    hasher.update(sid_string(sid)?.as_bytes());
    hasher.update([0]);
    hasher.update(
        acl_win32_path(path)
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_bytes(),
    );
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn ensure_persistent_workspace_grant(
    marker_root: &Path,
    path: &Path,
    sid: &Arc<SharedSid>,
) -> WorkspaceResult<()> {
    let _guard = APPCONTAINER_RUNTIME_GRANT_LOCK.lock().map_err(|_| {
        provider_error(
            "SANDBOX_ACL_GRANT_FAILED",
            "Persistent workspace grant lock is poisoned.",
        )
    })?;
    let marker_root = marker_root.join(APPCONTAINER_WORKSPACE_GRANT_MARKER_DIR);
    fs::create_dir_all(&marker_root).map_err(|error| {
        provider_error(
            "SANDBOX_ACL_GRANT_FAILED",
            format!(
                "Failed to create persistent workspace grant state {}: {error}",
                marker_root.display()
            ),
        )
    })?;
    let key = persistent_workspace_grant_key(path, sid.sid())?;
    let marker = marker_root.join(format!("{key}.ready"));
    let expected_marker = format!("{key}\n");
    let marker_matches = fs::read_to_string(&marker)
        .map(|value| value == expected_marker)
        .unwrap_or(false);
    let acl_matches = runtime_acl_has_required_grant(
        path,
        sid.sid(),
        AclGrantAccess::Modify,
        AclGrantInheritance::Children,
    )?;
    if marker_matches && acl_matches {
        return Ok(());
    }
    if marker.exists() {
        fs::remove_file(&marker).map_err(|error| {
            provider_error(
                "SANDBOX_ACL_GRANT_FAILED",
                format!(
                    "Failed to invalidate persistent workspace grant marker {}: {error}",
                    marker.display()
                ),
            )
        })?;
    }
    if acl_matches {
        return publish_persistent_acl_marker(
            &marker_root,
            &marker,
            &key,
            &expected_marker,
            "SANDBOX_ACL_GRANT_FAILED",
            "workspace grant",
            || {
                runtime_acl_has_required_grant(
                    path,
                    sid.sid(),
                    AclGrantAccess::Modify,
                    AclGrantInheritance::Children,
                )
            },
        );
    }

    if acl_helper_executable().is_some() {
        let sid_text = sid_string(sid.sid())?;
        run_acl_helper_process(
            "set_via_handle",
            path,
            &sid_text,
            Some(AclGrantAccess::Modify),
            Some(AclGrantInheritance::Children),
            APPCONTAINER_RUNTIME_ACL_INSTALL_TIMEOUT,
        )?;
    } else {
        apply_acl_set_via_handle_direct(
            path,
            sid.as_ref(),
            AclGrantAccess::Modify,
            AclGrantInheritance::Children,
        )?;
    }
    if !runtime_acl_has_required_grant(
        path,
        sid.sid(),
        AclGrantAccess::Modify,
        AclGrantInheritance::Children,
    )? {
        return Err(provider_error(
            "SANDBOX_ACL_GRANT_FAILED",
            format!(
                "Persistent workspace ACL verification failed for {}.",
                path.display()
            ),
        ));
    }

    publish_persistent_acl_marker(
        &marker_root,
        &marker,
        &key,
        &expected_marker,
        "SANDBOX_ACL_GRANT_FAILED",
        "workspace grant",
        || {
            runtime_acl_has_required_grant(
                path,
                sid.sid(),
                AclGrantAccess::Modify,
                AclGrantInheritance::Children,
            )
        },
    )
}

fn ensure_persistent_runtime_grant(
    marker_root: &Path,
    path: &Path,
    sid: &Arc<SharedSid>,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
    key: &str,
) -> WorkspaceResult<()> {
    if access != AclGrantAccess::ReadExecute {
        return Err(provider_error(
            "SANDBOX_RUNTIME_GRANT_INVALID",
            "Shared runtime capability grants must remain read/execute-only.",
        ));
    }
    let _guard = APPCONTAINER_RUNTIME_GRANT_LOCK.lock().map_err(|_| {
        provider_error(
            "SANDBOX_RUNTIME_GRANT_FAILED",
            "Persistent runtime grant lock is poisoned.",
        )
    })?;
    fs::create_dir_all(marker_root).map_err(|error| {
        provider_error(
            "SANDBOX_RUNTIME_GRANT_FAILED",
            format!(
                "Failed to create persistent runtime grant state {}: {error}",
                marker_root.display()
            ),
        )
    })?;
    let marker = marker_root.join(format!("{key}.ready"));
    let expected_marker = format!("{key}\n");
    let marker_matches = fs::read_to_string(&marker)
        .map(|value| value == expected_marker)
        .unwrap_or(false);
    let acl_matches = runtime_acl_has_required_grant(path, sid.sid(), access, inheritance)?;
    if marker_matches && acl_matches {
        return Ok(());
    }
    if marker.exists() {
        fs::remove_file(&marker).map_err(|error| {
            provider_error(
                "SANDBOX_RUNTIME_GRANT_FAILED",
                format!(
                    "Failed to invalidate persistent runtime grant marker {}: {error}",
                    marker.display()
                ),
            )
        })?;
    }
    if acl_matches {
        return publish_persistent_acl_marker(
            marker_root,
            &marker,
            key,
            &expected_marker,
            "SANDBOX_RUNTIME_GRANT_FAILED",
            "runtime grant",
            || runtime_acl_has_required_grant(path, sid.sid(), access, inheritance),
        );
    }

    if acl_helper_executable().is_some() {
        run_persistent_runtime_acl_helper_process(path, sid, access, inheritance)?;
    } else {
        apply_acl_set_via_handle_direct(path, sid, access, inheritance)?;
    }
    if !runtime_acl_has_required_grant(path, sid.sid(), access, inheritance)? {
        return Err(provider_error(
            "SANDBOX_RUNTIME_GRANT_FAILED",
            format!(
                "Persistent runtime ACL verification failed for {}.",
                path.display()
            ),
        ));
    }

    publish_persistent_acl_marker(
        marker_root,
        &marker,
        key,
        &expected_marker,
        "SANDBOX_RUNTIME_GRANT_FAILED",
        "runtime grant",
        || runtime_acl_has_required_grant(path, sid.sid(), access, inheritance),
    )
}
struct PythonVenvConfig {
    home: PathBuf,
    uv_managed: bool,
}

fn python_venv_config(venv_root: &Path) -> WorkspaceResult<Option<PythonVenvConfig>> {
    let config = venv_root.join("pyvenv.cfg");
    if !config.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&config).map_err(|error| {
        provider_error(
            "SANDBOX_RUNTIME_INVALID",
            format!("Failed to read {}: {error}", config.display()),
        )
    })?;
    let mut home = None;
    let mut uv_managed = false;
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("home") {
            home = Some(value.trim());
        } else if key.trim().eq_ignore_ascii_case("uv") && !value.trim().is_empty() {
            uv_managed = true;
        }
    }
    let Some(home) = home.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(home);
    if !path.is_dir() {
        return Err(provider_error(
            "SANDBOX_RUNTIME_NOT_FOUND",
            format!(
                "Python virtual environment base runtime does not exist: {}",
                path.display()
            ),
        ));
    }
    Ok(Some(PythonVenvConfig {
        home: path,
        uv_managed,
    }))
}

fn is_unc_authorization_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _))
    )
}

fn canonical_external_path(grant: &SandboxPathGrant) -> WorkspaceResult<PathBuf> {
    let raw = grant.path.trim();
    if raw.is_empty() {
        return Err(provider_error(
            "SANDBOX_EXTERNAL_PATH_INVALID",
            "External sandbox path cannot be empty.",
        ));
    }
    let requested = PathBuf::from(raw);
    if !requested.is_absolute() {
        return Err(provider_error(
            "SANDBOX_EXTERNAL_PATH_INVALID",
            format!(
                "External sandbox path must be absolute: {}",
                requested.display()
            ),
        ));
    }
    if is_unc_authorization_path(&requested) {
        return Err(provider_error(
            "SANDBOX_EXTERNAL_PATH_UNSUPPORTED",
            format!(
                "UNC external sandbox paths are not supported by the AppContainer ACL provider: {}",
                requested.display()
            ),
        ));
    }
    let canonical = requested.canonicalize().map_err(|error| {
        provider_error(
            "SANDBOX_EXTERNAL_PATH_INVALID",
            format!(
                "External sandbox path does not exist or cannot be resolved: {}: {error}",
                requested.display()
            ),
        )
    })?;
    if is_unc_authorization_path(&canonical) {
        return Err(provider_error(
            "SANDBOX_EXTERNAL_PATH_UNSUPPORTED",
            format!(
                "External sandbox path resolves to an unsupported UNC target: {}",
                canonical.display()
            ),
        ));
    }
    Ok(canonical)
}

fn protected_repository_metadata_path(workspace_root: &Path, candidate: &Path) -> bool {
    [".git", ".github"].iter().any(|relative| {
        let protected = workspace_root.join(relative);
        if !protected.exists() {
            return false;
        }
        let protected = protected.canonicalize().unwrap_or(protected);
        candidate == protected || candidate.starts_with(&protected)
    })
}

fn acl_win32_path(path: &Path) -> PathBuf {
    const VERBATIM_PREFIX: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC_PREFIX: [u16; 8] = [
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];
    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.starts_with(&VERBATIM_UNC_PREFIX) {
        let mut normalized = vec![b'\\' as u16, b'\\' as u16];
        normalized.extend_from_slice(&wide[VERBATIM_UNC_PREFIX.len()..]);
        return PathBuf::from(OsString::from_wide(&normalized));
    }
    if wide.starts_with(&VERBATIM_PREFIX) {
        return PathBuf::from(OsString::from_wide(&wide[VERBATIM_PREFIX.len()..]));
    }
    path.to_path_buf()
}

fn prepare_external_path_grants(
    specs: &[SandboxPathGrant],
    package_sid: &Arc<SharedSid>,
    workspace_root: &Path,
) -> WorkspaceResult<Vec<TemporaryAclGrant>> {
    let mut canonical = BTreeMap::<PathBuf, SandboxPathAccess>::new();
    for spec in specs {
        let path = canonical_external_path(spec)?;
        if protected_repository_metadata_path(workspace_root, &path) {
            return Err(provider_error(
                "SANDBOX_EXTERNAL_PATH_PROTECTED",
                format!(
                    "External sandbox path cannot target protected repository metadata: {}",
                    path.display()
                ),
            ));
        }
        canonical
            .entry(path)
            .and_modify(|access| {
                if spec.access == SandboxPathAccess::Modify {
                    *access = SandboxPathAccess::Modify;
                }
            })
            .or_insert(spec.access);
    }

    let mut grants = Vec::new();
    for (path, access) in canonical {
        let access = match access {
            SandboxPathAccess::ReadOnly => AclGrantAccess::Read,
            SandboxPathAccess::Modify => AclGrantAccess::Modify,
        };
        let inheritance = if path.is_dir() {
            AclGrantInheritance::Children
        } else {
            AclGrantInheritance::None
        };
        grants.push(TemporaryAclGrant::apply(
            path,
            package_sid,
            access,
            inheritance,
        )?);
    }
    Ok(grants)
}

fn prepare_protected_asset_restrictions(
    workspace_root: &Path,
    marker_root: &Path,
    sid: &Arc<SharedSid>,
) -> WorkspaceResult<()> {
    for relative in [".git", ".github"] {
        let path = workspace_root.join(relative);
        if !path.exists() {
            continue;
        }
        ensure_persistent_protected_metadata_grant(marker_root, &path, sid)?;
    }
    Ok(())
}

struct AppContainerLease {
    #[allow(dead_code)]
    profile: Arc<AppContainerProfile>,
    #[allow(dead_code)]
    capability: DerivedCapability,
    workspace_capability: DerivedCapability,
    metadata_capability: DerivedCapability,
    runtime_capability_sid: Arc<SharedSid>,
    internet: Option<DerivedCapability>,
    _state_grant: TemporaryAclGrant,
    _external_grants: Vec<TemporaryAclGrant>,
    runtime_grants: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppContainerNetwork {
    None,
    Internet,
}

fn selected_network(options: &BTreeMap<String, String>) -> WorkspaceResult<AppContainerNetwork> {
    let raw = options
        .get(APPCONTAINER_NETWORK_OPTION_ID)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(APPCONTAINER_DEFAULT_NETWORK);
    match raw.to_ascii_lowercase().as_str() {
        "none" | "deny" | "false" | "off" => Ok(AppContainerNetwork::None),
        "internet" | "allow" | "true" | "on" => Ok(AppContainerNetwork::Internet),
        _ => Err(provider_error(
            "SANDBOX_APPCONTAINER_NETWORK_INVALID",
            format!("AppContainer network option must be 'none' or 'internet', got '{raw}'."),
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AclGrantAccess {
    Read,
    ReadExecute,
    Modify,
}

impl AclGrantAccess {
    fn permissions(self) -> u32 {
        match self {
            Self::Read => FILE_GENERIC_READ.0,
            Self::ReadExecute => (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0,
            Self::Modify => {
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE).0
            }
        }
    }
}
impl AclGrantAccess {
    fn helper_arg(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ReadExecute => "read_execute",
            Self::Modify => "modify",
        }
    }

    fn from_helper_arg(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "read_execute" => Some(Self::ReadExecute),
            "modify" => Some(Self::Modify),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AclGrantInheritance {
    None,
    Children,
}

impl AclGrantInheritance {
    fn flags(self) -> windows::Win32::Security::ACE_FLAGS {
        match self {
            Self::None => NO_INHERITANCE,
            Self::Children => OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
        }
    }
}
impl AclGrantInheritance {
    fn helper_arg(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Children => "children",
        }
    }

    fn from_helper_arg(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "children" => Some(Self::Children),
            _ => None,
        }
    }
}

struct RuntimeGrantSpec {
    path: PathBuf,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
}

impl RuntimeGrantSpec {
    fn new(path: PathBuf, access: AclGrantAccess, inheritance: AclGrantInheritance) -> Self {
        Self {
            path,
            access,
            inheritance,
        }
    }
}

fn dedupe_grants(specs: Vec<RuntimeGrantSpec>) -> Vec<RuntimeGrantSpec> {
    let mut merged = BTreeMap::<PathBuf, (AclGrantAccess, AclGrantInheritance)>::new();
    for spec in specs {
        merged
            .entry(spec.path)
            .and_modify(|(access, inheritance)| {
                *access = (*access).max(spec.access);
                *inheritance = (*inheritance).max(spec.inheritance);
            })
            .or_insert((spec.access, spec.inheritance));
    }
    merged
        .into_iter()
        .map(|(path, (access, inheritance))| RuntimeGrantSpec::new(path, access, inheritance))
        .collect()
}

struct SharedSid {
    sid: PSID,
}

// ConvertStringSidToSidW allocates immutable SID storage with LocalAlloc. Sharing
// that storage across grants is safe until the final Arc is dropped.
unsafe impl Send for SharedSid {}
unsafe impl Sync for SharedSid {}

impl SharedSid {
    fn copy_from(sid: PSID) -> WorkspaceResult<Self> {
        Self::from_text(&sid_string(sid)?)
    }

    fn from_text(value: &str) -> WorkspaceResult<Self> {
        let text = HSTRING::from(value);
        let mut copied = PSID::default();
        unsafe { ConvertStringSidToSidW(&text, &mut copied) }.map_err(|error| {
            provider_error(
                "SANDBOX_SID_CACHE_FAILED",
                format!("Failed to cache sandbox SID for ACL cleanup: {error}"),
            )
        })?;
        Ok(Self { sid: copied })
    }

    fn sid(&self) -> PSID {
        self.sid
    }
}

impl Drop for SharedSid {
    fn drop(&mut self) {
        if !self.sid.0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.sid.0)));
            }
        }
    }
}

fn apply_acl_grant_direct(
    path: &Path,
    sid: &SharedSid,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<bool> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access.permissions(),
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: inheritance.flags(),
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result.map(|_| was_protected)
}

fn apply_acl_grant_via_handle_direct(
    path: &Path,
    sid: &SharedSid,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<bool> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access.permissions(),
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: inheritance.flags(),
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl_via_handle(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result.map(|_| was_protected)
}

fn apply_acl_set_via_handle_direct(
    path: &Path,
    sid: &SharedSid,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<bool> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access.permissions(),
        grfAccessMode: SET_ACCESS,
        grfInheritance: inheritance.flags(),
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl_via_handle(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result.map(|_| was_protected)
}
fn apply_acl_grant_with_set_file_security_direct(
    path: &Path,
    sid: &SharedSid,
    access: AclGrantAccess,
    inheritance: AclGrantInheritance,
) -> WorkspaceResult<bool> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access.permissions(),
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: inheritance.flags(),
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl_with_set_file_security(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result.map(|_| was_protected)
}

fn revoke_acl_grant_direct(path: &Path, sid: &SharedSid) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: 0,
        grfAccessMode: REVOKE_ACCESS,
        grfInheritance: NO_INHERITANCE,
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result
}

fn revoke_acl_grant_via_handle_direct(path: &Path, sid: &SharedSid) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: 0,
        grfAccessMode: REVOKE_ACCESS,
        grfInheritance: NO_INHERITANCE,
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl_via_handle(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result
}

fn revoke_acl_grant_with_set_file_security_direct(
    path: &Path,
    sid: &SharedSid,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: 0,
        grfAccessMode: REVOKE_ACCESS,
        grfInheritance: NO_INHERITANCE,
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid.sid()));
    }
    let result = merge_and_install_acl_with_set_file_security(&path, dacl, entry, was_protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result
}

struct TemporaryAclGrant {
    path: PathBuf,
    sid: Arc<SharedSid>,
    sid_text: String,
    direct_was_protected: Option<bool>,
    helper_backed: bool,
}

impl TemporaryAclGrant {
    fn apply(
        path: PathBuf,
        sid: &Arc<SharedSid>,
        access: AclGrantAccess,
        inheritance: AclGrantInheritance,
    ) -> WorkspaceResult<Self> {
        Self::apply_via_handle(path, sid, access, inheritance)
    }

    fn apply_via_handle(
        path: PathBuf,
        sid: &Arc<SharedSid>,
        access: AclGrantAccess,
        inheritance: AclGrantInheritance,
    ) -> WorkspaceResult<Self> {
        let path = acl_win32_path(&path);
        if !path.exists() {
            return Err(provider_error(
                "SANDBOX_ACL_GRANT_FAILED",
                format!("ACL target does not exist: {}", path.display()),
            ));
        }
        let sid_text = sid_string(sid.sid())?;
        if acl_helper_executable().is_some() {
            run_acl_helper_process(
                "grant_via_handle",
                &path,
                &sid_text,
                Some(access),
                Some(inheritance),
                APPCONTAINER_ACL_COMMAND_TIMEOUT,
            )?;
            return Ok(Self {
                path,
                sid: Arc::clone(sid),
                sid_text,
                direct_was_protected: None,
                helper_backed: true,
            });
        }
        let was_protected = apply_acl_grant_via_handle_direct(&path, sid, access, inheritance)?;
        Ok(Self {
            path,
            sid: Arc::clone(sid),
            sid_text,
            direct_was_protected: Some(was_protected),
            helper_backed: false,
        })
    }
}

impl Drop for TemporaryAclGrant {
    fn drop(&mut self) {
        if self.helper_backed {
            let _ = run_acl_helper_process(
                "revoke_via_handle",
                &self.path,
                &self.sid_text,
                None,
                None,
                APPCONTAINER_ACL_CLEANUP_TIMEOUT,
            );
            return;
        }
        let _ = set_trustee_access_via_handle(
            &self.path,
            self.sid.sid(),
            REVOKE_ACCESS,
            0,
            NO_INHERITANCE,
            self.direct_was_protected.unwrap_or(false),
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
struct SavedDacl {
    storage: Vec<usize>,
    is_null: bool,
}

impl SavedDacl {
    fn capture(dacl: *mut ACL) -> WorkspaceResult<Self> {
        if dacl.is_null() {
            return Ok(Self {
                storage: Vec::new(),
                is_null: true,
            });
        }
        let byte_len = unsafe { (*dacl).AclSize as usize };
        if byte_len < std::mem::size_of::<ACL>() {
            return Err(provider_error(
                "SANDBOX_ACL_QUERY_FAILED",
                "Windows returned an invalid DACL size.",
            ));
        }
        let word_size = std::mem::size_of::<usize>();
        let mut storage = vec![0usize; (byte_len + word_size - 1) / word_size];
        unsafe {
            ptr::copy_nonoverlapping(
                dacl.cast::<u8>(),
                storage.as_mut_ptr().cast::<u8>(),
                byte_len,
            );
        }
        Ok(Self {
            storage,
            is_null: false,
        })
    }

    fn as_acl_ptr(&self) -> Option<*const ACL> {
        (!self.is_null).then(|| self.storage.as_ptr().cast::<ACL>())
    }

    #[cfg(test)]
    fn helper_arg(&self) -> String {
        if self.is_null {
            return "null".into();
        }
        let byte_len = self.storage.len() * std::mem::size_of::<usize>();
        let bytes =
            unsafe { std::slice::from_raw_parts(self.storage.as_ptr().cast::<u8>(), byte_len) };
        format!("acl:{}", STANDARD_NO_PAD.encode(bytes))
    }

    fn from_helper_arg(value: &str) -> WorkspaceResult<Self> {
        if value == "null" {
            return Ok(Self {
                storage: Vec::new(),
                is_null: true,
            });
        }
        let encoded = value.strip_prefix("acl:").ok_or_else(|| {
            provider_error(
                "SANDBOX_ACL_HELPER_INVALID",
                "ACL helper DACL snapshot prefix is invalid.",
            )
        })?;
        let bytes = STANDARD_NO_PAD.decode(encoded).map_err(|error| {
            provider_error(
                "SANDBOX_ACL_HELPER_INVALID",
                format!("ACL helper DACL snapshot is invalid: {error}"),
            )
        })?;
        if bytes.len() < std::mem::size_of::<ACL>() {
            return Err(provider_error(
                "SANDBOX_ACL_HELPER_INVALID",
                "ACL helper DACL snapshot is too small.",
            ));
        }
        let word_size = std::mem::size_of::<usize>();
        let mut storage = vec![0usize; (bytes.len() + word_size - 1) / word_size];
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                storage.as_mut_ptr().cast::<u8>(),
                bytes.len(),
            );
        }
        let acl = storage.as_ptr().cast::<ACL>();
        let acl_size = unsafe { (*acl).AclSize as usize };
        if acl_size < std::mem::size_of::<ACL>() || acl_size > bytes.len() {
            return Err(provider_error(
                "SANDBOX_ACL_HELPER_INVALID",
                "ACL helper DACL snapshot has an invalid ACL size.",
            ));
        }
        Ok(Self {
            storage,
            is_null: false,
        })
    }
}

fn snapshot_named_dacl(path: &Path) -> WorkspaceResult<(SavedDacl, bool)> {
    let (descriptor, dacl, protected) = query_named_dacl(path)?;
    let saved = SavedDacl::capture(dacl);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    saved.map(|dacl| (dacl, protected))
}

fn write_saved_dacl(
    path: &Path,
    dacl: &SavedDacl,
    protected: bool,
    error_code: &'static str,
    action: &str,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let security_info = DACL_SECURITY_INFORMATION
        | if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
    let name = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
    let status = unsafe {
        SetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            security_info,
            None,
            None,
            dacl.as_acl_ptr(),
            None,
        )
    };
    if status.0 != 0 {
        return Err(provider_error(
            error_code,
            format!("Failed to {action} for {}: {status:?}", path.display()),
        ));
    }
    Ok(())
}

fn restore_named_dacl(path: &Path, dacl: &SavedDacl, protected: bool) -> WorkspaceResult<()> {
    write_saved_dacl(
        path,
        dacl,
        protected,
        "SANDBOX_ACL_RESTORE_FAILED",
        "restore DACL",
    )
}

fn apply_protected_acl_direct(path: &Path, sids: &[SharedSid]) -> WorkspaceResult<()> {
    let (original_dacl, was_protected) = snapshot_named_dacl(path)?;
    if !was_protected {
        write_saved_dacl_via_handle(
            path,
            &original_dacl,
            true,
            "SANDBOX_ACL_RESTRICT_FAILED",
            "protect DACL",
        )?;
    }
    for sid in sids {
        set_trustee_access_via_handle(
            path,
            sid.sid(),
            SET_ACCESS,
            (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0,
            if path.is_dir() {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                NO_INHERITANCE
            },
            true,
        )?;
    }
    Ok(())
}

fn query_named_dacl(path: &Path) -> WorkspaceResult<(PSECURITY_DESCRIPTOR, *mut ACL, bool)> {
    let path = acl_win32_path(path);
    let name = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
    let mut dacl = ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let status = unsafe {
        GetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        )
    };
    if status.0 != 0 {
        return Err(provider_error(
            "SANDBOX_ACL_QUERY_FAILED",
            format!("Failed to read DACL for {}: {status:?}", path.display()),
        ));
    }
    let mut control = Default::default();
    let mut revision = 0u32;
    if let Err(error) =
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
    {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        return Err(provider_error(
            "SANDBOX_ACL_QUERY_FAILED",
            format!(
                "Failed to inspect DACL control for {}: {error}",
                path.display()
            ),
        ));
    }
    Ok((descriptor, dacl, (control & SE_DACL_PROTECTED.0) != 0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllowedAceInfo {
    mask: u32,
    flags: u8,
}

fn allowed_aces_for_sid(path: &Path, sid: PSID) -> WorkspaceResult<Vec<AllowedAceInfo>> {
    const ACCESS_ALLOWED_ACE_TYPE_VALUE: u8 = 0;
    let path = acl_win32_path(path);
    let (descriptor, dacl, _) = query_named_dacl(&path)?;
    if dacl.is_null() {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        return Ok(Vec::new());
    }

    let result = (|| -> WorkspaceResult<Vec<AllowedAceInfo>> {
        let ace_count = unsafe { (*dacl).AceCount as u32 };
        let mut result = Vec::new();
        for index in 0..ace_count {
            let mut ace = ptr::null_mut();
            unsafe { GetAce(dacl, index, &mut ace) }.map_err(|error| {
                provider_error(
                    "SANDBOX_ACL_QUERY_FAILED",
                    format!("Failed to read ACE {index} for {}: {error}", path.display()),
                )
            })?;
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE_VALUE {
                continue;
            }
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let ace_sid = PSID(
                (&allowed.SidStart as *const u32)
                    .cast_mut()
                    .cast::<std::ffi::c_void>(),
            );
            if unsafe { EqualSid(ace_sid, sid) }.is_ok() {
                result.push(AllowedAceInfo {
                    mask: allowed.Mask,
                    flags: header.AceFlags,
                });
            }
        }
        Ok(result)
    })();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result
}

#[cfg(test)]
fn allowed_ace_for_sid(path: &Path, sid: PSID) -> WorkspaceResult<Option<AllowedAceInfo>> {
    Ok(allowed_aces_for_sid(path, sid)?.into_iter().next())
}
#[derive(Default)]
struct SidFilterTreeStats {
    visited: u64,
    changed: u64,
    skipped_reparse: u64,
}

fn filter_allowed_sid_tree_with_set_file_security_direct(
    root: &Path,
    sid: &SharedSid,
) -> WorkspaceResult<SidFilterTreeStats> {
    let root = acl_win32_path(root);
    let mut stack = vec![root];
    let mut stats = SidFilterTreeStats::default();
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            provider_error(
                "SANDBOX_ACL_QUERY_FAILED",
                format!(
                    "Failed to inspect cleanup target {}: {error}",
                    path.display()
                ),
            )
        })?;
        let is_reparse = (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0;
        stats.visited += 1;
        if filter_allowed_sid_with_set_file_security_direct(&path, sid)? {
            stats.changed += 1;
        }
        if is_reparse {
            stats.skipped_reparse += 1;
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&path).map_err(|error| {
            provider_error(
                "SANDBOX_ACL_QUERY_FAILED",
                format!(
                    "Failed to enumerate cleanup target {}: {error}",
                    path.display()
                ),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                provider_error(
                    "SANDBOX_ACL_QUERY_FAILED",
                    format!("Failed to enumerate child of {}: {error}", path.display()),
                )
            })?;
            stack.push(entry.path());
        }
        if stats.visited % 1000 == 0 {
            eprintln!(
                "AppContainer ACL cleanup progress: visited={} changed={} skipped_reparse={}",
                stats.visited, stats.changed, stats.skipped_reparse
            );
        }
    }
    Ok(stats)
}

fn filter_allowed_sid_with_set_file_security_direct(
    path: &Path,
    sid: &SharedSid,
) -> WorkspaceResult<bool> {
    const ACCESS_ALLOWED_ACE_TYPE_VALUE: u8 = 0;
    let path = acl_win32_path(path);
    let (descriptor, dacl, was_protected) = query_named_dacl(&path)?;
    if dacl.is_null() {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        return Ok(false);
    }

    let acl_size = unsafe { (*dacl).AclSize as usize };
    let ace_count = unsafe { (*dacl).AceCount as u32 };
    let revision = ACE_REVISION(unsafe { (*dacl).AclRevision as u32 });
    let word_size = std::mem::size_of::<usize>();
    let mut storage = vec![0usize; (acl_size + word_size - 1) / word_size];
    let filtered = storage.as_mut_ptr().cast::<ACL>();
    unsafe { InitializeAcl(filtered, acl_size as u32, revision) }.map_err(|error| {
        provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Failed to initialize filtered DACL for {}: {error}",
                path.display()
            ),
        )
    })?;

    let mut removed = false;
    for index in 0..ace_count {
        let mut ace = ptr::null_mut();
        unsafe { GetAce(dacl, index, &mut ace) }.map_err(|error| {
            provider_error(
                "SANDBOX_ACL_QUERY_FAILED",
                format!("Failed to read ACE {index} for {}: {error}", path.display()),
            )
        })?;
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        let matches_sid = if header.AceType == ACCESS_ALLOWED_ACE_TYPE_VALUE {
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let ace_sid = PSID(
                (&allowed.SidStart as *const u32)
                    .cast_mut()
                    .cast::<std::ffi::c_void>(),
            );
            unsafe { EqualSid(ace_sid, sid.sid()) }.is_ok()
        } else {
            false
        };
        if matches_sid {
            removed = true;
            continue;
        }
        unsafe {
            AddAce(
                filtered,
                revision,
                u32::MAX,
                ace.cast_const(),
                header.AceSize as u32,
            )
        }
        .map_err(|error| {
            provider_error(
                "SANDBOX_ACL_RESTRICT_FAILED",
                format!("Failed to copy ACE {index} for {}: {error}", path.display()),
            )
        })?;
    }

    let result = if removed {
        install_dacl_with_set_file_security(
            &path,
            Some(filtered.cast_const()),
            was_protected,
            "SANDBOX_ACL_RESTRICT_FAILED",
            "filter probe SID from DACL",
        )
        .map(|_| true)
    } else {
        Ok(false)
    };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result
}

fn install_dacl_with_set_file_security(
    path: &Path,
    dacl: Option<*const ACL>,
    protected: bool,
    error_code: &'static str,
    action: &str,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_ptr = PSECURITY_DESCRIPTOR(
        (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast::<std::ffi::c_void>(),
    );
    unsafe { InitializeSecurityDescriptor(descriptor_ptr, 1) }.map_err(|error| {
        provider_error(
            error_code,
            format!(
                "Failed to initialize security descriptor for {}: {error}",
                path.display()
            ),
        )
    })?;
    unsafe { SetSecurityDescriptorDacl(descriptor_ptr, true, dacl, false) }.map_err(|error| {
        provider_error(
            error_code,
            format!("Failed to attach DACL for {}: {error}", path.display()),
        )
    })?;
    let security_info = DACL_SECURITY_INFORMATION
        | if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
    let name = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
    let success = unsafe { SetFileSecurityW(&name, security_info, descriptor_ptr) };
    if !success.as_bool() {
        let status = unsafe { GetLastError() };
        return Err(provider_error(
            error_code,
            format!(
                "Failed to {action} with SetFileSecurityW for {}: {status:?}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn merge_and_install_acl_with_set_file_security(
    path: &Path,
    dacl: *mut ACL,
    entry: EXPLICIT_ACCESS_W,
    protected: bool,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let mut new_acl: *mut ACL = ptr::null_mut();
    let merge_status = unsafe { SetEntriesInAclW(Some(&[entry]), Some(dacl), &mut new_acl) };
    if merge_status.0 != 0 {
        return Err(provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Failed to build SetFileSecurityW DACL for {}: {merge_status:?}",
                path.display()
            ),
        ));
    }
    let result = install_dacl_with_set_file_security(
        &path,
        Some(new_acl.cast_const()),
        protected,
        "SANDBOX_ACL_RESTRICT_FAILED",
        "install DACL",
    );
    unsafe {
        let _ = LocalFree(Some(HLOCAL(new_acl.cast())));
    }
    result
}

fn install_dacl_via_handle(
    path: &Path,
    dacl: Option<*const ACL>,
    protected: bool,
    error_code: &'static str,
    action: &str,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let name = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
    let flags = if path.is_dir() {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    let handle = unsafe {
        CreateFileW(
            &name,
            READ_CONTROL.0 | WRITE_DAC.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    }
    .map_err(|error| {
        provider_error(
            error_code,
            format!(
                "Failed to open {} for {action} without ACL propagation: {error}",
                path.display()
            ),
        )
    })?;
    let security_info = DACL_SECURITY_INFORMATION
        | if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
    let status = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            security_info,
            None,
            None,
            dacl,
            None,
        )
    };
    let close_result = unsafe { CloseHandle(handle) };
    if status.0 != 0 {
        return Err(provider_error(
            error_code,
            format!(
                "Failed to {action} without ACL propagation for {}: {status:?}",
                path.display()
            ),
        ));
    }
    close_result.map_err(|error| {
        provider_error(
            error_code,
            format!(
                "Failed to close ACL handle after {action} for {}: {error}",
                path.display()
            ),
        )
    })
}

fn write_saved_dacl_via_handle(
    path: &Path,
    dacl: &SavedDacl,
    protected: bool,
    error_code: &'static str,
    action: &str,
) -> WorkspaceResult<()> {
    install_dacl_via_handle(path, dacl.as_acl_ptr(), protected, error_code, action)
}

fn merge_and_install_acl_via_handle(
    path: &Path,
    dacl: *mut ACL,
    entry: EXPLICIT_ACCESS_W,
    protected: bool,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let mut new_acl: *mut ACL = ptr::null_mut();
    let merge_status = unsafe { SetEntriesInAclW(Some(&[entry]), Some(dacl), &mut new_acl) };
    if merge_status.0 != 0 {
        return Err(provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Failed to build handle-based DACL for {}: {merge_status:?}",
                path.display()
            ),
        ));
    }
    let result = install_dacl_via_handle(
        &path,
        Some(new_acl.cast_const()),
        protected,
        "SANDBOX_ACL_RESTRICT_FAILED",
        "install DACL",
    );
    unsafe {
        let _ = LocalFree(Some(HLOCAL(new_acl.cast())));
    }
    result
}

fn merge_and_install_acl(
    path: &Path,
    dacl: *mut ACL,
    entry: EXPLICIT_ACCESS_W,
    protected: bool,
) -> WorkspaceResult<()> {
    let path = acl_win32_path(path);
    let mut new_acl: *mut ACL = ptr::null_mut();
    let merge_status = unsafe { SetEntriesInAclW(Some(&[entry]), Some(dacl), &mut new_acl) };
    if merge_status.0 != 0 {
        return Err(provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Failed to build restricted DACL for {}: {merge_status:?}",
                path.display()
            ),
        ));
    }

    let security_info = DACL_SECURITY_INFORMATION
        | if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
    let name = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
    let set_status = unsafe {
        SetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            security_info,
            None,
            None,
            Some(new_acl.cast_const()),
            None,
        )
    };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(new_acl.cast())));
    }
    if set_status.0 != 0 {
        return Err(provider_error(
            "SANDBOX_ACL_RESTRICT_FAILED",
            format!(
                "Failed to install restricted DACL for {}: {set_status:?}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn set_trustee_access_via_handle(
    path: &Path,
    sid: PSID,
    mode: windows::Win32::Security::Authorization::ACCESS_MODE,
    permissions: u32,
    inheritance: windows::Win32::Security::ACE_FLAGS,
    protected: bool,
) -> WorkspaceResult<()> {
    let (descriptor, dacl, _) = query_named_dacl(path)?;
    let mut entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: permissions,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        ..Default::default()
    };
    unsafe {
        BuildTrusteeWithSidW(&mut entry.Trustee, Some(sid));
    }

    let result = merge_and_install_acl_via_handle(path, dacl, entry, protected);
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    result
}

struct DerivedCapability {
    sid: PSID,
    group_sids: *mut PSID,
    group_count: u32,
    capability_sids: *mut PSID,
    capability_count: u32,
}

unsafe impl Send for DerivedCapability {}
unsafe impl Sync for DerivedCapability {}

impl DerivedCapability {
    fn derive(name: &str) -> WorkspaceResult<Self> {
        let name = HSTRING::from(name);
        let mut group_sids: *mut PSID = ptr::null_mut();
        let mut group_count = 0u32;
        let mut capability_sids: *mut PSID = ptr::null_mut();
        let mut capability_count = 0u32;
        unsafe {
            DeriveCapabilitySidsFromName(
                &name,
                &mut group_sids,
                &mut group_count,
                &mut capability_sids,
                &mut capability_count,
            )
        }
        .map_err(|error| provider_error("SANDBOX_CAPABILITY_DERIVE_FAILED", error.to_string()))?;
        if capability_count != 1 || capability_sids.is_null() {
            unsafe {
                free_sid_array(group_sids, group_count);
                free_sid_array(capability_sids, capability_count);
            }
            return Err(provider_error(
                "SANDBOX_CAPABILITY_DERIVE_FAILED",
                format!("Expected one private capability SID, got {capability_count}."),
            ));
        }
        let sid = unsafe { *capability_sids };
        Ok(Self {
            sid,
            group_sids,
            group_count,
            capability_sids,
            capability_count,
        })
    }

    fn sid(&self) -> PSID {
        self.sid
    }
}

impl Drop for DerivedCapability {
    fn drop(&mut self) {
        unsafe {
            free_sid_array(self.group_sids, self.group_count);
            free_sid_array(self.capability_sids, self.capability_count);
        }
    }
}

unsafe fn free_sid_array(array: *mut PSID, count: u32) {
    if array.is_null() {
        return;
    }
    for index in 0..count as usize {
        let sid = *array.add(index);
        if !sid.0.is_null() {
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
        }
    }
    let _ = LocalFree(Some(HLOCAL(array.cast())));
}

fn workspace_identity_key(workspace_root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace_root.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn managed_appcontainer_root() -> WorkspaceResult<PathBuf> {
    let app_root = platform().app_config_dir().map_err(|error| {
        provider_error(
            "SANDBOX_STATE_PREPARE_FAILED",
            format!("Failed to resolve application config directory: {error}"),
        )
    })?;
    Ok(app_root.join("sandbox").join("appcontainer"))
}

fn managed_runtime_grant_marker_root() -> WorkspaceResult<PathBuf> {
    Ok(managed_appcontainer_root()?.join(APPCONTAINER_RUNTIME_GRANT_MARKER_DIR))
}

fn managed_state_root(workspace: &Workspace) -> WorkspaceResult<PathBuf> {
    let key = workspace_identity_key(workspace.root());
    Ok(managed_appcontainer_root()?.join(key).join("state"))
}

fn state_layout(root: &Path) -> SandboxStateLayout {
    SandboxStateLayout {
        root: root.to_path_buf(),
        home: root.join("home"),
        temp: root.join("tmp"),
        cache: root.join("cache"),
        persistence: SandboxStatePersistence::Workspace,
    }
}

fn state_environment(root: &Path) -> WorkspaceResult<BTreeMap<String, String>> {
    let home = root.join("home");
    let temp = root.join("tmp");
    let cache = root.join("cache");
    let cargo_home = root.join("cargo-home");
    let cargo_target = root.join("cargo-target");
    let npm_cache = root.join("npm-cache");
    let npm_prefix = root.join("npm-prefix");
    let pycache = root.join("pycache");
    let appdata = home.join("AppData").join("Roaming");
    let local_appdata = home.join("AppData").join("Local");
    for path in [
        &home,
        &temp,
        &cache,
        &cargo_home,
        &cargo_target,
        &npm_cache,
        &npm_prefix,
        &pycache,
        &appdata,
        &local_appdata,
    ] {
        fs::create_dir_all(path).map_err(|error| {
            provider_error(
                "SANDBOX_STATE_PREPARE_FAILED",
                format!("Failed to create {}: {error}", path.display()),
            )
        })?;
    }
    let value = |path: &Path| path.to_string_lossy().into_owned();
    Ok(BTreeMap::from([
        ("TEMP".into(), value(&temp)),
        ("TMP".into(), value(&temp)),
        ("TMPDIR".into(), value(&temp)),
        ("HOME".into(), value(&home)),
        ("USERPROFILE".into(), value(&home)),
        ("APPDATA".into(), value(&appdata)),
        ("LOCALAPPDATA".into(), value(&local_appdata)),
        ("XDG_CACHE_HOME".into(), value(&cache)),
        ("CARGO_HOME".into(), value(&cargo_home)),
        ("CARGO_TARGET_DIR".into(), value(&cargo_target)),
        ("NPM_CONFIG_CACHE".into(), value(&npm_cache)),
        ("NPM_CONFIG_PREFIX".into(), value(&npm_prefix)),
        ("PYTHONPYCACHEPREFIX".into(), value(&pycache)),
    ]))
}

fn file_name_lowercase(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn trusted_windows_runtime(program: &Path) -> WorkspaceResult<Option<PathBuf>> {
    let name = file_name_lowercase(program);
    if name == "cmd.exe" {
        let cmd = windows_system_directory()?.join("cmd.exe");
        if is_bare_program(program, "cmd.exe") || same_existing_path(program, &cmd) {
            return Ok(Some(canonical_or_original(cmd)));
        }
    }

    if matches!(
        name.as_str(),
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe"
    ) {
        if let Some(selected) = crate::tools::exec::selected_powershell_program() {
            let selected_name = selected
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if (!selected_name.is_empty() && is_bare_program(program, selected_name))
                || same_existing_path(program, &selected)
            {
                return Ok(Some(canonical_or_original(selected)));
            }
        }
    }

    if matches!(name.as_str(), "git" | "git.exe") {
        if let Ok(git) = which::which("git.exe").or_else(|_| which::which("git")) {
            if is_bare_program(program, "git.exe")
                || is_bare_program(program, "git")
                || same_existing_path(program, &git)
            {
                return Ok(Some(canonical_or_original(git)));
            }
        }
    }

    Ok(None)
}

fn node_runtime_root(program: &Path) -> WorkspaceResult<PathBuf> {
    let concrete = program.is_absolute()
        && program.is_file()
        && matches!(file_name_lowercase(program).as_str(), "node" | "node.exe");
    let node = if concrete {
        program.to_path_buf()
    } else {
        which::which("node").map_err(|error| {
            provider_error("SANDBOX_RUNTIME_NOT_FOUND", format!("node: {error}"))
        })?
    };
    node.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| provider_error("SANDBOX_RUNTIME_INVALID", "node has no parent"))
}

fn windows_system_directory() -> WorkspaceResult<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(provider_error(
            "SANDBOX_RUNTIME_NOT_FOUND",
            "Failed to resolve the Windows system directory.",
        ));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn is_bare_program(program: &Path, expected_name: &str) -> bool {
    program.components().count() == 1
        && program
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(expected_name))
}

fn same_existing_path(left: &Path, right: &Path) -> bool {
    let Ok(left) = left.canonicalize() else {
        return false;
    };
    let Ok(right) = right.canonicalize() else {
        return false;
    };
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn canonical_or_original(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn resolve_reparse_target(path: &Path) -> WorkspaceResult<PathBuf> {
    match fs::read_link(path) {
        Ok(target) if target.is_absolute() => Ok(target),
        Ok(target) => Ok(path
            .parent()
            .ok_or_else(|| {
                provider_error("SANDBOX_RUNTIME_INVALID", "reparse point has no parent")
            })?
            .join(target)),
        Err(_) => Ok(path.to_path_buf()),
    }
}

fn rustup_which(tool: &str) -> Option<PathBuf> {
    let output = Command::new("rustup").args(["which", tool]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn rust_toolchain_root(executable: &Path) -> Option<PathBuf> {
    let mut current = executable.parent()?;
    while let Some(parent) = current.parent() {
        if current.file_name().and_then(|name| name.to_str()) == Some("bin") {
            return Some(parent.to_path_buf());
        }
        current = parent;
    }
    executable.parent().map(Path::to_path_buf)
}

fn provider_error(code: &'static str, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::ToolDetails {
        code,
        message: message.into(),
        category: "security",
        retryable: false,
        details: serde_json::json!({
            "sandbox_enabled": true,
            "sandbox_backend": "appcontainer",
            "sandbox_status": "prepare_failed",
            "fallback_allowed": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_state_environment_is_workspace_scoped_and_complete() {
        let root = tempfile::tempdir().expect("state root");
        let environment = state_environment(root.path()).expect("state env");
        for key in [
            "TEMP",
            "TMP",
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "CARGO_HOME",
            "CARGO_TARGET_DIR",
            "NPM_CONFIG_CACHE",
            "PYTHONPYCACHEPREFIX",
        ] {
            let value = environment
                .get(key)
                .unwrap_or_else(|| panic!("missing {key}"));
            assert!(Path::new(value).starts_with(root.path()));
        }
    }

    #[test]
    fn appcontainer_identity_is_unique_for_the_same_timestamp() {
        let timestamp = 1_700_000_000_000_000_000u128;
        let first = appcontainer_identity_suffix_at(timestamp);
        let second = appcontainer_identity_suffix_at(timestamp);
        assert_ne!(first, second);
        assert!(first.starts_with(&format!("{}.{timestamp}.", std::process::id())));
        assert!(second.starts_with(&format!("{}.{timestamp}.", std::process::id())));
    }

    #[test]
    fn runtime_capability_sid_is_stable_across_sandbox_identities() {
        let first = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)
            .expect("first runtime capability");
        let second = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)
            .expect("second runtime capability");
        assert_eq!(
            sid_string(first.sid()).expect("first runtime SID"),
            sid_string(second.sid()).expect("second runtime SID")
        );

        let first_identity = appcontainer_identity_suffix().expect("first sandbox identity");
        let second_identity = appcontainer_identity_suffix().expect("second sandbox identity");
        assert_ne!(first_identity, second_identity);
    }

    #[test]
    fn protected_metadata_capability_is_stable_and_workspace_scoped() {
        let first_root = tempfile::tempdir().expect("first workspace");
        let second_root = tempfile::tempdir().expect("second workspace");
        let first_name =
            protected_metadata_capability_name(first_root.path()).expect("first metadata name");
        let repeated_name =
            protected_metadata_capability_name(first_root.path()).expect("repeated metadata name");
        let second_name =
            protected_metadata_capability_name(second_root.path()).expect("second metadata name");

        assert_eq!(first_name, repeated_name);
        assert_ne!(first_name, second_name);

        let first = DerivedCapability::derive(&first_name).expect("first metadata capability");
        let repeated =
            DerivedCapability::derive(&repeated_name).expect("repeated metadata capability");
        let second = DerivedCapability::derive(&second_name).expect("second metadata capability");
        assert_eq!(
            sid_string(first.sid()).expect("first metadata SID"),
            sid_string(repeated.sid()).expect("repeated metadata SID")
        );
        assert_ne!(
            sid_string(first.sid()).expect("first metadata SID"),
            sid_string(second.sid()).expect("second metadata SID")
        );
    }

    #[test]
    fn managed_runtime_grant_markers_are_shared_across_workspaces() {
        let first_root = tempfile::tempdir().expect("first workspace");
        let second_root = tempfile::tempdir().expect("second workspace");
        let first = Workspace::new(first_root.path().to_path_buf()).expect("first workspace model");
        let second =
            Workspace::new(second_root.path().to_path_buf()).expect("second workspace model");
        let first_state = managed_state_root(&first).expect("first managed state");
        let second_state = managed_state_root(&second).expect("second managed state");
        let marker_root = managed_runtime_grant_marker_root().expect("managed runtime marker root");

        assert_ne!(first_state.parent(), second_state.parent());
        assert_eq!(
            first_state.parent().and_then(Path::parent),
            marker_root.parent()
        );
        assert_eq!(
            second_state.parent().and_then(Path::parent),
            marker_root.parent()
        );
    }

    #[test]
    fn runtime_grant_deduplication_merges_same_path_to_strongest_inheritance() {
        let path = PathBuf::from(r"C:\runtime\shared");
        let grants = dedupe_grants(vec![
            RuntimeGrantSpec::new(
                path.clone(),
                AclGrantAccess::ReadExecute,
                AclGrantInheritance::None,
            ),
            RuntimeGrantSpec::new(
                path.clone(),
                AclGrantAccess::ReadExecute,
                AclGrantInheritance::Children,
            ),
        ]);
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].path, path);
        assert_eq!(grants[0].access, AclGrantAccess::ReadExecute);
        assert_eq!(grants[0].inheritance, AclGrantInheritance::Children);
    }

    #[test]
    fn canonical_runtime_grants_merge_aliases_before_selecting_inheritance() {
        let root = tempfile::tempdir().expect("runtime alias root");
        let workspace = root.path().join("workspace");
        let physical = root.path().join("physical-runtime");
        let alias = root.path().join("runtime-alias");
        fs::create_dir_all(&workspace).expect("workspace fixture");
        fs::create_dir_all(&physical).expect("physical runtime fixture");
        let junction_status = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&alias)
            .arg(&physical)
            .status()
            .expect("create runtime alias junction");
        assert!(
            junction_status.success(),
            "junction setup failed: {junction_status}"
        );

        let grants = canonical_runtime_grants(
            &workspace,
            vec![
                RuntimeGrantSpec::new(
                    alias,
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::None,
                ),
                RuntimeGrantSpec::new(
                    physical.clone(),
                    AclGrantAccess::ReadExecute,
                    AclGrantInheritance::Children,
                ),
            ],
        )
        .expect("canonical runtime grants");
        assert_eq!(grants.len(), 1);
        assert_eq!(
            grants[0].path,
            physical.canonicalize().expect("canonical physical runtime")
        );
        assert_eq!(grants[0].access, AclGrantAccess::ReadExecute);
        assert_eq!(grants[0].inheritance, AclGrantInheritance::Children);
    }
    #[test]
    fn persistent_runtime_grant_is_rx_only_persists_and_repairs_a_stale_marker() {
        let runtime = tempfile::tempdir().expect("persistent runtime root");
        let nested = runtime.path().join("nested");
        fs::create_dir_all(&nested).expect("persistent runtime nested directory");
        let nested_file = nested.join("module.py");
        fs::write(&nested_file, "print('runtime')").expect("persistent runtime file");
        let markers = tempfile::tempdir().expect("persistent runtime markers");

        let capability = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)
            .expect("persistent runtime capability");
        let sid = Arc::new(
            SharedSid::copy_from(capability.sid()).expect("persistent runtime capability SID"),
        );
        let key = persistent_runtime_grant_key(
            runtime.path(),
            sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("persistent runtime grant key");
        ensure_persistent_runtime_grant(
            markers.path(),
            runtime.path(),
            &sid,
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
            &key,
        )
        .expect("install persistent runtime grant");

        let expected_mask = AclGrantAccess::ReadExecute.permissions();
        let root_aces = allowed_aces_for_sid(runtime.path(), sid.sid()).expect("runtime root ACEs");
        assert!(!root_aces.is_empty());
        assert!(root_aces.iter().all(|ace| ace.mask == expected_mask));
        assert!(runtime_acl_has_required_grant(
            runtime.path(),
            sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("runtime root grant verification"));
        let nested_aces =
            allowed_aces_for_sid(&nested_file, sid.sid()).expect("runtime nested file ACEs");
        assert!(!nested_aces.is_empty());
        assert!(nested_aces.iter().all(|ace| ace.mask == expected_mask));

        let marker = markers.path().join(format!("{key}.ready"));
        assert_eq!(
            fs::read_to_string(&marker).expect("persistent runtime marker"),
            format!("{key}\n")
        );

        fs::remove_file(&marker).expect("remove marker while retaining valid ACL");
        assert!(!marker.exists());
        ensure_persistent_runtime_grant(
            markers.path(),
            runtime.path(),
            &sid,
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
            &key,
        )
        .expect("repair missing marker from existing ACL");
        assert_eq!(
            fs::read_to_string(&marker).expect("repaired persistent runtime marker"),
            format!("{key}\n")
        );
        assert!(runtime_acl_has_required_grant(
            runtime.path(),
            sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("existing ACL remains valid after marker repair"));

        let stable_sid = sid_string(sid.sid()).expect("stable runtime SID");
        drop(sid);
        drop(capability);
        let rederived = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)
            .expect("rederived runtime capability");
        assert_eq!(
            sid_string(rederived.sid()).expect("rederived runtime SID"),
            stable_sid
        );
        let rederived_sid = Arc::new(
            SharedSid::copy_from(rederived.sid()).expect("rederived runtime capability SID"),
        );
        assert!(runtime_acl_has_required_grant(
            runtime.path(),
            rederived_sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("persistent grant survives SID owner drop"));

        revoke_acl_grant_via_handle_direct(runtime.path(), &rederived_sid)
            .expect("remove runtime grant while leaving marker stale");
        assert!(marker.exists());
        assert!(!runtime_acl_has_required_grant(
            runtime.path(),
            rederived_sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("stale marker ACL verification"));
        ensure_persistent_runtime_grant(
            markers.path(),
            runtime.path(),
            &rederived_sid,
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
            &key,
        )
        .expect("repair stale persistent runtime grant");
        assert!(runtime_acl_has_required_grant(
            runtime.path(),
            rederived_sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("repaired runtime grant verification"));
    }

    #[test]
    fn persistent_runtime_capability_rejects_modify_access() {
        let runtime = tempfile::tempdir().expect("runtime root");
        let markers = tempfile::tempdir().expect("runtime markers");
        let capability = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)
            .expect("runtime capability");
        let sid = Arc::new(SharedSid::copy_from(capability.sid()).expect("runtime capability SID"));
        let key = persistent_runtime_grant_key(
            runtime.path(),
            sid.sid(),
            AclGrantAccess::Modify,
            AclGrantInheritance::Children,
        )
        .expect("runtime modify key");
        let error = ensure_persistent_runtime_grant(
            markers.path(),
            runtime.path(),
            &sid,
            AclGrantAccess::Modify,
            AclGrantInheritance::Children,
            &key,
        )
        .expect_err("shared runtime capability must reject modify access");
        match error {
            WorkspaceError::ToolDetails { code, .. } => {
                assert_eq!(code, "SANDBOX_RUNTIME_GRANT_INVALID");
            }
            other => panic!("unexpected runtime grant error: {other}"),
        }
        assert!(allowed_aces_for_sid(runtime.path(), sid.sid())
            .expect("runtime ACE query")
            .is_empty());
    }
    #[test]
    fn bounded_command_status_terminates_slow_helpers() {
        let mut command = Command::new("ping.exe");
        command
            .args(["127.0.0.1", "-n", "6"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let started = Instant::now();
        let error = command_status_and_stderr_with_timeout(&mut command, Duration::from_millis(50))
            .expect_err("slow helper must time out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "timed helper exceeded the bounded return window"
        );
    }

    #[test]
    fn saved_dacl_helper_arg_round_trips_exact_snapshot() {
        let root = tempfile::tempdir().expect("DACL snapshot root");
        let (saved, _) = snapshot_named_dacl(root.path()).expect("DACL snapshot");
        let decoded = SavedDacl::from_helper_arg(&saved.helper_arg()).expect("DACL helper decode");
        assert_eq!(decoded, saved);

        let null = SavedDacl {
            storage: Vec::new(),
            is_null: true,
        };
        let decoded_null =
            SavedDacl::from_helper_arg(&null.helper_arg()).expect("null DACL helper decode");
        assert_eq!(decoded_null, null);
    }

    #[test]
    fn handle_based_descendant_ace_probe() {
        let root = tempfile::tempdir().expect("ACL propagation probe root");
        let nested = root.path().join("nested");
        fs::create_dir_all(&nested).expect("ACL propagation probe nested directory");
        let nested_file = nested.join("existing.txt");
        fs::write(&nested_file, "before").expect("ACL propagation probe nested file");
        let identity = appcontainer_identity_suffix().expect("ACL propagation probe identity");
        let capability = DerivedCapability::derive(&format!(
            "CodingToolsMcp.Sandbox.PropagationProbe.{identity}"
        ))
        .expect("ACL propagation probe capability");
        let sid =
            Arc::new(SharedSid::copy_from(capability.sid()).expect("ACL propagation probe SID"));
        let grant = TemporaryAclGrant::apply_via_handle(
            root.path().to_path_buf(),
            &sid,
            AclGrantAccess::Modify,
            AclGrantInheritance::Children,
        )
        .expect("apply ACL propagation probe grant");
        let root_ace = allowed_ace_for_sid(root.path(), sid.sid()).expect("root ACE query");
        let nested_ace = allowed_ace_for_sid(&nested, sid.sid()).expect("nested ACE query");
        let file_ace = allowed_ace_for_sid(&nested_file, sid.sid()).expect("file ACE query");
        eprintln!(
            "handle-based-descendants root={root_ace:?} nested={nested_ace:?} file={file_ace:?}"
        );
        assert!(root_ace.is_some());
        drop(grant);
        let nested_after =
            allowed_ace_for_sid(&nested, sid.sid()).expect("nested ACE query after revoke");
        let file_after =
            allowed_ace_for_sid(&nested_file, sid.sid()).expect("file ACE query after revoke");
        eprintln!("handle-revoke-descendants nested={nested_after:?} file={file_after:?}");
        assert!(nested_after.is_none());
        assert!(file_after.is_none());
    }

    #[test]
    fn workspace_identity_key_is_stable_and_workspace_scoped() {
        let first = Path::new(r"E:\work\alpha");
        let second = Path::new(r"E:\work\beta");
        assert_eq!(workspace_identity_key(first), workspace_identity_key(first));
        assert_ne!(
            workspace_identity_key(first),
            workspace_identity_key(second)
        );
        assert_eq!(workspace_identity_key(first).len(), 24);
    }

    #[test]
    fn acl_path_normalizes_verbatim_paths() {
        assert_eq!(
            acl_win32_path(Path::new(r"\\?\E:\work\coding-tools-mcp")),
            PathBuf::from(r"E:\work\coding-tools-mcp")
        );
        assert_eq!(AclGrantInheritance::None.flags(), NO_INHERITANCE);
    }

    #[test]
    fn acl_grant_failure_is_structured_and_disallows_fallback() {
        let root = tempfile::tempdir().expect("ACL root");
        let missing = root.path().join("missing-acl-target");
        let sid = Arc::new(SharedSid {
            sid: PSID::default(),
        });
        let error = match TemporaryAclGrant::apply(
            missing,
            &sid,
            AclGrantAccess::Modify,
            AclGrantInheritance::Children,
        ) {
            Ok(grant) => {
                drop(grant);
                panic!("missing ACL target unexpectedly succeeded")
            }
            Err(error) => error,
        };
        match error {
            WorkspaceError::ToolDetails {
                code,
                category,
                retryable,
                details,
                ..
            } => {
                assert_eq!(code, "SANDBOX_ACL_GRANT_FAILED");
                assert_eq!(category, "security");
                assert!(!retryable);
                assert_eq!(details["sandbox_backend"], "appcontainer");
                assert_eq!(details["sandbox_status"], "prepare_failed");
                assert_eq!(details["fallback_allowed"], false);
            }
            other => panic!("unexpected ACL error: {other:?}"),
        }
    }

    #[test]
    fn trusted_system_runtime_requires_exact_windows_identity() {
        let trusted_cmd = trusted_windows_runtime(Path::new("cmd.exe"))
            .expect("trusted cmd lookup")
            .expect("trusted cmd");
        assert!(same_existing_path(
            &trusted_cmd,
            &windows_system_directory()
                .expect("system directory")
                .join("cmd.exe")
        ));

        let fake_root = tempfile::tempdir().expect("fake runtime root");
        let fake_cmd = fake_root.path().join("cmd.exe");
        fs::write(&fake_cmd, b"not cmd").expect("fake cmd");
        assert!(trusted_windows_runtime(&fake_cmd)
            .expect("fake cmd lookup")
            .is_none());

        if let Some(selected) = crate::tools::exec::selected_powershell_program() {
            assert!(trusted_windows_runtime(&selected)
                .expect("selected PowerShell lookup")
                .is_some());
            let fake_pwsh = fake_root
                .path()
                .join(selected.file_name().expect("selected PowerShell file name"));
            fs::write(&fake_pwsh, b"not PowerShell").expect("fake PowerShell");
            assert!(trusted_windows_runtime(&fake_pwsh)
                .expect("fake PowerShell lookup")
                .is_none());
        }

        let git = which::which("git.exe")
            .or_else(|_| which::which("git"))
            .expect("Git runtime");
        assert!(trusted_windows_runtime(&git).expect("Git lookup").is_some());
        let fake_git = fake_root
            .path()
            .join(git.file_name().expect("Git file name"));
        fs::write(&fake_git, b"not Git").expect("fake Git");
        assert!(trusted_windows_runtime(&fake_git)
            .expect("fake Git lookup")
            .is_none());
    }

    #[test]
    fn workspace_local_program_requires_no_external_runtime_grant() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let workspace_model = Workspace::new(workspace.path().to_path_buf()).expect("workspace");
        let prepared = prepare_with_state_root(&workspace_model, state.path().join("state"))
            .expect("prepare provider");
        fs::write(workspace.path().join("tool.exe"), b"test").expect("workspace tool");
        let command = SandboxCommand::new(
            workspace.path().join("tool.exe"),
            vec!["--version".into()],
            workspace.path().to_path_buf(),
        );
        let logical = prepared
            .normalize_logical_command(command.clone())
            .expect("logical command");
        let process = ProcessLaunchSpec {
            program: logical.executable,
            args: logical.args,
            cwd: Some(logical.cwd),
            env: Vec::new(),
            remove_env: Vec::new(),
            required_env: Vec::new(),
            windows_raw_arg: None,
            using_wsl: false,
        };
        let plan = prepared.prepare_process(process).expect("process plan");
        assert_eq!(
            plan.process.program,
            command.executable.canonicalize().expect("canonical tool")
        );
        assert_eq!(plan.process.args, command.args);
        assert_eq!(plan.process.cwd.as_deref(), Some(command.cwd.as_path()));
        assert_eq!(plan.backend_id, "appcontainer");
        assert_eq!(
            plan.state.as_ref().map(|state| state.persistence),
            Some(SandboxStatePersistence::Workspace)
        );
        drop(prepared);
    }

    #[tokio::test]
    async fn provider_launch_keeps_workspace_lease_alive_after_prepared_provider_drop() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("inside");
        fs::create_dir_all(&workspace).expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let workspace_model = Workspace::new(workspace.clone()).expect("workspace model");
        let child_exe = workspace.join("provider-launch-test.exe");
        fs::copy(std::env::current_exe().expect("current exe"), &child_exe).expect("copy child");

        let prepared = prepare_with_state_root(&workspace_model, state.path().join("state"))
            .expect("prepare provider");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                child_exe,
                vec![
                    "--exact".into(),
                    "tools::sandbox::appcontainer::tests::appcontainer_child_probe".into(),
                    "--nocapture".into(),
                ],
                workspace.clone(),
            ),
            vec![
                ("CTMCP_APPCONTAINER_CHILD_TEST".into(), "1".into()),
                ("CTMCP_APPCONTAINER_CHILD_DELAY_MS".into(), "300".into()),
            ],
            Vec::new(),
        )
        .await
        .expect("orchestrated provider launch");
        assert_eq!(started.diagnostics.attempts, 1);
        let crate::tools::process_start::StartedChild {
            child,
            diagnostics: _,
            startup_guard,
        } = started;
        assert!(child.process_tree_contained());

        // Drop the provider before the delayed child write. The ProcessChild lifetime
        // guard must keep workspace/state/runtime ACLs, profile and capability alive.
        drop(prepared);
        let output = child
            .wait_with_output()
            .await
            .expect("provider child output");
        drop(startup_guard);
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("appcontainer-child-ok"));
        assert!(workspace.join("inside-created.txt").exists());
        assert!(!root.path().join("outside-blocked.txt").exists());
    }

    async fn run_workspace_script_through_provider(
        file_name: &str,
        contents: &str,
        expected_output: &str,
    ) {
        let workspace = tempfile::tempdir().expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let script = workspace.path().join(file_name);
        fs::write(&script, contents).expect("script");
        let workspace_model =
            Workspace::new(workspace.path().to_path_buf()).expect("workspace model");
        let prepared = prepare_with_state_root(&workspace_model, state.path().join("state"))
            .expect("prepare provider");

        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(script, Vec::new(), workspace.path().to_path_buf()),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("script provider launch");
        assert_eq!(started.diagnostics.attempts, 1);
        assert!(started.child.process_tree_contained());

        // The script may still be opened by cmd/PowerShell after process creation.
        // The child-held provider lease must therefore keep workspace access alive.
        drop(prepared);
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("script output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(expected_output));
    }

    #[tokio::test]
    async fn workspace_cmd_uses_trusted_zero_grant_system_runtime() {
        run_workspace_script_through_provider(
            "provider-system-runtime.cmd",
            "@echo appcontainer-provider-cmd-ok\r\n",
            "appcontainer-provider-cmd-ok",
        )
        .await;
    }

    #[tokio::test]
    async fn workspace_ps1_uses_runner_selected_zero_grant_powershell() {
        run_workspace_script_through_provider(
            "provider-system-runtime.ps1",
            "Write-Output 'appcontainer-provider-pwsh-ok'\r\n",
            "appcontainer-provider-pwsh-ok",
        )
        .await;
    }

    #[tokio::test]
    async fn protected_repository_assets_are_readable_but_not_mutable() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let protected_dir = workspace.join(".github").join("workflows");
        fs::create_dir_all(&protected_dir).expect("protected dir");
        let pointer = workspace.join(".git");
        let workflow = protected_dir.join("ci.yml");
        fs::write(&pointer, "gitdir: protected\n").expect("pointer fixture");
        fs::write(&workflow, "name: protected\n").expect("workflow fixture");
        let github_root = workspace.join(".github");
        let metadata_capability = DerivedCapability::derive(
            &protected_metadata_capability_name(&workspace).expect("metadata capability name"),
        )
        .expect("metadata capability");
        let script = workspace.join("protected-assets.cmd");
        fs::write(
            &script,
            "@echo off\r\ntype \"%~1\" >nul || exit /b 41\r\ntype \"%~2\" >nul || exit /b 42\r\necho changed>\"%~1\" 2>nul\r\necho created>\"%~3\" 2>nul\r\ndel /q \"%~2\" 2>nul\r\ndel /q \"%~1\" 2>nul\r\nexit /b 0\r\n",
        )
        .expect("generic protected target script");
        let created = workspace.join(".github").join("created.txt");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root(&model, state.path().join("state"))
            .expect("prepare protected workspace");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                script,
                vec![
                    pointer.to_string_lossy().into_owned(),
                    workflow.to_string_lossy().into_owned(),
                    created.to_string_lossy().into_owned(),
                ],
                workspace,
            ),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("protected target launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("protected output");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            fs::read_to_string(&pointer).expect("pointer remains"),
            "gitdir: protected\n"
        );
        assert_eq!(
            fs::read_to_string(&workflow).expect("workflow remains"),
            "name: protected\n"
        );
        assert!(!created.exists());
        drop(prepared);
        assert!(
            protected_metadata_acl_has_required_grant(&pointer, metadata_capability.sid())
                .expect("persistent .git metadata ACL"),
            "the stable workspace metadata capability must keep read-only .git access"
        );
        assert!(
            protected_metadata_acl_has_required_grant(&github_root, metadata_capability.sid())
                .expect("persistent .github metadata ACL"),
            "the stable workspace metadata capability must keep read-only .github access"
        );
        fs::write(&pointer, "host-restored\n").expect("host can still update .git pointer");
        fs::write(&workflow, "name: host-restored\n")
            .expect("host can still update protected workflow");
    }

    #[tokio::test]
    async fn reparse_and_verbatim_aliases_do_not_expand_workspace_authorization() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&outside).expect("outside");
        let secret = outside.join("secret.txt");
        fs::write(&secret, "outside-secret").expect("outside secret");

        let junction = workspace.join("junction");
        let junction_status = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .expect("create junction");
        assert!(
            junction_status.success(),
            "junction setup failed: {junction_status}"
        );

        let symlink = workspace.join("outside-link.txt");
        std::os::windows::fs::symlink_file(&secret, &symlink)
            .expect("create Windows file symlink for sandbox bypass regression");

        let verbatim_secret = secret.canonicalize().expect("verbatim secret path");
        assert!(
            verbatim_secret
                .as_os_str()
                .to_string_lossy()
                .starts_with(r"\\?\"),
            "canonical Windows test path should use verbatim form: {}",
            verbatim_secret.display()
        );
        let junction_marker = junction.join("junction-created.txt");
        let symlink_marker = outside.join("symlink-created.txt");
        let verbatim_marker = outside
            .join("verbatim-created.txt")
            .canonicalize()
            .unwrap_or_else(|_| {
                let canonical_outside = outside.canonicalize().expect("canonical outside");
                canonical_outside.join("verbatim-created.txt")
            });
        let script = workspace.join("alias-boundary.cmd");
        fs::write(
            &script,
            "@echo off\r\ntype \"%~1\" >nul 2>&1\r\nif not errorlevel 1 exit /b 41\r\ntype \"%~2\" >nul 2>&1\r\nif not errorlevel 1 exit /b 42\r\ntype \"%~3\" >nul 2>&1\r\nif not errorlevel 1 exit /b 43\r\necho escape>\"%~4\" 2>nul\r\nif exist \"%~4\" exit /b 44\r\necho escape>\"%~5\" 2>nul\r\nif exist \"%~5\" exit /b 45\r\necho escape>\"%~6\" 2>nul\r\nif exist \"%~6\" exit /b 46\r\nexit /b 0\r\n",
        )
        .expect("alias boundary script");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root(&model, state.path().join("state"))
            .expect("prepare alias boundary");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                script,
                vec![
                    junction.join("secret.txt").to_string_lossy().into_owned(),
                    symlink.to_string_lossy().into_owned(),
                    verbatim_secret.to_string_lossy().into_owned(),
                    junction_marker.to_string_lossy().into_owned(),
                    symlink_marker.to_string_lossy().into_owned(),
                    verbatim_marker.to_string_lossy().into_owned(),
                ],
                workspace,
            ),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("alias boundary launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("alias output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(secret).expect("outside secret remains"),
            "outside-secret"
        );
        assert!(!junction_marker.exists());
        assert!(!symlink_marker.exists());
        assert!(!verbatim_marker.exists());
    }

    #[test]
    fn filesystem_grant_aliases_canonicalize_locally_and_reject_network_prefixes() {
        let root = tempfile::tempdir().expect("root");
        let local = root.path().join("local");
        fs::create_dir_all(&local).expect("local grant target");
        let normal = SandboxPathGrant {
            path: local.to_string_lossy().into_owned(),
            access: SandboxPathAccess::ReadOnly,
        };
        let verbatim = SandboxPathGrant {
            path: local
                .canonicalize()
                .expect("canonical local")
                .to_string_lossy()
                .into_owned(),
            access: SandboxPathAccess::ReadOnly,
        };
        assert_eq!(
            canonical_external_path(&normal).expect("normal local grant"),
            canonical_external_path(&verbatim).expect("verbatim local grant")
        );

        for path in [r"\\server\share\folder", r"\\?\UNC\server\share\folder"] {
            let error = canonical_external_path(&SandboxPathGrant {
                path: path.into(),
                access: SandboxPathAccess::ReadOnly,
            })
            .expect_err("network grant must fail closed");
            let value = error.to_error_value();
            assert_eq!(
                value["code"], "SANDBOX_EXTERNAL_PATH_UNSUPPORTED",
                "{value}"
            );
        }
    }

    #[test]
    #[ignore = "requires a local Windows checkout and the built Desktop ACL helper"]
    fn dropbox_workspace_root_handle_acl_probe() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .to_path_buf();
        let (original_dacl, was_protected) =
            snapshot_named_dacl(&workspace).expect("workspace DACL snapshot");
        let identity = appcontainer_identity_suffix().expect("probe identity");
        let capability = DerivedCapability::derive(&format!(
            "CodingToolsMcp.Sandbox.NonPropagatingAclProbe.{identity}"
        ))
        .expect("probe capability");
        let sid_text = sid_string(capability.sid()).expect("probe SID");
        let grant_args = vec![
            OsString::from(&sid_text),
            OsString::from(AclGrantAccess::Modify.helper_arg()),
            OsString::from(AclGrantInheritance::Children.helper_arg()),
        ];
        let started = Instant::now();
        let grant_result = run_acl_helper_process_args(
            "grant_via_handle",
            &workspace,
            &grant_args,
            APPCONTAINER_ACL_COMMAND_TIMEOUT,
            "SANDBOX_ACL_GRANT_TIMEOUT",
            "SANDBOX_ACL_HELPER_FAILED",
        );
        let grant_elapsed = started.elapsed();

        let restore_args = vec![
            OsString::from(if was_protected {
                "protected"
            } else {
                "inheriting"
            }),
            OsString::from(original_dacl.helper_arg()),
        ];
        let restore_result = run_acl_helper_process_args(
            "restore_dacl_via_handle",
            &workspace,
            &restore_args,
            APPCONTAINER_ACL_CLEANUP_TIMEOUT,
            "SANDBOX_ACL_RESTORE_TIMEOUT",
            "SANDBOX_ACL_RESTORE_FAILED",
        );
        restore_result.expect("restore exact workspace DACL without propagation");
        grant_result.expect("grant workspace root without propagation");
        assert!(
            grant_elapsed < APPCONTAINER_ACL_COMMAND_TIMEOUT,
            "handle-based grant exceeded the bounded helper timeout"
        );
        let (restored_dacl, restored_protected) =
            snapshot_named_dacl(&workspace).expect("restored workspace DACL snapshot");
        assert_eq!(restored_dacl, original_dacl);
        assert_eq!(restored_protected, was_protected);
    }

    #[tokio::test]
    #[ignore = "requires a local Windows AppContainer checkout and Node runtime"]
    async fn fresh_dropbox_workspace_allows_first_write() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .to_path_buf();
        let marker = workspace.join(format!(
            ".appcontainer-real-workspace-marker-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&marker);
        let model = Workspace::new(workspace.clone()).expect("real workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root(&model, state.path().join("state"))
            .expect("prepare real workspace grant");
        let node = which::which("node.exe")
            .or_else(|_| which::which("node"))
            .expect("Node runtime");
        let script = format!(
            "try {{ require('fs').writeFileSync({:?}, 'real-workspace-ok') }} catch (error) {{ console.error('write-error', error.code, error.errno, error.syscall); process.exit(41) }}",
            marker.to_string_lossy()
        );
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(node, vec!["-e".into(), script], workspace.clone()),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("real workspace launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("real workspace output");
        drop(prepared);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&marker)
                .expect("real workspace marker")
                .trim(),
            "real-workspace-ok"
        );
        fs::remove_file(marker).expect("cleanup real Dropbox workspace marker");
    }

    #[tokio::test]
    #[ignore = "requires a local Windows AppContainer checkout and Node runtime"]
    async fn dropbox_workspace_handle_grant_reaches_existing_nested_file() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .to_path_buf();
        let fixture_dir = workspace.join(format!(
            ".appcontainer-real-nested-marker-{}",
            std::process::id()
        ));
        let nested_file = fixture_dir.join("existing.txt");
        let _ = fs::remove_dir_all(&fixture_dir);
        fs::create_dir_all(&fixture_dir).expect("create real Dropbox nested fixture");
        fs::write(&nested_file, "before").expect("create real Dropbox existing nested file");

        let model = Workspace::new(workspace.clone()).expect("real workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root(&model, state.path().join("state"))
            .expect("prepare real workspace grant");
        let node = which::which("node.exe")
            .or_else(|_| which::which("node"))
            .expect("Node runtime");
        let script = format!(
            "try {{ require('fs').writeFileSync({:?}, 'after') }} catch (error) {{ console.error('write-error', error.code, error.errno, error.syscall); process.exit(41) }}",
            nested_file.to_string_lossy()
        );
        let launch = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(node, vec!["-e".into(), script], workspace),
            Vec::new(),
            Vec::new(),
        )
        .await;
        let started = match launch {
            Ok(started) => started,
            Err(error) => {
                drop(prepared);
                let _ = fs::remove_dir_all(&fixture_dir);
                panic!("real nested workspace launch failed: {error:?}");
            }
        };
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("real nested workspace output");
        drop(prepared);
        let nested_contents = fs::read_to_string(&nested_file);
        let cleanup = fs::remove_dir_all(&fixture_dir);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            nested_contents.expect("read real Dropbox existing nested file"),
            "after"
        );
        cleanup.expect("cleanup real Dropbox nested fixture");
    }

    #[tokio::test]
    async fn workspace_root_inheritance_reaches_existing_nested_files() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let nested = workspace.join("nested");
        fs::create_dir_all(&nested).expect("nested workspace");
        let nested_file = nested.join("existing.txt");
        fs::write(&nested_file, "before").expect("nested fixture");
        let script = workspace.join("nested-write.cmd");
        fs::write(
            &script,
            format!(
                "@echo off\r\necho after>\"{}\" || exit /b 41\r\n",
                nested_file.display()
            ),
        )
        .expect("nested writer");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root(&model, state.path().join("state"))
            .expect("prepare inherited workspace grant");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(script, Vec::new(), workspace),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("nested write launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("nested output");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            fs::read_to_string(nested_file)
                .expect("nested result")
                .trim(),
            "after"
        );
    }

    #[test]
    fn external_grants_cannot_target_protected_repository_metadata() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let git = workspace.join(".git");
        let workflow = workspace.join(".github").join("workflows");
        fs::create_dir_all(&git).expect("git metadata");
        fs::create_dir_all(&workflow).expect("github metadata");
        let sid = Arc::new(SharedSid {
            sid: PSID::default(),
        });

        for target in [git, workflow] {
            let grants = vec![SandboxPathGrant {
                path: target.to_string_lossy().into_owned(),
                access: SandboxPathAccess::Modify,
            }];
            let error = match prepare_external_path_grants(&grants, &sid, &workspace) {
                Ok(grants) => {
                    drop(grants);
                    panic!("protected repository metadata grant unexpectedly succeeded")
                }
                Err(error) => error,
            };
            let value = error.to_error_value();
            assert_eq!(value["code"], "SANDBOX_EXTERNAL_PATH_PROTECTED", "{value}");
        }
    }

    #[tokio::test]
    async fn configured_external_paths_enforce_read_only_modify_and_ungranted_boundaries() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let read_only = root.path().join("external-read-only");
        let writable = root.path().join("external-modify");
        let outside = root.path().join("outside");
        for path in [&workspace, &read_only, &writable, &outside] {
            fs::create_dir_all(path).expect("fixture directory");
        }
        fs::write(read_only.join("read.txt"), "read-only").expect("read-only fixture");
        fs::write(writable.join("read.txt"), "writable").expect("writable fixture");
        fs::write(outside.join("secret.txt"), "outside").expect("outside fixture");

        let read_only_blocked = read_only.join("blocked.txt");
        let writable_created = writable.join("created.txt");
        let outside_created = outside.join("created.txt");
        let script = workspace.join("external-grants.cmd");
        fs::write(
            &script,
            format!(
                "@echo off\r\ntype \"{}\" >nul 2>&1 || exit /b 41\r\necho blocked>\"{}\" 2>nul\r\nif exist \"{}\" exit /b 42\r\ntype \"{}\" >nul 2>&1 || exit /b 43\r\necho allowed>\"{}\" || exit /b 44\r\ntype \"{}\" >nul 2>&1\r\nif not errorlevel 1 exit /b 45\r\necho escape>\"{}\" 2>nul\r\nif exist \"{}\" exit /b 46\r\nexit /b 0\r\n",
                read_only.join("read.txt").display(),
                read_only_blocked.display(),
                read_only_blocked.display(),
                writable.join("read.txt").display(),
                writable_created.display(),
                outside.join("secret.txt").display(),
                outside_created.display(),
                outside_created.display(),
            ),
        )
        .expect("external grant script");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let grants = vec![
            SandboxPathGrant {
                path: read_only.to_string_lossy().into_owned(),
                access: SandboxPathAccess::ReadOnly,
            },
            SandboxPathGrant {
                path: writable.to_string_lossy().into_owned(),
                access: SandboxPathAccess::Modify,
            },
        ];
        let prepared = prepare_with_state_root_and_external_paths(
            &model,
            state.path().join("state"),
            &grants,
            &BTreeMap::new(),
        )
        .expect("prepare external grants");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(script, Vec::new(), workspace),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("external grant launch");
        assert!(started.child.process_tree_contained());
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("external grant output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!read_only_blocked.exists());
        assert_eq!(
            fs::read_to_string(&writable_created)
                .expect("writable grant result")
                .trim(),
            "allowed"
        );
        assert!(!outside_created.exists());
        drop(prepared);
    }

    #[tokio::test]
    async fn stable_runtime_capability_allows_read_but_not_write() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let runtime = root.path().join("runtime");
        fs::create_dir_all(&workspace).expect("workspace fixture");
        fs::create_dir_all(&runtime).expect("runtime fixture");
        let readable = runtime.join("read.txt");
        let blocked = runtime.join("blocked.txt");
        fs::write(&readable, "runtime-readable").expect("runtime readable fixture");

        let markers = tempfile::tempdir().expect("runtime markers");
        let capability = DerivedCapability::derive(APPCONTAINER_RUNTIME_CAPABILITY_NAME)
            .expect("runtime capability");
        let sid = Arc::new(SharedSid::copy_from(capability.sid()).expect("runtime capability SID"));
        let canonical_runtime = runtime.canonicalize().expect("canonical runtime fixture");
        let key = persistent_runtime_grant_key(
            &canonical_runtime,
            sid.sid(),
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
        )
        .expect("runtime grant key");
        ensure_persistent_runtime_grant(
            markers.path(),
            &canonical_runtime,
            &sid,
            AclGrantAccess::ReadExecute,
            AclGrantInheritance::Children,
            &key,
        )
        .expect("install runtime read/execute grant");

        let script = workspace.join("runtime-capability-boundary.cmd");
        fs::write(
            &script,
            format!(
                "@echo off\r\ntype \"{}\" >nul 2>&1 || exit /b 41\r\necho blocked>\"{}\" 2>nul\r\nif exist \"{}\" exit /b 42\r\nexit /b 0\r\n",
                readable.display(),
                blocked.display(),
                blocked.display(),
            ),
        )
        .expect("runtime capability boundary script");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root(&model, state.path().join("state"))
            .expect("prepare runtime capability boundary");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(script, Vec::new(), workspace),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("runtime capability boundary launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("runtime capability boundary output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!blocked.exists());
        assert_eq!(
            fs::read_to_string(readable).expect("runtime file remains"),
            "runtime-readable"
        );
    }
    #[tokio::test]
    async fn descendant_process_inherits_explicit_grant_without_scope_expansion() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let granted = root.path().join("granted");
        let outside = root.path().join("outside");
        for path in [&workspace, &granted, &outside] {
            fs::create_dir_all(path).expect("fixture directory");
        }
        fs::write(granted.join("read.txt"), "granted").expect("granted fixture");
        fs::write(outside.join("secret.txt"), "outside").expect("outside fixture");
        let granted_created = granted.join("child-created.txt");
        let outside_created = outside.join("child-created.txt");

        let child = workspace.join("child-boundary.cmd");
        fs::write(
            &child,
            "@echo off\r\ntype \"%~1\" >nul 2>&1 || exit /b 41\r\necho child>\"%~2\" || exit /b 42\r\ntype \"%~3\" >nul 2>&1\r\nif not errorlevel 1 exit /b 43\r\necho escape>\"%~4\" 2>nul\r\nif exist \"%~4\" exit /b 44\r\nexit /b 0\r\n",
        )
        .expect("child boundary script");
        let parent = workspace.join("parent.cmd");
        fs::write(
            &parent,
            "@echo off\r\ncmd.exe /d /c call \"%~1\" \"%~2\" \"%~3\" \"%~4\" \"%~5\"\r\nexit /b %errorlevel%\r\n",
        )
        .expect("parent script");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let grants = vec![SandboxPathGrant {
            path: granted.to_string_lossy().into_owned(),
            access: SandboxPathAccess::Modify,
        }];
        let prepared = prepare_with_state_root_and_external_paths(
            &model,
            state.path().join("state"),
            &grants,
            &BTreeMap::new(),
        )
        .expect("prepare descendant grant");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                parent,
                vec![
                    child.to_string_lossy().into_owned(),
                    granted.join("read.txt").to_string_lossy().into_owned(),
                    granted_created.to_string_lossy().into_owned(),
                    outside.join("secret.txt").to_string_lossy().into_owned(),
                    outside_created.to_string_lossy().into_owned(),
                ],
                workspace,
            ),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("descendant grant launch");
        assert!(started.child.process_tree_contained());
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("descendant grant output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(granted_created)
                .expect("descendant granted write")
                .trim(),
            "child"
        );
        assert!(!outside_created.exists());
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).expect("outside secret remains"),
            "outside"
        );
    }

    #[tokio::test]
    async fn reparse_points_inside_an_external_grant_do_not_expand_authorization() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let granted = root.path().join("granted");
        let secret = root.path().join("secret");
        for path in [&workspace, &granted, &secret] {
            fs::create_dir_all(path).expect("fixture directory");
        }
        fs::write(granted.join("allowed.txt"), "granted").expect("granted fixture");
        fs::write(secret.join("hidden.txt"), "secret").expect("secret fixture");

        let junction = granted.join("escape-junction");
        let junction_status = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&secret)
            .status()
            .expect("create grant-internal junction");
        assert!(
            junction_status.success(),
            "junction setup failed: {junction_status}"
        );

        let symlink = granted.join("escape-link.txt");
        std::os::windows::fs::symlink_file(secret.join("hidden.txt"), &symlink)
            .expect("create grant-internal file symlink");

        let junction_marker = secret.join("junction-created.txt");
        let symlink_marker = secret.join("symlink-created.txt");
        let script = workspace.join("grant-reparse.cmd");
        fs::write(
            &script,
            "@echo off\r\ntype \"%~1\" >nul 2>&1 || exit /b 41\r\ntype \"%~2\" >nul 2>&1\r\nif not errorlevel 1 exit /b 42\r\ntype \"%~3\" >nul 2>&1\r\nif not errorlevel 1 exit /b 43\r\necho escape>\"%~4\" 2>nul\r\nif exist \"%~4\" exit /b 44\r\necho escape>\"%~5\" 2>nul\r\nif exist \"%~5\" exit /b 45\r\nexit /b 0\r\n",
        )
        .expect("grant reparse script");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let grants = vec![SandboxPathGrant {
            path: granted.to_string_lossy().into_owned(),
            access: SandboxPathAccess::Modify,
        }];
        let prepared = prepare_with_state_root_and_external_paths(
            &model,
            state.path().join("state"),
            &grants,
            &BTreeMap::new(),
        )
        .expect("prepare grant-internal reparse");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                script,
                vec![
                    granted.join("allowed.txt").to_string_lossy().into_owned(),
                    junction.join("hidden.txt").to_string_lossy().into_owned(),
                    symlink.to_string_lossy().into_owned(),
                    junction_marker.to_string_lossy().into_owned(),
                    symlink_marker.to_string_lossy().into_owned(),
                ],
                workspace,
            ),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("grant-internal reparse launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("grant-internal reparse output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(secret.join("hidden.txt")).expect("secret remains"),
            "secret"
        );
        assert!(!junction_marker.exists());
        assert!(!symlink_marker.exists());
    }

    #[tokio::test]
    async fn read_only_grant_reparse_cannot_write_the_target() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        let granted = root.path().join("granted-ro");
        let outside = root.path().join("outside");
        for path in [&workspace, &granted, &outside] {
            fs::create_dir_all(path).expect("fixture directory");
        }
        fs::write(granted.join("visible.txt"), "read-only").expect("read-only fixture");
        fs::write(outside.join("target.txt"), "original").expect("outside fixture");

        let junction = granted.join("to-outside");
        let junction_status = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .expect("create read-only grant junction");
        assert!(
            junction_status.success(),
            "junction setup failed: {junction_status}"
        );

        let replaced = outside.join("target.txt");
        let created = outside.join("created.txt");
        let script = workspace.join("readonly-reparse.cmd");
        fs::write(
            &script,
            "@echo off\r\ntype \"%~1\" >nul 2>&1 || exit /b 41\r\necho changed>\"%~2\" 2>nul\r\nif exist \"%~3\" exit /b 42\r\necho created>\"%~3\" 2>nul\r\nif exist \"%~3\" exit /b 43\r\nexit /b 0\r\n",
        )
        .expect("read-only reparse script");

        let model = Workspace::new(workspace.clone()).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let grants = vec![SandboxPathGrant {
            path: granted.to_string_lossy().into_owned(),
            access: SandboxPathAccess::ReadOnly,
        }];
        let prepared = prepare_with_state_root_and_external_paths(
            &model,
            state.path().join("state"),
            &grants,
            &BTreeMap::new(),
        )
        .expect("prepare read-only reparse");
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                script,
                vec![
                    granted.join("visible.txt").to_string_lossy().into_owned(),
                    replaced.to_string_lossy().into_owned(),
                    created.to_string_lossy().into_owned(),
                ],
                workspace,
            ),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("read-only reparse launch");
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("read-only reparse output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&replaced).expect("outside target remains"),
            "original"
        );
        assert!(!created.exists());
    }

    #[test]
    fn appcontainer_network_option_defaults_to_none_and_rejects_unknown_values() {
        assert_eq!(
            selected_network(&BTreeMap::new()).expect("default network"),
            AppContainerNetwork::None
        );
        assert_eq!(
            selected_network(&BTreeMap::from([(
                APPCONTAINER_NETWORK_OPTION_ID.into(),
                "none".into()
            )]))
            .expect("explicit none"),
            AppContainerNetwork::None
        );
        assert_eq!(
            selected_network(&BTreeMap::from([(
                APPCONTAINER_NETWORK_OPTION_ID.into(),
                "internet".into()
            )]))
            .expect("internet"),
            AppContainerNetwork::Internet
        );
        let error = selected_network(&BTreeMap::from([(
            APPCONTAINER_NETWORK_OPTION_ID.into(),
            "host".into(),
        )]))
        .expect_err("unknown network");
        assert_eq!(
            error.to_error_value()["code"],
            "SANDBOX_APPCONTAINER_NETWORK_INVALID"
        );
    }

    #[test]
    fn internet_network_option_derives_the_well_known_capability_during_prepare() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let model = Workspace::new(workspace).expect("workspace model");
        let state = tempfile::tempdir().expect("state");
        let prepared = prepare_with_state_root_and_external_paths(
            &model,
            state.path().join("state"),
            &[],
            &BTreeMap::from([(APPCONTAINER_NETWORK_OPTION_ID.into(), "internet".into())]),
        )
        .expect("prepare internet AppContainer");
        assert_eq!(prepared.backend_id(), "appcontainer");
    }

    #[tokio::test]
    async fn separate_workspace_providers_cannot_cross_write_each_other() {
        let root = tempfile::tempdir().expect("workspace root");
        let workspace_a = root.path().join("workspace-a");
        let workspace_b = root.path().join("workspace-b");
        fs::create_dir_all(&workspace_a).expect("workspace A");
        fs::create_dir_all(&workspace_b).expect("workspace B");

        let script_a = workspace_a.join("isolation-a.cmd");
        fs::write(
            &script_a,
            "@echo off\r\necho own-a>own-a.txt\r\necho escape>\"..\\workspace-b\\a-cross.txt\" 2>nul\r\nif exist \"..\\workspace-b\\a-cross.txt\" exit /b 41\r\nexit /b 0\r\n",
        )
        .expect("workspace A script");
        let script_b = workspace_b.join("isolation-b.cmd");
        fs::write(
            &script_b,
            "@echo off\r\necho own-b>own-b.txt\r\necho escape>\"..\\workspace-a\\b-cross.txt\" 2>nul\r\nif exist \"..\\workspace-a\\b-cross.txt\" exit /b 42\r\nexit /b 0\r\n",
        )
        .expect("workspace B script");

        let state_a = tempfile::tempdir().expect("state A");
        let state_b = tempfile::tempdir().expect("state B");
        let model_a = Workspace::new(workspace_a.clone()).expect("workspace A model");
        let model_b = Workspace::new(workspace_b.clone()).expect("workspace B model");
        let prepared_a = prepare_with_state_root(&model_a, state_a.path().join("state"))
            .expect("prepare workspace A");
        let prepared_b = prepare_with_state_root(&model_b, state_b.path().join("state"))
            .expect("prepare workspace B");

        for (prepared, script, cwd, own_marker) in [
            (
                prepared_a.as_ref(),
                script_a,
                workspace_a.clone(),
                workspace_a.join("own-a.txt"),
            ),
            (
                prepared_b.as_ref(),
                script_b,
                workspace_b.clone(),
                workspace_b.join("own-b.txt"),
            ),
        ] {
            let started = crate::tools::sandbox::start_prepared_sandbox_command(
                prepared,
                SandboxCommand::new(script, Vec::new(), cwd),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("cross-workspace provider launch");
            assert!(started.child.process_tree_contained());
            let output = started
                .child
                .wait_with_output()
                .await
                .expect("cross-workspace output");
            assert_eq!(
                output.status.code(),
                Some(0),
                "stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(own_marker.exists(), "selected workspace was not writable");
        }

        assert!(!workspace_b.join("a-cross.txt").exists());
        assert!(!workspace_a.join("b-cross.txt").exists());
        drop(prepared_a);
        drop(prepared_b);
    }

    #[tokio::test]
    async fn git_uses_exact_identity_zero_grant_runtime() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let workspace_model = Workspace::new(workspace.path().to_path_buf()).expect("workspace");
        let prepared = prepare_with_state_root(&workspace_model, state.path().join("state"))
            .expect("prepare provider");
        let git = which::which("git.exe")
            .or_else(|_| which::which("git"))
            .expect("Git runtime");

        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared.as_ref(),
            SandboxCommand::new(
                git,
                vec!["--version".into()],
                workspace.path().to_path_buf(),
            ),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("Git provider launch");
        assert_eq!(started.diagnostics.attempts, 1);
        assert!(started.child.process_tree_contained());
        drop(prepared);

        let output = started.child.wait_with_output().await.expect("Git output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("git version"));
    }

    async fn run_runtime_case(
        prepared: &dyn PreparedSandbox,
        workspace: &Path,
        program: PathBuf,
        args: Vec<String>,
        expected_stdout: Option<&str>,
    ) {
        let started = crate::tools::sandbox::start_prepared_sandbox_command(
            prepared,
            SandboxCommand::new(program, args, workspace.to_path_buf()),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("production provider runtime launch");
        assert!(started.child.process_tree_contained());
        let output = started
            .child
            .wait_with_output()
            .await
            .expect("production provider runtime output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(expected) = expected_stdout {
            assert!(
                String::from_utf8_lossy(&output.stdout).contains(expected),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[tokio::test]
    async fn production_provider_runtime_matrix_executes_current_toolchains() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let workspace_model =
            Workspace::new(workspace.path().to_path_buf()).expect("workspace model");
        let helper_backed = acl_helper_executable().is_some();
        let helper_invocations_before =
            APPCONTAINER_RUNTIME_HELPER_INVOCATIONS.load(Ordering::Relaxed);
        let prepared = prepare_with_state_root(&workspace_model, state.path().join("state"))
            .expect("prepare production provider");

        let python = which::which("python.exe")
            .or_else(|_| which::which("python"))
            .expect("Python runtime");
        let python_venv_root = python
            .parent()
            .and_then(Path::parent)
            .filter(|root| root.join("pyvenv.cfg").is_file())
            .map(Path::to_path_buf);
        let mut python_args = vec!["-I".into(), "-c".into()];
        if let Some(venv_root) = python_venv_root.as_deref() {
            python_args.extend([
                "import os,sys; norm=lambda value: os.path.normcase(os.path.normpath(value)); assert norm(sys.prefix)==norm(sys.argv[1]), (sys.prefix, sys.argv[1]); assert norm(sys.executable)==norm(sys.argv[2]), (sys.executable, sys.argv[2]); print('provider-python-ok')".into(),
                venv_root.to_string_lossy().into_owned(),
                python.to_string_lossy().into_owned(),
            ]);
        } else {
            python_args.push("print('provider-python-ok')".into());
        }
        run_runtime_case(
            prepared.as_ref(),
            workspace.path(),
            python.clone(),
            python_args.clone(),
            Some("provider-python-ok"),
        )
        .await;

        let node = which::which("node.exe")
            .or_else(|_| which::which("node"))
            .expect("Node runtime");
        let node_args = vec!["-e".into(), "console.log('provider-node-ok')".into()];
        run_runtime_case(
            prepared.as_ref(),
            workspace.path(),
            node.clone(),
            node_args.clone(),
            Some("provider-node-ok"),
        )
        .await;

        let npm = which::which("npm.cmd")
            .or_else(|_| which::which("npm"))
            .expect("npm runtime");
        let npm_args = vec!["--version".into()];
        run_runtime_case(
            prepared.as_ref(),
            workspace.path(),
            npm.clone(),
            npm_args.clone(),
            None,
        )
        .await;

        let rustc = which::which("rustc.exe")
            .or_else(|_| which::which("rustc"))
            .expect("rustc runtime");
        let rustc_args = vec!["--version".into()];
        run_runtime_case(
            prepared.as_ref(),
            workspace.path(),
            rustc.clone(),
            rustc_args.clone(),
            Some("rustc "),
        )
        .await;

        fs::create_dir_all(workspace.path().join("src")).expect("Cargo src");
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"appcontainer-runtime-matrix\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("Cargo manifest");
        fs::write(
            workspace.path().join("src").join("main.rs"),
            "fn main() { println!(\"provider-cargo-ok\"); }\n",
        )
        .expect("Cargo main");
        let cargo = which::which("cargo.exe")
            .or_else(|_| which::which("cargo"))
            .expect("Cargo runtime");
        run_runtime_case(
            prepared.as_ref(),
            workspace.path(),
            cargo,
            vec!["check".into(), "--quiet".into()],
            None,
        )
        .await;

        let state_layout = prepared.state_layout().expect("provider state layout");
        assert!(state_layout.root.join("cargo-target").exists());
        let marker_root = runtime_grant_marker_root(&state.path().join("state"));
        assert!(
            fs::read_dir(&marker_root)
                .expect("runtime marker directory")
                .next()
                .is_some(),
            "cold runtime preparation did not publish any persistent grant markers"
        );
        let helper_invocations_after_cold =
            APPCONTAINER_RUNTIME_HELPER_INVOCATIONS.load(Ordering::Relaxed);
        if helper_backed {
            assert!(
                helper_invocations_after_cold > helper_invocations_before,
                "helper-backed cold runtime preparation did not invoke the persistent ACL helper"
            );
        }
        drop(prepared);

        let warm = prepare_with_state_root(&workspace_model, state.path().join("warm-state"))
            .expect("prepare warm production provider");
        run_runtime_case(
            warm.as_ref(),
            workspace.path(),
            python,
            python_args,
            Some("provider-python-ok"),
        )
        .await;
        run_runtime_case(
            warm.as_ref(),
            workspace.path(),
            node,
            node_args,
            Some("provider-node-ok"),
        )
        .await;
        run_runtime_case(warm.as_ref(), workspace.path(), npm, npm_args, None).await;
        run_runtime_case(
            warm.as_ref(),
            workspace.path(),
            rustc,
            rustc_args,
            Some("rustc "),
        )
        .await;
        if helper_backed {
            assert_eq!(
                APPCONTAINER_RUNTIME_HELPER_INVOCATIONS.load(Ordering::Relaxed),
                helper_invocations_after_cold,
                "warm runtime preparation re-invoked the persistent ACL helper"
            );
        }
        drop(warm);
    }
    #[test]
    fn concrete_node_runtime_root_reuses_the_resolved_program_parent() {
        let root = tempfile::tempdir().expect("node runtime root");
        let node = root.path().join("node.exe");
        fs::write(&node, b"node fixture").expect("node fixture");
        assert_eq!(
            node_runtime_root(&node).expect("concrete node runtime root"),
            root.path()
        );
    }

    #[test]
    fn npm_normalization_uses_physical_node_and_symlink_preservation_flags() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let workspace_model = Workspace::new(workspace.path().to_path_buf()).expect("workspace");
        let prepared = prepare_with_state_root(&workspace_model, state.path().join("state"))
            .expect("prepare provider");
        let npm = which::which("npm").expect("npm");
        let normalized = prepared
            .normalize_logical_command(SandboxCommand::new(
                npm,
                vec!["--version".into()],
                workspace.path().to_path_buf(),
            ))
            .expect("normalize npm");
        assert_eq!(
            normalized
                .executable
                .file_name()
                .and_then(|name| name.to_str()),
            Some("node.exe")
        );
        assert_eq!(normalized.args[0], "--preserve-symlinks");
        assert_eq!(normalized.args[1], "--preserve-symlinks-main");
        assert!(normalized.args[2].ends_with("npm-cli.js"));
        assert_eq!(
            normalized.args.last().map(String::as_str),
            Some("--version")
        );
        drop(prepared);
    }
}
