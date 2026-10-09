<script lang="ts">
  import { get as getStore } from "svelte/store";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import { appUrl } from "$lib/app-path";
  import Tabs from "$lib/components/Tabs.svelte";
  import type { ActionsPolicyDraft } from "$lib/components/ActionsPolicyForm.svelte";
  import type { RuntimePolicyDraft } from "$lib/components/RuntimePolicyForm.svelte";
  import type {
    SaveTunnelOptions,
    TunnelFormConfig,
  } from "$lib/components/TunnelConfigForm.svelte";
  import {
    deleteWorkspace,
    getActionsRuntimeStatus,
    getRuntimeStatus,
    listWorkspaces,
    restartActionsRuntime,
    restartRuntime,
    startActionsRuntime,
    startRuntime,
    stopActionsRuntime,
    stopRuntime,
    updateWorkspace,
  } from "$lib/api/workspaces";
  import { listFrpProfiles, setLastWorkspace, type FrpProfileDto } from "$lib/api/settings";
  import { confirm } from "$lib/api/native";
  import { getBackend } from "$lib/backend";
  import { restartTunnel, stopTunnel, testTunnel } from "$lib/api/tunnel";
  import { runServiceToggle, notifyStartFailure } from "$lib/runtime/service";
  import { showToast } from "$lib/stores/toast";
  import { promptServiceRestart } from "$lib/runtime/restart-hint";
  import { actionsRuntimeStates, mcpRuntimeStates, workspaces } from "$lib/stores/app";
  import {
    actionsConfig,
    actionsLocalEndpoint,
    compatibilityPermissionMode,
    compatibilityToolProfile,
    frpPublicUrl,
    mcpLocalEndpoint,
    sandboxConfig,
    securityPolicyConfig,
    type ActionsAuthDraft,
    type AuthConfig,
    type SandboxConfig,
    type RuntimeState,
    type WorkspaceProfile,
    workspaceFolders,
  } from "$lib/types";
  import { t } from "$lib/i18n";
  import { chatLocation } from '$lib/chat/location';
  import Settings from '@lucide/svelte/icons/settings';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import ListChecks from '@lucide/svelte/icons/list-checks';
  import ActivityDrawer from '$lib/components/activity/ActivityDrawer.svelte';
  import { activityPanelExpanded, activityPanelOpen } from '$lib/stores/activity-panel';
  import ChatPanel from "$lib/components/chat/ChatPanel.svelte";
  import AutoWorkspaceView from "$lib/components/auto/AutoWorkspaceView.svelte";
  import { uiMode } from "$lib/stores/ui-mode";

  type WorkspaceTab = "overview" | "history" | "telemetry" | "logs" | "health" | "features" | "mcp" | "actions" | "settings" | "chat";
  type ServiceSection = "service" | "tunnel" | "auth" | "policy" | "logs" | "health";

  const capabilities = getBackend().capabilities;
  const workspaceTabValues: WorkspaceTab[] = [
    "chat",
    "overview",
    "history",
    "telemetry",
    "logs",
    "health",
    "features",
    "mcp",
    "actions",
    "settings",
  ];
  const serviceSectionValues: ServiceSection[] = [
    "service",
    "tunnel",
    "auth",
    "policy",
    "logs",
    "health",
  ];

  let profile = $state<WorkspaceProfile | null>(null);
  let mcpStatus = $state<RuntimeState>("stopped");
  let actionsStatus = $state<RuntimeState>("stopped");
  let mcpStatusMessage = $state("");
  let actionsStatusMessage = $state("");
  let mcpBusy = $state(false);
  let actionsBusy = $state(false);
  let mcpLocal = $state("");
  let mcpPublic = $state("");
  let actionsLocal = $state("");
  let actionsPublic = $state("");
  let frpProfiles = $state<FrpProfileDto[]>([]);
  let activeWorkspaceTab = $state<WorkspaceTab>("chat");
  let mcpSection = $state<ServiceSection>("service");
  let actionsSection = $state<ServiceSection>("service");
  let loadGeneration = 0;

  let overviewPanelPromise:
    | Promise<typeof import("$lib/components/workspace/WorkspaceOverview.svelte")>
    | undefined;
  let settingsPanelPromise:
    | Promise<typeof import("$lib/components/workspace/WorkspaceSettings.svelte")>
    | undefined;
  let mcpPanelPromise:
    | Promise<typeof import("$lib/components/workspace/McpWorkspacePanel.svelte")>
    | undefined;
  let actionsPanelPromise:
    | Promise<typeof import("$lib/components/workspace/ActionsWorkspacePanel.svelte")>
    | undefined;
  let historyViewerPromise:
    | Promise<typeof import("$lib/components/HistoryViewer.svelte")>
    | undefined;
  let telemetryViewerPromise:
    | Promise<typeof import("$lib/components/TelemetryViewer.svelte")>
    | undefined;
  let operationLogViewerPromise:
    | Promise<typeof import("$lib/components/OperationLogViewer.svelte")>
    | undefined;
  let healthPanelPromise:
    | Promise<typeof import("$lib/components/HealthPanel.svelte")>
    | undefined;
  let workspaceFeatureControlsPromise:
    | Promise<typeof import("$lib/components/workspace/WorkspaceFeatureControls.svelte")>
    | undefined;

  const workspaceTabs = $derived(
    [
      { value: "chat", label: $t("chat.46") },
      { value: "overview", label: $t("Overview") },
      { value: "history", label: $t("History") },
      { value: "telemetry", label: $t("Telemetry") },
      capabilities.operationLogs ? { value: "logs", label: $t("Operation logs") } : null,
      { value: "health", label: $t("Health") },
      capabilities.workspaceFeatureControls
        ? { value: "features", label: $t("Skills / Hooks / MCP") }
        : null,
      { value: "mcp", label: "MCP" },
      capabilities.actions ? { value: "actions", label: "Actions" } : null,
      { value: "settings", label: $t("Workspace settings") },
    ].filter((item): item is { value: string; label: string } => item != null),
  );

  const workspaceId = $derived($page.params.id);
  const actions = $derived(profile ? actionsConfig(profile) : null);
  const sandboxLocked = false;

  function loadOverviewPanel() {
    return (overviewPanelPromise ??= import("$lib/components/workspace/WorkspaceOverview.svelte"));
  }

  function loadSettingsPanel() {
    return (settingsPanelPromise ??= import("$lib/components/workspace/WorkspaceSettings.svelte"));
  }

  function loadMcpPanel() {
    return (mcpPanelPromise ??= import("$lib/components/workspace/McpWorkspacePanel.svelte"));
  }

  function loadActionsPanel() {
    return (actionsPanelPromise ??= import("$lib/components/workspace/ActionsWorkspacePanel.svelte"));
  }

  function loadHistoryViewer() {
    return (historyViewerPromise ??= import("$lib/components/HistoryViewer.svelte"));
  }

  function loadTelemetryViewer() {
    return (telemetryViewerPromise ??= import("$lib/components/TelemetryViewer.svelte"));
  }

  function loadOperationLogViewer() {
    return (operationLogViewerPromise ??= import("$lib/components/OperationLogViewer.svelte"));
  }

  function loadHealthPanel() {
    return (healthPanelPromise ??= import("$lib/components/HealthPanel.svelte"));
  }

  function loadWorkspaceFeatureControls() {
    return (workspaceFeatureControlsPromise ??= import("$lib/components/workspace/WorkspaceFeatureControls.svelte"));
  }

  function currentTime(): number {
    return globalThis.performance?.now() ?? Date.now();
  }

  async function measure<T>(label: string, operation: () => Promise<T>): Promise<T> {
    const started = currentTime();
    try {
      return await operation();
    } finally {
      console.debug(`[workspace:${workspaceId || "unknown"}] ${label}`, {
        durationMs: Math.round((currentTime() - started) * 10) / 10,
      });
    }
  }

  function validWorkspaceTab(value: string | null): WorkspaceTab {
    const tab = workspaceTabValues.includes(value as WorkspaceTab) ? (value as WorkspaceTab) : "chat";
    if (tab === "actions" && !capabilities.actions) return "overview";
    if (tab === "logs" && !capabilities.operationLogs) return "overview";
    if (tab === "features" && !capabilities.workspaceFeatureControls) return "overview";
    return tab;
  }

  function validServiceSection(value: string | null): ServiceSection {
    return serviceSectionValues.includes(value as ServiceSection)
      ? (value as ServiceSection)
      : "service";
  }

  function syncNavigationFromUrl(url: URL) {
    const tab = url.searchParams.get("autostart") === "1" ? "overview" : validWorkspaceTab(url.searchParams.get("tab"));
    const section = validServiceSection(url.searchParams.get("section"));
    activeWorkspaceTab = tab;
    if (tab === "mcp") mcpSection = section;
    if (tab === "actions") actionsSection = section;
  }

  function navigateWorkspace(tab: WorkspaceTab, section?: ServiceSection) {
    activeWorkspaceTab = tab;
    const nextUrl = new URL($page.url);
    nextUrl.searchParams.set("tab", tab);

    if (tab === "mcp") {
      mcpSection = section ?? mcpSection;
      nextUrl.searchParams.set("section", mcpSection);
    } else if (tab === "actions") {
      actionsSection = section ?? actionsSection;
      nextUrl.searchParams.set("section", actionsSection);
    } else {
      nextUrl.searchParams.delete("section");
    }

    void goto(nextUrl, { replaceState: false, noScroll: true, keepFocus: true });
  }

  function stateLabel(state: RuntimeState): string {
    switch (state) {
      case "running":
        return $t("Running");
      case "starting":
        return $t("Starting");
      case "stopping":
        return $t("Stopping");
      case "error":
        return $t("Error");
      default:
        return $t("Stopped");
    }
  }

  function applyMcpRuntime(
    runtime: { state: RuntimeState; localEndpoint: string; publicEndpoint: string; localMessage?: string },
    id = workspaceId,
  ) {
    if (!id || id !== workspaceId) return;
    mcpStatus = runtime.state;
    mcpStatusMessage = runtime.localMessage ?? "";
    mcpLocal = runtime.localEndpoint;
    mcpPublic = runtime.publicEndpoint;
    mcpRuntimeStates.update((current) => ({ ...current, [id]: runtime.state }));
  }

  function applyActionsRuntime(
    runtime: { state: RuntimeState; localEndpoint: string; publicEndpoint: string; localMessage?: string },
    id = workspaceId,
  ) {
    if (!id || id !== workspaceId) return;
    actionsStatus = runtime.state;
    actionsStatusMessage = runtime.localMessage ?? "";
    actionsLocal = runtime.localEndpoint;
    actionsPublic = runtime.publicEndpoint;
    actionsRuntimeStates.update((current) => ({ ...current, [id]: runtime.state }));
  }

  function applyWorkspaceProfile(next: WorkspaceProfile) {
    profile = next;
    workspaces.update((items) => items.map((item) => (item.id === next.id ? next : item)));
  }

  async function persistProfile(label: string, next: WorkspaceProfile, id = workspaceId): Promise<boolean> {
    if (!id) return false;
    await measure(label, () => updateWorkspace(next));
    if (id !== workspaceId) return false;
    applyWorkspaceProfile(next);
    return true;
  }

  async function refreshMcpRuntime(id = workspaceId) {
    if (!id) return null;
    const runtime = await measure("runtime.mcp", () => getRuntimeStatus(id));
    applyMcpRuntime(runtime, id);
    return runtime;
  }

  async function refreshActionsRuntime(id = workspaceId) {
    if (!id) return null;
    const runtime = await measure("runtime.actions", () => getActionsRuntimeStatus(id));
    applyActionsRuntime(runtime, id);
    return runtime;
  }

  async function load(id = workspaceId) {
    if (!id) return;
    const generation = ++loadGeneration;
    const [items, profiles] = await measure("load.workspace-data", () =>
      Promise.all([
        listWorkspaces(),
        capabilities.frpManagement ? listFrpProfiles() : Promise.resolve([]),
      ]),
    );
    if (generation !== loadGeneration || id !== workspaceId) return;

    workspaces.set(items);
    frpProfiles = profiles;
    const nextProfile = items.find((item) => item.id === id) ?? null;
    profile = nextProfile;

    if (!nextProfile) {
      await goto(appUrl("/"));
      return;
    }

    const [mcpRuntime, actionsRuntime] = await Promise.all([
      measure("load.runtime-status", () => getRuntimeStatus(id)),
      capabilities.actions
        ? measure("load.actions-status", () => getActionsRuntimeStatus(id))
        : Promise.resolve(null),
      measure("load.last-workspace", () => setLastWorkspace(nextProfile.id)),
    ]);
    if (generation !== loadGeneration || id !== workspaceId) return;
    applyMcpRuntime(mcpRuntime, id);
    if (actionsRuntime) applyActionsRuntime(actionsRuntime, id);
  }

  async function refreshProfile(id = workspaceId): Promise<WorkspaceProfile | null> {
    if (!id) return null;
    const items = await measure("profile.refresh", () => listWorkspaces());
    if (id !== workspaceId) return null;
    workspaces.set(items);
    const nextProfile = items.find((item) => item.id === id) ?? null;
    profile = nextProfile;
    return nextProfile;
  }

  function tunnelConfigured(type: string | undefined): boolean {
    return type === "cloudflare" || type === "frp";
  }

  async function afterServiceStart(
    service: "mcp" | "actions",
    runtime: { state: RuntimeState; publicEndpoint: string },
    id: string,
  ) {
    const nextProfile = await refreshProfile(id);
    if (id !== workspaceId) return;
    const tunnelType =
      service === "mcp"
        ? nextProfile?.tunnel.type
        : nextProfile
          ? actionsConfig(nextProfile).tunnel_type
          : undefined;
    if (runtime.state === "running" && tunnelConfigured(tunnelType) && !runtime.publicEndpoint) {
      showToast(
        $t("The local service started, but the tunnel could not connect automatically. Check proxy and tunnel settings or review the logs."),
        { title: $t("Tunnel not connected"), kind: "warning", duration: 8000 },
      );
    }
  }

  async function toggleMcp() {
    const id = workspaceId;
    if (!id || mcpBusy) return;
    const wasRunning = mcpStatus === "running";
    mcpBusy = true;
    try {
      const runtime = await measure("service.mcp.toggle", () =>
        runServiceToggle(
          wasRunning,
          () => startRuntime(id),
          () => stopRuntime(id),
          "MCP",
        ),
      );
      if (runtime && id === workspaceId) {
        applyMcpRuntime(runtime, id);
        if (!wasRunning) {
          if (runtime.state === "running") await afterServiceStart("mcp", runtime, id);
          else notifyStartFailure("MCP", runtime);
        }
      }
    } finally {
      mcpBusy = false;
    }
  }

  async function toggleActions() {
    const id = workspaceId;
    if (!id || actionsBusy) return;
    const wasRunning = actionsStatus === "running";
    actionsBusy = true;
    try {
      const runtime = await measure("service.actions.toggle", () =>
        runServiceToggle(
          wasRunning,
          () => startActionsRuntime(id),
          () => stopActionsRuntime(id),
          "Actions",
        ),
      );
      if (runtime && id === workspaceId) {
        applyActionsRuntime(runtime, id);
        if (!wasRunning) {
          if (runtime.state === "running") await afterServiceStart("actions", runtime, id);
          else notifyStartFailure("Actions", runtime);
        }
      }
    } finally {
      actionsBusy = false;
    }
  }

  async function saveMcpPort(port: number) {
    if (!profile || profile.runtime.local_port === port) return;
    const next: WorkspaceProfile = {
      ...profile,
      runtime: { ...profile.runtime, local_port: port },
    };
    if (!(await persistProfile("save.mcp.port", next))) return;
    mcpLocal = mcpLocalEndpoint(port, next.runtime.bind_address);
  }

  async function saveMcpBindAddress(bindAddress: string) {
    if (!profile) return;
    const nextAddress = bindAddress.trim() || "127.0.0.1";
    if ((profile.runtime.bind_address ?? "127.0.0.1") === nextAddress) return;
    const next: WorkspaceProfile = {
      ...profile,
      runtime: { ...profile.runtime, bind_address: nextAddress },
    };
    if (!(await persistProfile("save.mcp.bind-address", next))) return;
    mcpLocal = mcpLocalEndpoint(next.runtime.local_port, nextAddress);
  }

  async function saveActionsPort(port: number) {
    if (!profile) return;
    const current = actionsConfig(profile);
    if (current.local_port === port) return;
    const next: WorkspaceProfile = {
      ...profile,
      actions: { ...current, local_port: port },
    };
    if (!(await persistProfile("save.actions.port", next))) return;
    actionsLocal = actionsLocalEndpoint(port, current.bind_address);
  }

  async function saveActionsBindAddress(bindAddress: string) {
    if (!profile) return;
    const current = actionsConfig(profile);
    const nextAddress = bindAddress.trim() || "127.0.0.1";
    if ((current.bind_address ?? "127.0.0.1") === nextAddress) return;
    const next: WorkspaceProfile = {
      ...profile,
      actions: { ...current, bind_address: nextAddress },
    };
    if (!(await persistProfile("save.actions.bind-address", next))) return;
    actionsLocal = actionsLocalEndpoint(current.local_port, nextAddress);
  }

  function publicEndpointFromTunnel(config: TunnelFormConfig, suffix: string): string {
    if ((config.type === "frp" || config.type === "builtin") && suffix === "/mcp" && config.public_url.trim()) {
      const url = config.public_url.trim().replace(/\/$/, "");
      return url.endsWith("/mcp") ? url : `${url}/mcp`;
    }
    const base = frpPublicUrl(
      config.type,
      config.frp_subdomain,
      config.frp_server,
      config.frp_profile_id,
      frpProfiles,
      config.public_url,
    );
    return base ? `${base.replace(/\/$/, "")}${suffix}` : "";
  }
  async function restartTunnelIfConfigured(
    targetWorkspaceId: string,
    config: TunnelFormConfig,
    service: "mcp" | "actions",
  ): Promise<string> {
    if (config.type === "none") {
      await measure(`tunnel.${service}.stop`, () => stopTunnel(targetWorkspaceId, service));
      return "";
    }
    const publicUrl =
      config.type === "builtin"
        ? await measure(`tunnel.${service}.enroll`, async () => {
            const result = await testTunnel(targetWorkspaceId, service);
            if (!result.success || !result.publicUrl) {
              throw new Error(result.message);
            }
            return result.publicUrl;
          })
        : await measure(`tunnel.${service}.restart`, async () => {
            const status = await restartTunnel(targetWorkspaceId, service);
            return status.publicUrl;
          });
    if (workspaceId !== targetWorkspaceId || !publicUrl) return "";
    const normalized = publicUrl.replace(/\/$/, "");
    if (service === "mcp") {
      return normalized.endsWith("/mcp") ? normalized : `${normalized}/mcp`;
    }
    return `${normalized}/openapi.json`;
  }

  async function saveMcpTunnel(config: TunnelFormConfig, options?: SaveTunnelOptions) {
    if (!profile || !workspaceId) return;
    const targetWorkspaceId = workspaceId;
    const next: WorkspaceProfile = {
      ...profile,
      tunnel: {
        ...profile.tunnel,
        type: config.type,
        public_url: config.public_url,
        frp_server: config.frp_server,
        frp_subdomain: config.frp_subdomain,
        frp_profile_id: config.frp_profile_id,
        frp_server_port: config.frp_server_port,
        cloudflare_mode: config.cloudflare_mode,
        use_proxy: config.use_proxy,
      },
    };
    if (!(await persistProfile("save.mcp.tunnel", next, targetWorkspaceId))) return;

    let restartedPublic = "";
    if (!options?.skipTunnelRestart) {
      restartedPublic = await restartTunnelIfConfigured(targetWorkspaceId, config, "mcp");
      await refreshProfile(targetWorkspaceId);
    }
    if (workspaceId !== targetWorkspaceId) return;
    mcpPublic = restartedPublic || publicEndpointFromTunnel(config, "/mcp");

    if (!options?.skipTunnelRestart && mcpStatus === "running") {
      await refreshMcpRuntime(targetWorkspaceId);
    }
    if (!options?.skipServicePrompt) {
      await promptServiceRestart(mcpStatus === "running", $t("MCP service"));
    }
  }

  async function saveActionsTunnel(config: TunnelFormConfig, options?: SaveTunnelOptions) {
    if (!profile || !workspaceId) return;
    const targetWorkspaceId = workspaceId;
    const current = actionsConfig(profile);
    const next: WorkspaceProfile = {
      ...profile,
      actions: {
        ...current,
        tunnel_type: config.type,
        public_url: config.public_url,
        frp_server: config.frp_server,
        frp_subdomain: config.frp_subdomain,
        frp_profile_id: config.frp_profile_id,
        frp_server_port: config.frp_server_port,
        cloudflare_mode: config.cloudflare_mode,
        use_proxy: config.use_proxy,
      },
    };
    if (!(await persistProfile("save.actions.tunnel", next, targetWorkspaceId))) return;

    let restartedPublic = "";
    if (!options?.skipTunnelRestart) {
      restartedPublic = await restartTunnelIfConfigured(targetWorkspaceId, config, "actions");
      await refreshProfile(targetWorkspaceId);
    }
    if (workspaceId !== targetWorkspaceId) return;
    actionsPublic = restartedPublic || publicEndpointFromTunnel(config, "/openapi.json");

    if (!options?.skipTunnelRestart && actionsStatus === "running") {
      await refreshActionsRuntime(targetWorkspaceId);
    }
    if (!options?.skipServicePrompt) {
      await promptServiceRestart(actionsStatus === "running", $t("Actions service"));
    }
  }

  async function saveMcpPolicy(draft: RuntimePolicyDraft) {
    if (!profile) return;
    const currentSecurityPolicy = securityPolicyConfig(profile.runtime);
    const requiresRestart =
      draft.transportMode !== profile.runtime.transport_mode ||
      JSON.stringify(draft.securityPolicy) !== JSON.stringify(currentSecurityPolicy) ||
      draft.blockingAdmissionLimit !== profile.runtime.blocking_admission_limit ||
      draft.processAdmissionLimit !== profile.runtime.process_admission_limit ||
      draft.globalBlockingAdmissionLimit !== profile.runtime.global_blocking_admission_limit ||
      draft.globalProcessAdmissionLimit !== profile.runtime.global_process_admission_limit ||
      draft.activeSessionLimit !== profile.runtime.active_session_limit;
    const next: WorkspaceProfile = {
      ...profile,
      runtime: {
        ...profile.runtime,
        transport_mode: draft.transportMode as "streamable-http" | "legacy-json",
        tool_profile: compatibilityToolProfile(draft.securityPolicy),
        permission_mode: compatibilityPermissionMode(draft.securityPolicy),
        security_policy: draft.securityPolicy,
        allowed_commands: draft.allowedCommands,
        workspace_local_entries: draft.workspaceLocalEntries,
        workspace_script_extensions: draft.workspaceScriptExtensions,
        blocking_admission_limit: draft.blockingAdmissionLimit,
        process_admission_limit: draft.processAdmissionLimit,
        global_blocking_admission_limit: draft.globalBlockingAdmissionLimit,
        global_process_admission_limit: draft.globalProcessAdmissionLimit,
        active_session_limit: draft.activeSessionLimit,
      },
    };
    if (!(await persistProfile("save.mcp.policy", next))) return;
    if (requiresRestart) {
      await promptServiceRestart(mcpStatus === "running", $t("MCP service"));
      await promptServiceRestart(actionsStatus === "running", $t("Actions service"));
    }
  }

  async function saveActionsPolicy(draft: ActionsPolicyDraft) {
    if (!profile) return;
    const current = actionsConfig(profile);
    const next: WorkspaceProfile = {
      ...profile,
      actions: {
        ...current,
        allowed_commands: draft.allowedCommands,
        max_patch_bytes: draft.maxPatchBytes,
        permission_mode: draft.permissionMode,
      },
    };
    if (!(await persistProfile("save.actions.policy", next))) return;
    await promptServiceRestart(actionsStatus === "running", $t("Actions service"));
  }

  async function saveMcpAuth(auth: AuthConfig, options?: { skipRuntimeRestart?: boolean }) {
    if (!profile || !workspaceId) return;
    const targetWorkspaceId = workspaceId;
    const next: WorkspaceProfile = { ...profile, auth };
    if (!(await persistProfile("save.mcp.auth", next, targetWorkspaceId))) return;
    if (!options?.skipRuntimeRestart && mcpStatus === "running") {
      try {
        const runtime = await measure("service.mcp.restart", () => restartRuntime(targetWorkspaceId));
        applyMcpRuntime(runtime, targetWorkspaceId);
      } catch {
        // The saved profile remains valid even if the service restart fails.
      }
    }
  }

  async function saveActionsAuth(draft: ActionsAuthDraft) {
    if (!profile || !workspaceId) return;
    const targetWorkspaceId = workspaceId;
    const current = actionsConfig(profile);
    const next: WorkspaceProfile = {
      ...profile,
      actions: {
        ...current,
        auth_type: draft.authType,
        oauth_client_id: draft.oauthClientId || current.oauth_client_id,
        oauth_scopes: draft.oauthScopes,
        use_shared_secrets: draft.useSharedSecrets,
      },
    };
    if (!(await persistProfile("save.actions.auth", next, targetWorkspaceId))) return;
    if (actionsStatus === "running") {
      try {
        const runtime = await measure("service.actions.restart", () =>
          restartActionsRuntime(targetWorkspaceId),
        );
        applyActionsRuntime(runtime, targetWorkspaceId);
      } catch {
        // The saved profile remains valid even if the service restart fails.
      }
    }
  }

  async function saveSandbox(config: SandboxConfig) {
    if (!profile || !workspaceId) return;
    const targetWorkspaceId = workspaceId;
    const current = sandboxConfig(profile.runtime);
    if (JSON.stringify(current) === JSON.stringify(config)) return;
    const disablingSandbox = current.enabled && !config.enabled;
    const next: WorkspaceProfile = {
      ...profile,
      runtime: {
        ...profile.runtime,
        sandbox: config,
      },
    };
    if (!(await persistProfile("save.workspace.sandbox", next, targetWorkspaceId))) return;
    if (!disablingSandbox) return;

    const wasRunning = mcpStatus === "running";
    mcpBusy = true;
    try {
      const runtime = await measure(
        wasRunning ? "service.mcp.restart.sandbox-disabled" : "service.mcp.start.sandbox-disabled",
        () => (wasRunning ? restartRuntime(targetWorkspaceId) : startRuntime(targetWorkspaceId)),
      );
      if (targetWorkspaceId !== workspaceId) return;
      applyMcpRuntime(runtime, targetWorkspaceId);
      if (runtime.state === "running") await afterServiceStart("mcp", runtime, targetWorkspaceId);
      else notifyStartFailure("MCP", runtime);
    } catch (error) {
      showToast(String(error), { kind: "error", title: $t("The service failed to start") });
    } finally {
      if (targetWorkspaceId === workspaceId) mcpBusy = false;
    }
  }

  async function saveWorkspaceName(name: string) {
    if (!profile || profile.name === name) return;
    await persistProfile("save.workspace.name", { ...profile, name });
  }

  async function handleWorkspaceFoldersChanged() {
    await promptServiceRestart(actionsStatus === "running", $t("Actions service"));
  }

  async function removeWorkspace() {
    if (!profile || !workspaceId) return;
    const confirmed = await confirm(
      $t("Delete workspace “{name}”? This action cannot be undone.", { name: profile.name }),
      {
        title: $t("Delete workspace"),
        kind: "warning",
        okLabel: $t("Delete"),
        cancelLabel: $t("Cancel"),
      },
    );
    if (!confirmed) return;
    await measure("workspace.delete", () => deleteWorkspace(workspaceId));
    workspaces.update((items) => items.filter((item) => item.id !== workspaceId));
    await goto(appUrl("/"));
  }

  $effect(() => {
    syncNavigationFromUrl($page.url);
  });

  $effect(() => {
    const id = workspaceId;
    if (!id) return;
    // Show the cached profile instantly; load() refreshes it in the background.
    profile = getStore(workspaces).find((item) => item.id === id) ?? null;
    void load(id);

    return () => {
      loadGeneration += 1;
    };
  });
</script>

{#if profile && ($uiMode === "auto" || activeWorkspaceTab === "chat")}
  <div class="workspace-chat-bar">
    <button class="workspace-chat-tab" class:active={activeWorkspaceTab === "chat"} onclick={() => navigateWorkspace("chat")}><MessageSquare size={15}/>{$t("chat.44")}</button>
    <div class="workspace-chat-actions">
      {#if activeWorkspaceTab === "chat" && capabilities.liveHistoryActivity}<button class="workspace-icon" class:active={$activityPanelOpen} title={$t('Task panel')} aria-label={$t('Task panel')} aria-expanded={$activityPanelOpen} onclick={() => activityPanelOpen.update(value => !value)}><ListChecks size={17}/></button>{/if}
      <button class="workspace-icon" class:active={activeWorkspaceTab !== "chat"} title={$t("chat.43")} aria-label={$t("chat.43")} onclick={() => navigateWorkspace("overview")}><Settings size={17}/></button>
    </div>
  </div>
{/if}
{#if profile && activeWorkspaceTab === "chat"}
  <div class="ad-host chat-workspace">
    <div class="chat-workspace-main">
      <ChatPanel externalNavigation connectRequested={$page.url.searchParams.get('connect') === '1'} requestedChatId={$page.url.searchParams.get('chat') ?? ''} requestedFolderId={$page.url.searchParams.get('folder') ?? ''} startNew={$page.url.searchParams.get('new') === '1'} onNavigate={(folder, chat) => void goto(appUrl(chatLocation(profile!.id, folder, chat)), {noScroll:true})} workspaceId={profile.id} folders={workspaceFolders(profile)} activeFolderId={profile.active_folder_id} endpoint={mcpPublic || mcpLocal} auth={profile.auth} />
    </div>
    {#if capabilities.liveHistoryActivity && $activityPanelOpen}
      {#key profile.id}<ActivityDrawer workspaceId={profile.id} live={mcpStatus === 'running'} expanded={$activityPanelExpanded} onToggleExpanded={() => activityPanelExpanded.update(value => !value)} onClose={() => activityPanelOpen.set(false)}/>{/key}
    {/if}
  </div>
{:else if profile && $uiMode === "auto"}
  {#key profile.id}
    <AutoWorkspaceView {profile} onProfileChange={applyWorkspaceProfile} />
  {/key}
{:else if profile && actions}
  <section class="page-scroll">
    <header class="page-header">
      <div class="flex items-start justify-between gap-4">
        <div>
          <p class="page-kicker">{$t("Workspaces")}</p>
          <h2 class="page-title">{profile.name}</h2>
        </div>
        {#if capabilities.workspaceLifecycle}
          <button
            type="button"
            class="tx-btn-ghost text-[var(--danger)]"
            onclick={() => void removeWorkspace()}
          >
            {$t("Delete workspace")}
          </button>
        {/if}
      </div>

      <div class="mt-5">
        <Tabs
          items={workspaceTabs}
          value={activeWorkspaceTab}
          idPrefix="workspace-tabs"
          ariaLabel={$t("Workspace features")}
          onchange={(value) => navigateWorkspace(value as WorkspaceTab)}
        />
      </div>
    </header>

    <div class="page-body">
      <div
        class="tx-tabpanel"
        role="tabpanel"
        id={`workspace-tabs-panel-${activeWorkspaceTab}`}
        aria-labelledby={`workspace-tabs-tab-${activeWorkspaceTab}`}
        tabindex="0"
      >
        {#if activeWorkspaceTab === "overview"}
          {#await loadOverviewPanel()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const WorkspaceOverview = module.default}
            <WorkspaceOverview
              {profile}
              {actions}
              {mcpStatus}
              {actionsStatus}
              {mcpBusy}
              {actionsBusy}
              {mcpLocal}
              {mcpPublic}
              {actionsLocal}
              {actionsPublic}
              {frpProfiles}
              {stateLabel}
              onToggleMcp={toggleMcp}
              onToggleActions={toggleActions}
              onNavigate={(tab) => navigateWorkspace(tab)}
            />
          {/await}
        {:else if activeWorkspaceTab === "history"}
          {#await loadHistoryViewer()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const HistoryViewer = module.default}
            <HistoryViewer
              workspaceId={workspaceId!}
              folders={workspaceFolders(profile)}
              activeFolderId={profile.active_folder_id}
            />
          {/await}
        {:else if activeWorkspaceTab === "telemetry"}
          {#await loadTelemetryViewer()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const TelemetryViewer = module.default}
            <TelemetryViewer workspaceId={workspaceId!} />
          {/await}
        {:else if activeWorkspaceTab === "logs"}
          {#await loadOperationLogViewer()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const OperationLogViewer = module.default}
            <OperationLogViewer
              workspaceId={workspaceId!}
              folders={workspaceFolders(profile)}
            />
          {/await}
        {:else if activeWorkspaceTab === "health"}
          {#await loadHealthPanel()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const HealthPanel = module.default}
            <HealthPanel workspaceId={workspaceId!} />
          {/await}
        {:else if activeWorkspaceTab === "features" && capabilities.workspaceFeatureControls}
          {#await loadWorkspaceFeatureControls()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const WorkspaceFeatureControls = module.default}
            <WorkspaceFeatureControls workspaceId={workspaceId!} />
          {/await}
        {:else if activeWorkspaceTab === "settings"}
          {#await loadSettingsPanel()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const WorkspaceSettings = module.default}
            <WorkspaceSettings
              {profile}
              onSaveName={saveWorkspaceName}
              onProfileChanged={applyWorkspaceProfile}
              onFoldersChanged={handleWorkspaceFoldersChanged}
              onSaveSandbox={saveSandbox}
              {sandboxLocked}
            />
          {:catch cause}
            <div class="tx-card p-5 text-sm text-[var(--color-danger)]">
              {$t("Workspace settings could not be loaded.")} {String(cause)}
            </div>
          {/await}
        {:else if activeWorkspaceTab === "mcp"}
          {#await loadMcpPanel()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const McpWorkspacePanel = module.default}
            <McpWorkspacePanel
              workspaceId={workspaceId!}
              {profile}
              section={mcpSection}
              status={mcpStatus}
              statusMessage={mcpStatusMessage}
              localEndpoint={mcpLocal}
              publicEndpoint={mcpPublic}
              busy={mcpBusy}
              {frpProfiles}
              onSectionChange={(section) => navigateWorkspace("mcp", section)}
              onToggle={toggleMcp}
              onPortChange={saveMcpPort}
              onBindAddressChange={saveMcpBindAddress}
              onSaveTunnel={saveMcpTunnel}
              onSavePolicy={saveMcpPolicy}
              onSaveAuth={saveMcpAuth}
            />
          {/await}
        {:else if activeWorkspaceTab === "actions" && capabilities.actions}
          {#await loadActionsPanel()}
            <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
          {:then module}
            {@const ActionsWorkspacePanel = module.default}
            <ActionsWorkspacePanel
              workspaceId={workspaceId!}
              {profile}
              section={actionsSection}
              status={actionsStatus}
              statusMessage={actionsStatusMessage}
              localEndpoint={actionsLocal}
              publicEndpoint={actionsPublic}
              busy={actionsBusy}
              {frpProfiles}
              onSectionChange={(section) => navigateWorkspace("actions", section)}
              onToggle={toggleActions}
              onPortChange={saveActionsPort}
              onBindAddressChange={saveActionsBindAddress}
              onSaveTunnel={saveActionsTunnel}
              onSavePolicy={saveActionsPolicy}
              onSaveAuth={saveActionsAuth}
            />
          {/await}
        {/if}
      </div>
    </div>

    <footer class="border-t border-[var(--color-border)] px-8 py-4 text-xs text-[var(--color-text-muted)]">
      {$t("MCP defaults to port 28766 and Actions to 8787. Both can run at the same time.")}
    </footer>
  </section>
{/if}

<style>
.workspace-chat-bar{display:flex;align-items:center;justify-content:space-between;flex:none;gap:12px;border-bottom:1px solid var(--color-border);padding:8px 18px}.workspace-chat-tab{display:flex;gap:7px;align-items:center;padding:6px 9px;border-radius:7px;font-size:12px;color:var(--color-text-muted);cursor:pointer}.workspace-chat-tab.active{color:var(--color-text);background:var(--surface-hover)}.workspace-chat-actions{display:flex;gap:6px}.workspace-icon{display:grid;place-items:center;width:30px;height:30px;border-radius:7px;color:var(--color-text-muted);cursor:pointer}.workspace-icon:hover,.workspace-icon.active{background:var(--surface-hover);color:var(--color-text)}button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}.chat-workspace-main{flex:1;min-width:0;min-height:0;padding:0}.chat-workspace :global(.ad-drawer){max-width:calc(100% - 20px)}@media(max-width:560px){.workspace-chat-bar{padding:6px 10px}.chat-workspace-main{padding:5px}}
</style>
