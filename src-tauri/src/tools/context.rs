use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use crate::harness::Harness;
use crate::tools::permission::PendingOperationStore;
use crate::tools::policy::PolicySettings;
use crate::tools::sandbox::PreparedSandbox;
use crate::tools::session::SessionStore;
use crate::tools::tool_runtime::{
    descriptor as tool_runtime, MutationLockGroup, ToolExecutionLane,
};
use crate::tools::workspace::{relative_display, Workspace, WorkspaceError};
use crate::workspace::{AuthConfig, RuntimeConfig, SandboxConfig, SecurityPolicy};

pub const DEFAULT_COMMAND_TIMEOUT_MAX_MS: u64 = 30 * 60_000;
pub const ABSOLUTE_COMMAND_TIMEOUT_MAX_MS: u64 = 60 * 60_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionLimits {
    pub blocking_admission: usize,
    pub process_admission: usize,
    pub global_blocking_admission: usize,
    pub global_process_admission: usize,
    pub active_sessions: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            blocking_admission: 128,
            process_admission: 64,
            global_blocking_admission: 1024,
            global_process_admission: 512,
            active_sessions: crate::tools::session::DEFAULT_ACTIVE_SESSION_LIMIT,
        }
    }
}

impl ExecutionLimits {
    pub fn new(
        blocking_admission: usize,
        process_admission: usize,
        active_sessions: usize,
    ) -> Self {
        Self::new_with_global(
            blocking_admission,
            process_admission,
            blocking_admission.saturating_mul(2),
            process_admission.saturating_mul(2),
            active_sessions,
        )
    }

    pub fn new_with_global(
        blocking_admission: usize,
        process_admission: usize,
        global_blocking_admission: usize,
        global_process_admission: usize,
        active_sessions: usize,
    ) -> Self {
        const MAX_CONFIGURED_CONCURRENCY: usize = u16::MAX as usize;
        Self {
            blocking_admission: blocking_admission.clamp(1, MAX_CONFIGURED_CONCURRENCY),
            process_admission: process_admission.clamp(1, MAX_CONFIGURED_CONCURRENCY),
            global_blocking_admission: global_blocking_admission
                .clamp(1, MAX_CONFIGURED_CONCURRENCY),
            global_process_admission: global_process_admission.clamp(1, MAX_CONFIGURED_CONCURRENCY),
            active_sessions: active_sessions.clamp(1, MAX_CONFIGURED_CONCURRENCY),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeToolConfig {
    pub project_isolation: bool,
    pub policy: PolicySettings,
    pub tool_profile: String,
    pub permission_mode: String,
    pub sandbox: SandboxConfig,
}

impl RuntimeToolConfig {
    fn normalized(
        mut policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        sandbox: SandboxConfig,
    ) -> Self {
        if !policy.explicit_security_policy {
            policy.security_policy = SecurityPolicy::legacy(&permission_mode, &tool_profile);
        }
        policy.permission_mode = permission_mode.clone();
        let mut sandbox = sandbox;
        // Full access never prepares AppContainer, whatever path built or
        // updated this runtime config (listener start, hub sync, hot apply).
        if permission_mode.eq_ignore_ascii_case("dangerous") {
            sandbox.enabled = false;
            policy.sandbox_enabled = false;
        }
        let effective_profile = if policy.explicit_security_policy {
            crate::tools::registry::resolve_tool_profile(
                policy.security_policy.compatibility_tool_profile(),
                "trusted",
            )
        } else {
            crate::tools::registry::resolve_tool_profile(&tool_profile, &permission_mode)
        };
        Self {
            project_isolation: false,
            policy,
            tool_profile: effective_profile.into(),
            permission_mode,
            sandbox,
        }
    }
}

#[derive(Clone)]
pub struct SharedRuntimeToolConfig {
    inner: Arc<RwLock<RuntimeToolConfig>>,
}

impl SharedRuntimeToolConfig {
    pub fn new(policy: PolicySettings, tool_profile: String, permission_mode: String) -> Self {
        Self::new_with_sandbox(
            policy,
            tool_profile,
            permission_mode,
            SandboxConfig::default(),
        )
    }

    pub fn new_with_sandbox(
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        sandbox: SandboxConfig,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(RuntimeToolConfig::normalized(
                policy,
                tool_profile,
                permission_mode,
                sandbox,
            ))),
        }
    }

    pub fn from_runtime(runtime: &RuntimeConfig) -> Self {
        Self::new_with_sandbox(
            PolicySettings::from_runtime(runtime),
            runtime.tool_profile.clone(),
            runtime.permission_mode.clone(),
            runtime.sandbox.clone(),
        )
    }

    pub fn snapshot(&self) -> RuntimeToolConfig {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn update(&self, policy: PolicySettings, tool_profile: String, permission_mode: String) {
        let sandbox = self.snapshot().sandbox;
        self.update_with_sandbox(policy, tool_profile, permission_mode, sandbox);
    }

    pub fn update_with_sandbox(
        &self,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        sandbox: SandboxConfig,
    ) {
        let mut current = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let isolated = current.project_isolation;
        let mut next =
            RuntimeToolConfig::normalized(policy, tool_profile, permission_mode, sandbox);
        if isolated {
            next.project_isolation = true;
            next.policy.security_policy.enforce_workspace_boundary = true;
            next.policy.sandbox_enabled = next.sandbox.enabled;
            next.sandbox.external_paths.clear();
        }
        *current = next;
    }

    pub fn require_project_isolation(&self) {
        let mut config = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        config.project_isolation = true;
        config.policy.security_policy.enforce_workspace_boundary = true;
        config.policy.sandbox_enabled = config.sandbox.enabled;
        config.sandbox.external_paths.clear();
    }

    pub fn update_from_runtime(&self, runtime: &RuntimeConfig) {
        self.update_with_sandbox(
            PolicySettings::from_runtime(runtime),
            runtime.tool_profile.clone(),
            runtime.permission_mode.clone(),
            runtime.sandbox.clone(),
        );
    }
}

#[derive(Clone)]
struct MutationLocks {
    history: Arc<AsyncMutex<()>>,
    workspace_content: Arc<AsyncMutex<()>>,
    git: Arc<AsyncMutex<()>>,
    task: Arc<AsyncMutex<()>>,
    cwd: Arc<AsyncMutex<()>>,
}

impl MutationLocks {
    fn new() -> Self {
        Self {
            history: Arc::new(AsyncMutex::new(())),
            workspace_content: Arc::new(AsyncMutex::new(())),
            git: Arc::new(AsyncMutex::new(())),
            task: Arc::new(AsyncMutex::new(())),
            cwd: Arc::new(AsyncMutex::new(())),
        }
    }

    fn lock(&self, group: MutationLockGroup) -> Arc<AsyncMutex<()>> {
        match group {
            MutationLockGroup::History => self.history.clone(),
            MutationLockGroup::WorkspaceContent => self.workspace_content.clone(),
            MutationLockGroup::Git => self.git.clone(),
            MutationLockGroup::Task => self.task.clone(),
            MutationLockGroup::Cwd => self.cwd.clone(),
        }
    }
}

struct CachedPreparedSandbox {
    workspace_root: PathBuf,
    config: SandboxConfig,
    prepared: Arc<dyn PreparedSandbox>,
}

struct SharedExecutionResources {
    sessions: Arc<SessionStore>,
    pending_operations: Arc<PendingOperationStore>,
    blocking_admission: Arc<Semaphore>,
    process_admission: Arc<Semaphore>,
    mutation_locks: MutationLocks,
    resource_locks: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
    sandbox_backend_cache: Mutex<Option<CachedPreparedSandbox>>,
    limits: ExecutionLimits,
}

struct GlobalAdmissionResources {
    blocking_admission: Arc<Semaphore>,
    process_admission: Arc<Semaphore>,
    limits: ExecutionLimits,
}

static EXECUTION_RESOURCES: OnceLock<Mutex<HashMap<String, Weak<SharedExecutionResources>>>> =
    OnceLock::new();
static GLOBAL_ADMISSION_RESOURCES: OnceLock<
    Mutex<HashMap<String, Weak<GlobalAdmissionResources>>>,
> = OnceLock::new();

fn shared_execution_resources(
    profile_id: &str,
    limits: ExecutionLimits,
) -> Arc<SharedExecutionResources> {
    let registry = EXECUTION_RESOURCES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().expect("execution resources lock");
    registry.retain(|_, resources| resources.strong_count() > 0);
    if let Some(resources) = registry.get(profile_id).and_then(Weak::upgrade) {
        return resources;
    }
    let resources = Arc::new(SharedExecutionResources {
        sessions: Arc::new(SessionStore::with_active_session_limit(
            limits.active_sessions,
        )),
        pending_operations: Arc::new(PendingOperationStore::new()),
        blocking_admission: Arc::new(Semaphore::new(limits.blocking_admission)),
        process_admission: Arc::new(Semaphore::new(limits.process_admission)),
        mutation_locks: MutationLocks::new(),
        resource_locks: Arc::new(Mutex::new(HashMap::new())),
        sandbox_backend_cache: Mutex::new(None),
        limits,
    });
    registry.insert(profile_id.to_string(), Arc::downgrade(&resources));
    resources
}

fn shared_global_admission_resources(
    resource_id: &str,
    limits: ExecutionLimits,
) -> Arc<GlobalAdmissionResources> {
    let registry = GLOBAL_ADMISSION_RESOURCES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().expect("global admission resources lock");
    registry.retain(|_, resources| resources.strong_count() > 0);
    if let Some(resources) = registry.get(resource_id).and_then(Weak::upgrade) {
        return resources;
    }
    let resources = Arc::new(GlobalAdmissionResources {
        blocking_admission: Arc::new(Semaphore::new(limits.global_blocking_admission)),
        process_admission: Arc::new(Semaphore::new(limits.global_process_admission)),
        limits,
    });
    registry.insert(resource_id.to_string(), Arc::downgrade(&resources));
    resources
}

pub struct ToolContext {
    pub workspace: Workspace,
    pub profile_id: String,
    pub auth: AuthConfig,
    runtime_config: SharedRuntimeToolConfig,
    pub harness: Harness,
    default_cwd: Mutex<PathBuf>,
    pub sessions: Arc<SessionStore>,
    pub pending_operations: Arc<PendingOperationStore>,
    blocking_admission: Arc<Semaphore>,
    process_admission: Arc<Semaphore>,
    global_blocking_admission: Arc<Semaphore>,
    global_process_admission: Arc<Semaphore>,
    mutation_locks: MutationLocks,
    resource_locks: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
    execution_limits: ExecutionLimits,
    _execution_resources: Arc<SharedExecutionResources>,
    _global_admission_resources: Arc<GlobalAdmissionResources>,
}

pub type SharedToolContext = Arc<ToolContext>;

impl ToolContext {
    pub fn new(workspace_path: PathBuf) -> Result<Self, String> {
        let workspace = Workspace::new(workspace_path).map_err(|e| e.message())?;
        let auth = AuthConfig {
            auth_type: "noauth".into(),
            ..AuthConfig::default()
        };
        Ok(Self::from_workspace(
            workspace,
            auth,
            PolicySettings::default(),
            "full".into(),
            "trusted".into(),
        ))
    }

    pub fn from_workspace(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
    ) -> Self {
        let harness_root = Harness::default_root().expect("无法初始化 Harness 数据目录");
        Self::from_workspace_with_harness_root(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            harness_root,
        )
    }

    pub fn from_workspace_with_profile_id(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        profile_id: String,
    ) -> Self {
        Self::from_workspace_with_profile_id_and_limits(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            profile_id,
            ExecutionLimits::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_workspace_with_profile_id_and_limits(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        profile_id: String,
        limits: ExecutionLimits,
    ) -> Self {
        let harness_root = Harness::default_root().expect("无法初始化 Harness 数据目录");
        let execution_resource_id = profile_id.clone();
        Self::from_workspace_with_identity(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            harness_root,
            Some(profile_id),
            Some(execution_resource_id.clone()),
            Some(execution_resource_id),
            limits,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_workspace_with_profile_id_and_resource_id_and_limits(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        profile_id: String,
        execution_resource_id: String,
        limits: ExecutionLimits,
    ) -> Self {
        let harness_root = Harness::default_root().expect("无法初始化 Harness 数据目录");
        Self::from_workspace_with_identity(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            harness_root,
            Some(profile_id),
            Some(execution_resource_id.clone()),
            Some(execution_resource_id),
            limits,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_workspace_with_profile_id_and_resource_ids_and_limits(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        profile_id: String,
        execution_resource_id: String,
        global_admission_resource_id: String,
        limits: ExecutionLimits,
    ) -> Self {
        let harness_root = Harness::default_root().expect("无法初始化 Harness 数据目录");
        Self::from_workspace_with_identity(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            harness_root,
            Some(profile_id),
            Some(execution_resource_id),
            Some(global_admission_resource_id),
            limits,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_workspace_with_shared_runtime_config_and_resource_ids_and_limits(
        workspace: Workspace,
        auth: AuthConfig,
        runtime_config: SharedRuntimeToolConfig,
        profile_id: String,
        execution_resource_id: String,
        global_admission_resource_id: String,
        limits: ExecutionLimits,
    ) -> Self {
        let harness_root = Harness::default_root().expect("无法初始化 Harness 数据目录");
        Self::from_workspace_with_identity_and_runtime_config(
            workspace,
            auth,
            runtime_config,
            harness_root,
            Some(profile_id),
            Some(execution_resource_id),
            Some(global_admission_resource_id),
            limits,
        )
    }

    pub fn from_workspace_with_harness_root(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        harness_root: PathBuf,
    ) -> Self {
        Self::from_workspace_with_identity(
            workspace,
            auth,
            policy,
            tool_profile,
            permission_mode,
            harness_root,
            None,
            None,
            None,
            ExecutionLimits::default(),
        )
    }

    fn from_workspace_with_identity(
        workspace: Workspace,
        auth: AuthConfig,
        policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
        harness_root: PathBuf,
        explicit_profile_id: Option<String>,
        explicit_execution_resource_id: Option<String>,
        explicit_global_admission_resource_id: Option<String>,
        limits: ExecutionLimits,
    ) -> Self {
        Self::from_workspace_with_identity_and_runtime_config(
            workspace,
            auth,
            SharedRuntimeToolConfig::new(policy, tool_profile, permission_mode),
            harness_root,
            explicit_profile_id,
            explicit_execution_resource_id,
            explicit_global_admission_resource_id,
            limits,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_workspace_with_identity_and_runtime_config(
        workspace: Workspace,
        auth: AuthConfig,
        runtime_config: SharedRuntimeToolConfig,
        harness_root: PathBuf,
        explicit_profile_id: Option<String>,
        explicit_execution_resource_id: Option<String>,
        explicit_global_admission_resource_id: Option<String>,
        limits: ExecutionLimits,
    ) -> Self {
        let root = workspace.root().to_path_buf();
        let harness = Harness::new(root.clone(), harness_root).expect("无法初始化 Harness");
        let profile_id = explicit_profile_id.unwrap_or_else(|| harness.workspace_id().to_string());
        let execution_resource_id = explicit_execution_resource_id
            .as_deref()
            .unwrap_or(&profile_id);
        let global_admission_resource_id = explicit_global_admission_resource_id
            .as_deref()
            .unwrap_or(execution_resource_id);
        let resources = shared_execution_resources(execution_resource_id, limits);
        let global_resources =
            shared_global_admission_resources(global_admission_resource_id, limits);
        Self {
            workspace,
            profile_id,
            auth,
            runtime_config,
            harness,
            default_cwd: Mutex::new(root),
            sessions: resources.sessions.clone(),
            pending_operations: resources.pending_operations.clone(),
            blocking_admission: resources.blocking_admission.clone(),
            process_admission: resources.process_admission.clone(),
            global_blocking_admission: global_resources.blocking_admission.clone(),
            global_process_admission: global_resources.process_admission.clone(),
            mutation_locks: resources.mutation_locks.clone(),
            resource_locks: resources.resource_locks.clone(),
            execution_limits: resources.limits,
            _execution_resources: resources,
            _global_admission_resources: global_resources,
        }
    }

    pub fn runtime_config(&self) -> RuntimeToolConfig {
        self.runtime_config.snapshot()
    }

    pub fn shared_runtime_config(&self) -> SharedRuntimeToolConfig {
        self.runtime_config.clone()
    }

    pub fn update_runtime_config(
        &self,
        mut policy: PolicySettings,
        tool_profile: String,
        permission_mode: String,
    ) {
        policy.explicit_security_policy = false;
        policy.security_policy = SecurityPolicy::legacy(&permission_mode, &tool_profile);
        self.runtime_config
            .update(policy, tool_profile, permission_mode);
    }

    pub fn for_test(workspace_path: PathBuf, harness_root: PathBuf) -> Result<Self, String> {
        let workspace = Workspace::new(workspace_path).map_err(|e| e.message())?;
        Ok(Self::from_workspace_with_harness_root(
            workspace,
            AuthConfig {
                auth_type: "noauth".into(),
                ..AuthConfig::default()
            },
            PolicySettings::default(),
            "full".into(),
            "trusted".into(),
            harness_root,
        ))
    }

    pub fn workspace_path(&self) -> String {
        self.workspace.root_display()
    }

    pub fn default_cwd_display(&self) -> String {
        let cwd = self.default_cwd.lock().expect("cwd lock");
        let display = relative_display(self.workspace.root(), &cwd);
        if display.is_empty() {
            ".".into()
        } else {
            display
        }
    }

    pub fn set_default_cwd(&self, path: PathBuf) {
        *self.default_cwd.lock().expect("cwd lock") = path;
    }

    pub fn default_cwd_path(&self) -> PathBuf {
        self.default_cwd.lock().expect("cwd lock").clone()
    }

    pub(crate) fn mutation_lock_for(&self, group: MutationLockGroup) -> Arc<AsyncMutex<()>> {
        self.mutation_locks.lock(group)
    }

    pub(crate) fn resource_lock(&self, name: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self.resource_locks.lock().expect("resource locks registry");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(name).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(name.to_string(), Arc::downgrade(&lock));
        lock
    }

    pub(crate) fn cached_sandbox_backend<F>(
        &self,
        config: &SandboxConfig,
        prepare: F,
    ) -> Result<Arc<dyn PreparedSandbox>, WorkspaceError>
    where
        F: FnOnce() -> Result<Arc<dyn PreparedSandbox>, WorkspaceError>,
    {
        let workspace_root = self.workspace.root().to_path_buf();
        let mut cache = self
            ._execution_resources
            .sandbox_backend_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.as_ref() {
            if cached.workspace_root == workspace_root && cached.config == *config {
                return Ok(Arc::clone(&cached.prepared));
            }
        }
        cache.take();

        let prepared = prepare()?;
        *cache = Some(CachedPreparedSandbox {
            workspace_root,
            config: config.clone(),
            prepared: Arc::clone(&prepared),
        });
        Ok(prepared)
    }

    pub fn execution_limits(&self) -> ExecutionLimits {
        self.execution_limits
    }

    pub(crate) fn admission_for(
        &self,
        tool_name: &str,
    ) -> Option<(&'static str, usize, Arc<Semaphore>, usize, Arc<Semaphore>)> {
        match tool_runtime(tool_name).lane {
            // Keep lightweight fast/control calls responsive even when heavy
            // filesystem or process work is queued.
            ToolExecutionLane::Fast | ToolExecutionLane::Control => None,
            ToolExecutionLane::Process => Some((
                "process",
                self.execution_limits.process_admission,
                self.process_admission.clone(),
                self._global_admission_resources
                    .limits
                    .global_process_admission,
                self.global_process_admission.clone(),
            )),
            ToolExecutionLane::Blocking => Some((
                "blocking",
                self.execution_limits.blocking_admission,
                self.blocking_admission.clone(),
                self._global_admission_resources
                    .limits
                    .global_blocking_admission,
                self.global_blocking_admission.clone(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn folders_share_global_admission_but_keep_local_capacity() {
        let first_workspace = tempfile::tempdir().expect("first workspace");
        let second_workspace = tempfile::tempdir().expect("second workspace");
        let limits = ExecutionLimits::new_with_global(2, 2, 3, 3, 4);
        let make = |path: PathBuf, local_id: &str| {
            ToolContext::from_workspace_with_profile_id_and_resource_ids_and_limits(
                Workspace::new(path).expect("workspace"),
                AuthConfig::default(),
                PolicySettings::default(),
                "core".into(),
                "trusted".into(),
                "global-admission-test".into(),
                local_id.into(),
                "shared-runtime-admission".into(),
                limits,
            )
        };
        let first = make(first_workspace.path().to_path_buf(), "folder-a");
        let second = make(second_workspace.path().to_path_buf(), "folder-b");
        let (_, _, first_local, _, first_global) = first
            .admission_for("exec_command")
            .expect("first admission");
        let (_, _, second_local, _, second_global) = second
            .admission_for("exec_command")
            .expect("second admission");

        assert!(!Arc::ptr_eq(&first_local, &second_local));
        assert!(Arc::ptr_eq(&first_global, &second_global));
        let _global_permits = first_global
            .clone()
            .acquire_many_owned(3)
            .await
            .expect("global permits");
        assert!(second_global.clone().try_acquire_owned().is_err());
        assert!(second_local.clone().try_acquire_owned().is_ok());
    }

    #[tokio::test]
    async fn layered_admission_stress_caps_peaks_and_releases_permits() {
        let first_workspace = tempfile::tempdir().expect("first workspace");
        let second_workspace = tempfile::tempdir().expect("second workspace");
        let limits = ExecutionLimits::new_with_global(2, 2, 3, 3, 4);
        let make = |path: PathBuf, local_id: &str| {
            ToolContext::from_workspace_with_profile_id_and_resource_ids_and_limits(
                Workspace::new(path).expect("workspace"),
                AuthConfig::default(),
                PolicySettings::default(),
                "core".into(),
                "trusted".into(),
                "layered-admission-stress".into(),
                local_id.into(),
                "shared-stress-runtime".into(),
                limits,
            )
        };
        let first = make(first_workspace.path().to_path_buf(), "stress-folder-a");
        let second = make(second_workspace.path().to_path_buf(), "stress-folder-b");
        let (_, _, first_local, _, shared_global) = first
            .admission_for("exec_command")
            .expect("first admission");
        let (_, _, second_local, _, second_global) = second
            .admission_for("exec_command")
            .expect("second admission");

        assert!(Arc::ptr_eq(&shared_global, &second_global));

        let active_global = Arc::new(AtomicUsize::new(0));
        let peak_global = Arc::new(AtomicUsize::new(0));
        let active_first = Arc::new(AtomicUsize::new(0));
        let peak_first = Arc::new(AtomicUsize::new(0));
        let active_second = Arc::new(AtomicUsize::new(0));
        let peak_second = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for index in 0..12 {
            let local = if index % 2 == 0 {
                first_local.clone()
            } else {
                second_local.clone()
            };
            let global = shared_global.clone();
            let active_global = active_global.clone();
            let peak_global = peak_global.clone();
            let (active_local, peak_local) = if index % 2 == 0 {
                (active_first.clone(), peak_first.clone())
            } else {
                (active_second.clone(), peak_second.clone())
            };
            tasks.push(tokio::spawn(async move {
                let _local_permit = local.acquire_owned().await.expect("local permit");
                let _global_permit = global.acquire_owned().await.expect("global permit");

                let global_now = active_global.fetch_add(1, Ordering::SeqCst) + 1;
                peak_global.fetch_max(global_now, Ordering::SeqCst);
                let local_now = active_local.fetch_add(1, Ordering::SeqCst) + 1;
                peak_local.fetch_max(local_now, Ordering::SeqCst);

                tokio::time::sleep(Duration::from_millis(30)).await;

                active_local.fetch_sub(1, Ordering::SeqCst);
                active_global.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        for task in tasks {
            task.await.expect("stress task");
        }

        assert_eq!(peak_global.load(Ordering::SeqCst), 3);
        assert!(peak_first.load(Ordering::SeqCst) <= 2);
        assert!(peak_second.load(Ordering::SeqCst) <= 2);
        assert_eq!(shared_global.available_permits(), 3);
        assert_eq!(first_local.available_permits(), 2);
        assert_eq!(second_local.available_permits(), 2);
    }

    #[test]
    fn contexts_with_the_same_profile_share_execution_resources_and_limits() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let limits = ExecutionLimits::new(6, 3, 12);
        let make = || {
            ToolContext::from_workspace_with_profile_id_and_limits(
                Workspace::new(workspace.path().to_path_buf()).expect("workspace"),
                AuthConfig::default(),
                PolicySettings::default(),
                "core".into(),
                "trusted".into(),
                "shared-profile-test".into(),
                limits,
            )
        };

        let first = make();
        let second = make();
        assert!(Arc::ptr_eq(&first.sessions, &second.sessions));
        assert!(Arc::ptr_eq(
            &first.pending_operations,
            &second.pending_operations
        ));
        let first_content = first.mutation_lock_for(MutationLockGroup::WorkspaceContent);
        let second_content = second.mutation_lock_for(MutationLockGroup::WorkspaceContent);
        let first_history = first.mutation_lock_for(MutationLockGroup::History);
        assert!(Arc::ptr_eq(&first_content, &second_content));
        assert!(!Arc::ptr_eq(&first_content, &first_history));
        assert!(Arc::ptr_eq(
            &first.resource_lock("cargo-target"),
            &second.resource_lock("cargo-target")
        ));
        assert_eq!(first.execution_limits(), limits);
        assert_eq!(first.sessions.active_session_limit(), 12);
        assert_eq!(first.admission_for("read_file").unwrap().1, 6);
        assert_eq!(first.admission_for("read_file").unwrap().3, 12);
        assert_eq!(first.admission_for("exec_command").unwrap().1, 3);
        assert_eq!(first.admission_for("exec_command").unwrap().3, 6);
    }
}
