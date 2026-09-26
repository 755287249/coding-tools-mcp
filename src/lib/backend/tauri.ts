import { DESKTOP_CAPABILITIES, type BooleanCapability } from "./capabilities";
import { CapabilityError } from "./errors";
import type {
  AlertOptions,
  ConfirmOptions,
  FrontendBackend,
  InvokeFn,
  NativeUi,
  PickDirectoryOptions,
} from "./types";

export interface TauriDialog {
  open(options: {
    directory?: boolean;
    multiple?: boolean;
    defaultPath?: string;
    title?: string;
  }): Promise<string | string[] | null>;
  confirm(message: string, options?: ConfirmOptions): Promise<boolean>;
  message(message: string, options?: AlertOptions): Promise<void>;
}

export interface TauriBackendDeps {
  invoke: InvokeFn;
  dialog: TauriDialog;
}

function unsupported(capability: BooleanCapability): () => Promise<never> {
  return async () => {
    throw new CapabilityError(capability);
  };
}

function createNative(dialog: TauriDialog): NativeUi {
  return {
    pickDirectory(options: PickDirectoryOptions = {}) {
      return dialog.open({
        directory: true,
        multiple: options.multiple ?? false,
        defaultPath: options.defaultPath,
        title: options.title,
      });
    },
    confirm(message, options) {
      return dialog.confirm(message, options);
    },
    alert(message, options) {
      return dialog.message(message, options);
    },
  };
}

export function createTauriBackend(deps: TauriBackendDeps): FrontendBackend {
  const { invoke } = deps;

  return {
    capabilities: DESKTOP_CAPABILITIES,
    native: createNative(deps.dialog),

    workspaces: {
      list: () => invoke("list_workspaces"),
      create: (path, name) => invoke("create_workspace", { path, name }),
      listWslDistributions: () => invoke("list_wsl_distributions"),
      listSandboxBackends: () => invoke("list_sandbox_backends"),
      update: (profile) => invoke("update_workspace", { profile }),
      addFolder: (id, path, name) => invoke("add_workspace_folder", { id, path, name }),
      addWslFolder: (id, distro, linuxPath, name) =>
        invoke("add_wsl_workspace_folder", { id, distro, linuxPath, name }),
      removeFolder: (id, folderId) => invoke("remove_workspace_folder", { id, folderId }),
      openDirectory: (path) => invoke("open_workspace_directory", { path }),
      delete: (id) => invoke("delete_workspace", { id }),
      startRuntime: (id) => invoke("start_runtime", { id }),
      stopRuntime: (id) => invoke("stop_runtime", { id }),
      getRuntimeStatus: (id) => invoke("get_runtime_status", { id }),
      startActionsRuntime: (id) => invoke("start_actions_runtime", { id }),
      stopActionsRuntime: (id) => invoke("stop_actions_runtime", { id }),
      getActionsRuntimeStatus: (id) => invoke("get_actions_runtime_status", { id }),
      restartRuntime: (id) => invoke("restart_runtime", { id }),
      restartActionsRuntime: (id) => invoke("restart_actions_runtime", { id }),
    },

    settings: {
      listFrpProfiles: () => invoke("list_frp_profiles"),
      saveFrpProfile: (profile, token) => invoke("save_frp_profile", { profile, token }),
      deleteFrpProfile: (id) => invoke("delete_frp_profile", { id }),
      getLastWorkspaceId: () => invoke("get_last_workspace_id"),
      setLastWorkspace: (id) => invoke("set_last_workspace", { id }),
      getProxy: () => invoke("get_proxy"),
      setProxy: (proxy) => invoke("set_proxy", { proxy }),
    },

    telemetry: {
      query: (workspaceId, options = {}) =>
        invoke("read_workspace_telemetry", {
          id: workspaceId,
          limit: options.limit,
          errorsOnly: options.errorsOnly,
          minDurationMs: options.minDurationMs,
          sinceTsMs: options.sinceTsMs,
        }),
    },

    activity: {
      read: (workspaceId, sinceRev = 0) =>
        invoke("read_workspace_activity", { id: workspaceId, sinceRev }),
      detail: (workspaceId, seq) =>
        invoke("read_workspace_activity_detail", { id: workspaceId, seq }),
      clear: (workspaceId) => invoke("clear_workspace_activity", { id: workspaceId }),
    },

    history: {
      list: (workspaceId, folderId) =>
        invoke("list_history_sessions", { id: workspaceId, folderId }),
      read: (workspaceId, number, folderId) =>
        invoke("read_history_session", { id: workspaceId, number, folderId }),
    },

    health: {
      run: (workspaceId) => invoke("run_health_checks", { id: workspaceId }),
    },

    logs: {
      readRaw: (workspaceId, service) =>
        invoke("read_workspace_logs", { id: workspaceId, service }),
    },

    secrets: {
      getWorkspaceSecret: (id, key) => invoke("get_workspace_secret", { id, key }),
      setWorkspaceSecret: (id, key, value) => invoke("set_workspace_secret", { id, key, value }),
      regenerateWorkspaceSecret: (id, key) => invoke("regenerate_workspace_secret", { id, key }),
      getSharedSecret: (key) => invoke("get_shared_secret", { key }),
      setSharedSecret: (key, value) => invoke("set_shared_secret", { key, value }),
      regenerateSharedSecret: (key) => invoke("regenerate_shared_secret", { key }),
    },

    software: {
      list: () => invoke("list_software"),
      install: (kind) => invoke("install_software", { kind }),
      uninstall: (kind) => invoke("uninstall_software", { kind }),
      getDownloadConfig: () => invoke("get_download_config"),
      setDownloadConfig: (config) => invoke("set_download_config", { config }),
    },

    tunnel: {
      getFrpSnippet: (id, service) => invoke("get_frp_snippet", { id, service }),
      start: (id, service) => invoke("start_tunnel", { id, service }),
      stop: (id, service) => invoke("stop_tunnel", { id, service }),
      test: (id, service) => invoke("test_tunnel", { id, service }),
      restart: (id, service) => invoke("restart_tunnel", { id, service }),
    },

    agent: {
      restart: unsupported("agentRestart"),
      status: unsupported("agentRestart"),
      loadConfig: unsupported("agentRestart"),
      saveConfig: unsupported("agentRestart"),
    },

    directories: {
      browse: unsupported("directoryBrowser"),
    },

    operations: {
      query: unsupported("operationLogs"),
    },

    workspaceFeatures: {
      skills: (workspaceId) => invoke("get_workspace_skills", { workspaceId }),
      setSkillsActive: (workspaceId, active) =>
        invoke("set_workspace_skills_active", { workspaceId, active }),
      setSkillEnabled: (workspaceId, skillKey, enabled) =>
        invoke("set_workspace_skill_enabled", { workspaceId, skillKey, enabled }),
      extensions: (workspaceId) => invoke("get_workspace_extensions", { workspaceId }),
      setExtensionActive: (workspaceId, extensionKind, active) =>
        invoke("set_workspace_extension_active", { workspaceId, extensionKind, active }),
      setExtensionEnabled: (workspaceId, extensionKind, extensionKey, enabled) =>
        invoke("set_workspace_extension_enabled", {
          workspaceId,
          extensionKind,
          extensionKey,
          enabled,
        }),
    },
  };
}
