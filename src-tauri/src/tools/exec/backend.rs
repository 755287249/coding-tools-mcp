use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::tools::context::ToolContext;
use crate::tools::process_start::{
    spawn_with_control, ControlledProcessStartError, ProcessStartError, StartedChild,
};
use crate::tools::sandbox::{
    ensure_execution_available, prepare_enabled_backend, start_prepared_sandbox_command,
    PreparedSandbox, SandboxCommand,
};
use crate::tools::workspace::{Workspace, WorkspaceError};
use crate::workspace::SandboxConfig;

use super::result::process_start_workspace_error;
use super::runner::{prepared_command, CommandIoMode};
use super::spec::ExecSpec;

#[derive(Clone)]
pub(super) enum CommandExecutionBackend {
    Native,
    Sandbox(Arc<dyn PreparedSandbox>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommandExecutionBoundary {
    PolicyOnly,
    Sandbox { backend_id: String },
}

impl CommandExecutionBoundary {
    pub(super) fn from_config(
        config: &SandboxConfig,
        permission_mode: &str,
        workspace: &Workspace,
    ) -> Result<Self, WorkspaceError> {
        // Last line of defense: full access never starts an OS sandbox, even if
        // a stale or reconstructed config still says the sandbox is enabled.
        if !config.enabled || permission_mode.eq_ignore_ascii_case("dangerous") {
            return Ok(Self::PolicyOnly);
        }
        ensure_execution_available(config, workspace)?;
        Ok(Self::Sandbox {
            backend_id: config.backend.trim().to_string(),
        })
    }

    pub(super) fn allows_native_diagnostic(&self) -> bool {
        matches!(self, Self::PolicyOnly)
    }

    pub(super) fn prepare_backend(
        &self,
        config: &SandboxConfig,
        ctx: &ToolContext,
    ) -> Result<CommandExecutionBackend, WorkspaceError> {
        match self {
            Self::PolicyOnly => Ok(CommandExecutionBackend::Native),
            Self::Sandbox { backend_id } => {
                if config.backend.trim() != backend_id {
                    return Err(WorkspaceError::ToolDetails {
                        code: "SANDBOX_CONFIGURATION_CHANGED",
                        message: "Sandbox configuration changed after execution preflight.".into(),
                        category: "security",
                        retryable: false,
                        details: json!({
                            "sandbox_enabled": true,
                            "sandbox_backend": backend_id,
                            "configured_backend": config.backend.trim(),
                            "fallback_allowed": false
                        }),
                    });
                }
                let prepared = ctx.cached_sandbox_backend(config, || {
                    prepare_enabled_backend(config, &ctx.workspace).map(Arc::from)
                })?;
                Ok(CommandExecutionBackend::Sandbox(prepared))
            }
        }
    }

    pub(super) fn attach_result_metadata(
        &self,
        result: &mut Value,
        child_process: bool,
        sandbox_started: bool,
    ) {
        let Some(object) = result.as_object_mut() else {
            return;
        };
        object.insert("child_process".into(), Value::Bool(child_process));
        match self {
            Self::PolicyOnly => {
                object.insert("sandbox_enforced".into(), Value::Bool(false));
                object.insert(
                    "execution_boundary".into(),
                    Value::String("policy_only".into()),
                );
                object.remove("sandbox_backend");
            }
            Self::Sandbox { backend_id } => {
                object.insert("sandbox_enforced".into(), Value::Bool(sandbox_started));
                object.insert("sandbox_backend".into(), Value::String(backend_id.clone()));
                object.insert(
                    "execution_boundary".into(),
                    Value::String(if sandbox_started {
                        backend_id.clone()
                    } else {
                        "sandbox_start_failed".into()
                    }),
                );
            }
        }
    }
}

pub(super) async fn start_exec_process(
    backend: &CommandExecutionBackend,
    spec: &ExecSpec,
    cwd: &Path,
    io_mode: CommandIoMode,
) -> Result<StartedChild, WorkspaceError> {
    match backend {
        CommandExecutionBackend::Native => {
            spawn_with_control(|| prepared_command(spec, cwd, io_mode))
                .await
                .map_err(process_start_workspace_error)
        }
        CommandExecutionBackend::Sandbox(prepared) => {
            let command = SandboxCommand::new(
                PathBuf::from(&spec.program),
                spec.args.clone(),
                cwd.to_path_buf(),
            );
            match start_prepared_sandbox_command(
                prepared.as_ref(),
                command,
                spec.env.clone(),
                spec.remove_env.clone(),
            )
            .await
            {
                Ok(started) => Ok(started),
                Err(ControlledProcessStartError::Start(error)) => Err(error),
                Err(ControlledProcessStartError::LoaderInitialization {
                    exit_code,
                    diagnostics,
                }) => Err(process_start_workspace_error(
                    ProcessStartError::LoaderInitialization {
                        exit_code,
                        diagnostics,
                    },
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_sandbox_selects_policy_only_boundary() {
        let root = tempfile::tempdir().expect("workspace");
        let workspace = Workspace::new(root.path().to_path_buf()).expect("workspace model");
        let config = SandboxConfig {
            enabled: false,
            backend: "appcontainer".into(),
            ..SandboxConfig::default()
        };
        assert_eq!(
            CommandExecutionBoundary::from_config(&config, "trusted", &workspace)
                .expect("boundary"),
            CommandExecutionBoundary::PolicyOnly
        );
    }

    #[test]
    fn full_access_never_selects_appcontainer_boundary() {
        let root = tempfile::tempdir().expect("workspace");
        let workspace = Workspace::new(root.path().to_path_buf()).expect("workspace model");
        let config = SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..SandboxConfig::default()
        };
        assert_eq!(
            CommandExecutionBoundary::from_config(&config, "dangerous", &workspace)
                .expect("boundary"),
            CommandExecutionBoundary::PolicyOnly
        );
    }

    #[cfg(windows)]
    #[test]
    fn enabled_appcontainer_selects_sandbox_boundary_when_ready() {
        let root = tempfile::tempdir().expect("workspace");
        let workspace = Workspace::new(root.path().to_path_buf()).expect("workspace model");
        let config = SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..SandboxConfig::default()
        };
        assert_eq!(
            CommandExecutionBoundary::from_config(&config, "trusted", &workspace)
                .expect("boundary"),
            CommandExecutionBoundary::Sandbox {
                backend_id: "appcontainer".into()
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn appcontainer_preparation_is_reused_for_the_same_context_and_config() {
        let root = tempfile::tempdir().expect("workspace");
        let harness = tempfile::tempdir().expect("harness");
        let ctx = ToolContext::for_test(root.path().to_path_buf(), harness.path().to_path_buf())
            .expect("context");
        let config = SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..SandboxConfig::default()
        };
        let boundary = CommandExecutionBoundary::from_config(&config, "trusted", &ctx.workspace)
            .expect("boundary");
        let first = boundary
            .prepare_backend(&config, &ctx)
            .expect("first prepare");
        let second = boundary
            .prepare_backend(&config, &ctx)
            .expect("second prepare");

        match (first, second) {
            (CommandExecutionBackend::Sandbox(first), CommandExecutionBackend::Sandbox(second)) => {
                assert!(Arc::ptr_eq(&first, &second));
            }
            _ => panic!("enabled AppContainer must use a sandbox backend"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn appcontainer_preparation_is_rebuilt_when_config_changes() {
        let root = tempfile::tempdir().expect("workspace");
        let harness = tempfile::tempdir().expect("harness");
        let ctx = ToolContext::for_test(root.path().to_path_buf(), harness.path().to_path_buf())
            .expect("context");
        let first_config = SandboxConfig {
            enabled: true,
            backend: "appcontainer".into(),
            ..SandboxConfig::default()
        };
        let mut second_config = first_config.clone();
        second_config
            .options
            .insert("appcontainer.network".into(), "internet".into());
        let boundary =
            CommandExecutionBoundary::from_config(&first_config, "trusted", &ctx.workspace)
                .expect("boundary");
        let first = boundary
            .prepare_backend(&first_config, &ctx)
            .expect("first prepare");
        let second = boundary
            .prepare_backend(&second_config, &ctx)
            .expect("changed prepare");

        match (first, second) {
            (CommandExecutionBackend::Sandbox(first), CommandExecutionBackend::Sandbox(second)) => {
                assert!(!Arc::ptr_eq(&first, &second));
            }
            _ => panic!("enabled AppContainer must use a sandbox backend"),
        }
    }

    #[test]
    fn sandbox_result_metadata_distinguishes_started_from_start_failure() {
        let boundary = CommandExecutionBoundary::Sandbox {
            backend_id: "appcontainer".into(),
        };
        let mut started = json!({});
        boundary.attach_result_metadata(&mut started, true, true);
        assert_eq!(started["sandbox_enforced"], true);
        assert_eq!(started["sandbox_backend"], "appcontainer");
        assert_eq!(started["execution_boundary"], "appcontainer");
        assert_eq!(started["child_process"], true);

        let mut failed = json!({});
        boundary.attach_result_metadata(&mut failed, false, false);
        assert_eq!(failed["sandbox_enforced"], false);
        assert_eq!(failed["sandbox_backend"], "appcontainer");
        assert_eq!(failed["execution_boundary"], "sandbox_start_failed");
        assert_eq!(failed["child_process"], false);
    }
}
