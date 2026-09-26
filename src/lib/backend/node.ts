import type { ActivitySnapshot } from "../api/activity";
import type { WorkspaceProfile } from "../types";
import { NODE_CAPABILITIES } from "./capabilities";
import { CapabilityError, UnimplementedError } from "./errors";
import {
  mapNodeWorkspace,
  overlayProfileOnConfig,
  runtimeStatusFromConfig,
  type NodeConfigSnapshot,
  type NodeConfigUpdatePayload,
  type NodeSafeConfig,
  type NodeWorkspaceSnapshot,
} from "./node-map";
import type {
  BooleanCapability,
  DirectoryBrowseResult,
  ExtensionInventoryPayload,
  ExtensionKind,
  ExtensionMasterToggleResult,
  ExtensionToggleResult,
  FrontendBackend,
  HealthItem,
  HistorySessionDetail,
  HistorySessionsResult,
  NativeUi,
  OperationLogPayload,
  SkillInventoryPayload,
  SkillMasterToggleResult,
  SkillToggleResult,
  TelemetryQueryOptions,
  TelemetryResult,
  TunnelStatus,
  TunnelTestResult,
} from "./types";

export type NodeRequestFn = <T>(
  route: string,
  init?: RequestInit,
  signal?: AbortSignal,
) => Promise<T>;

export interface NodeBackendDeps {
  request?: NodeRequestFn;
  native?: NativeUi;
  adminToken?: () => string;
}

interface NodeTelemetryPayload {
  scanned_lines?: number;
  matched_lines?: number;
  matched_async_session_events?: number;
  invalid_complete_lines?: number;
  records?: TelemetryResult["records"];
  aggregate?: TelemetryResult["aggregate"];
  performance?: TelemetryResult["performance"];
  warnings?: string[];
}

interface NodeHistoryListPayload {
  folder?: { id?: string; name?: string };
  sessions?: Array<{
    number: number;
    path?: string;
    title?: string;
    createdAt?: string | null;
    updatedAt?: string | null;
    status?: string;
    summary?: string;
    checkpointCount?: number;
  }>;
  integrity?: {
    missingNumbers?: number[];
    invalidFiles?: string[];
    emptyFiles?: string[];
  };
}

interface NodeHistoryDetailPayload {
  number: number;
  path?: string;
  title?: string;
  createdAt?: string | null;
  updatedAt?: string | null;
  status?: string;
  summary?: string;
  checkpointCount?: number;
  records?: HistorySessionDetail["records"];
  content?: string;
}

interface NodeHealthPayload {
  items?: Array<{
    label: string;
    ok: boolean;
    detail: string;
    hint?: string;
  }>;
}

interface NodeSecretResult {
  value?: string | null;
}

function notSupported(capability: BooleanCapability): () => Promise<never> {
  return async () => {
    throw new CapabilityError(capability);
  };
}

function notImplemented(method: string): () => Promise<never> {
  return async () => {
    throw new UnimplementedError(method);
  };
}

function createBrowserNative(): NativeUi {
  return {
    async pickDirectory() {
      throw new CapabilityError("nativeDirectoryPicker");
    },
    async confirm(message) {
      if (typeof globalThis.confirm === "function") {
        return globalThis.confirm(message);
      }
      throw new Error("No confirm dialog available");
    },
    async alert(message) {
      if (typeof globalThis.alert === "function") {
        globalThis.alert(message);
        return;
      }
      throw new Error("No alert dialog available");
    },
  };
}

function defaultAdminToken(): string {
  const token = document.querySelector<HTMLMetaElement>('meta[name="ctmcp-admin-token"]')?.content;
  if (!token) throw new Error("管理介面 token 遺失，請重新載入頁面。");
  return token;
}

function createDefaultRequest(adminToken: () => string): NodeRequestFn {
  return async (route, init = {}, signal) => {
    const headers = new Headers(init.headers);
    headers.set("x-ctmcp-admin-token", adminToken());
    if (init.body) headers.set("content-type", "application/json");
    const response = await fetch(route, {
      ...init,
      headers,
      signal,
      cache: "no-store",
      credentials: "same-origin",
    });
    const data = await response.json().catch(() => ({ error: { message: "伺服器回應格式錯誤。" } }));
    if (!response.ok) {
      const message = data?.error?.message ?? data?.error ?? response.statusText;
      throw new Error(String(message));
    }
    return data;
  };
}

function workspaceRoute(workspaceId: string, suffix: string): string {
  return `/admin/api/workspaces/${encodeURIComponent(workspaceId)}/${suffix}`;
}

function mapTunnelStatus(status: Partial<TunnelStatus> | null | undefined): TunnelStatus {
  return {
    state: status?.state ?? "stopped",
    publicUrl: status?.publicUrl ?? "",
    tunnelPid: status?.tunnelPid ?? null,
    configuredWorkers: status?.configuredWorkers ?? null,
    connectedWorkers: status?.connectedWorkers ?? null,
    idleWorkers: status?.idleWorkers ?? null,
    busyWorkers: status?.busyWorkers ?? null,
    recycledWorkers: status?.recycledWorkers ?? null,
    policyRevision: status?.policyRevision ?? null,
    lastError: status?.lastError ?? null,
  };
}

function assertMcpTunnel(service: string): void {
  if (service === "actions") throw new CapabilityError("actions");
}

function mapTelemetry(workspaceId: string, payload: NodeTelemetryPayload): TelemetryResult {
  return {
    workspace_id: workspaceId,
    log_dir: "",
    scanned_lines: payload.scanned_lines ?? 0,
    matched_lines: payload.matched_lines ?? 0,
    matched_async_session_events: payload.matched_async_session_events ?? 0,
    invalid_complete_lines: payload.invalid_complete_lines ?? 0,
    records: payload.records ?? [],
    aggregate: payload.aggregate ?? null,
    performance: payload.performance ?? null,
    warnings: payload.warnings ?? [],
  };
}

function mapHistoryList(payload: NodeHistoryListPayload): HistorySessionsResult {
  const sessions = (payload.sessions ?? []).map((session) => ({
    number: session.number,
    path: session.path ?? "",
    title: session.title ?? "",
    sessionKey: null,
    createdAt: session.createdAt ?? null,
    updatedAt: session.updatedAt ?? null,
    status: session.status ?? "",
    activityStatus: "completed" as const,
    activityTool: null,
    activityDescription: null,
    lastActivityAtMs: null,
    activeRequestCount: 0,
    lastActivityOutcome: null,
    summary: session.summary ?? "",
    checkpointCount: session.checkpointCount ?? 0,
  }));
  return {
    historyDir: payload.folder?.name ?? "",
    sessions,
    count: sessions.length,
    missingNumbers: payload.integrity?.missingNumbers ?? [],
    invalidFiles: payload.integrity?.invalidFiles ?? [],
    emptyFiles: payload.integrity?.emptyFiles ?? [],
  };
}

function mapHistoryDetail(payload: NodeHistoryDetailPayload): HistorySessionDetail {
  return {
    number: payload.number,
    path: payload.path ?? "",
    title: payload.title ?? "",
    sessionKey: null,
    createdAt: payload.createdAt ?? null,
    updatedAt: payload.updatedAt ?? null,
    status: payload.status ?? "",
    activityStatus: "completed",
    activityTool: null,
    activityDescription: null,
    lastActivityAtMs: null,
    activeRequestCount: 0,
    lastActivityOutcome: null,
    summary: payload.summary ?? "",
    checkpointCount: payload.checkpointCount ?? 0,
    records: payload.records ?? [],
    content: payload.content ?? "",
  };
}

function mapHealth(payload: NodeHealthPayload): HealthItem[] {
  return (payload.items ?? []).map((item) => ({
    label: item.label,
    ok: item.ok,
    detail: item.detail,
    hint: item.hint ?? "",
  }));
}

export function createNodeBackend(deps: NodeBackendDeps = {}): FrontendBackend {
  const request = deps.request ?? createDefaultRequest(deps.adminToken ?? defaultAdminToken);
  const native = deps.native ?? createBrowserNative();
  const extras = new Map<string, NodeSafeConfig>();
  let snapshot: NodeConfigSnapshot | null = null;

  async function loadConfig(signal?: AbortSignal): Promise<NodeConfigSnapshot> {
    snapshot = await request<NodeConfigSnapshot>("/admin/api/config", {}, signal);
    for (const workspace of snapshot.workspaces) {
      extras.set(workspace.id, workspace.saved ?? workspace.effective);
    }
    return snapshot;
  }

  async function savedConfig(id: string): Promise<NodeSafeConfig> {
    const cached = extras.get(id);
    if (cached) return cached;
    const config = await loadConfig();
    const workspace = config.workspaces.find((item) => item.id === id);
    if (!workspace) throw new Error(`Workspace ${id} was not found.`);
    return workspace.saved ?? workspace.effective;
  }

  async function saveAndRefresh(
    id: string,
    payload: NodeConfigUpdatePayload,
  ): Promise<WorkspaceProfile> {
    await request(workspaceRoute(id, "config"), {
      method: "PUT",
      body: JSON.stringify(payload),
    });
    const config = await loadConfig();
    const workspace = config.workspaces.find((item) => item.id === id);
    if (!workspace) throw new Error(`Workspace ${id} was not found.`);
    return mapNodeWorkspace(workspace);
  }

  function workspaceSnapshot(id: string): NodeWorkspaceSnapshot | undefined {
    return snapshot?.workspaces.find((item) => item.id === id);
  }

  return {
    capabilities: NODE_CAPABILITIES,
    native,

    workspaces: {
      async list() {
        const config = await loadConfig();
        return config.workspaces.map(mapNodeWorkspace);
      },
      async create(folderPath, name) {
        const created = await request<{ id: string }>("/admin/api/workspaces", {
          method: "POST",
          body: JSON.stringify({ path: folderPath, name }),
        });
        const config = await loadConfig();
        const workspace = config.workspaces.find((item) => item.id === created.id);
        if (!workspace) throw new Error("Created workspace was not found.");
        return mapNodeWorkspace(workspace);
      },
      listWslDistributions: notSupported("wslFolders"),
      async listSandboxBackends() {
        const config = snapshot ?? (await loadConfig());
        return config.workspaces[0]?.effective.sandboxBackends ?? [];
      },
      async update(profile) {
        const saved = await savedConfig(profile.id);
        await saveAndRefresh(profile.id, overlayProfileOnConfig(saved, profile));
      },
      async addFolder(id, path, name) {
        const saved = await savedConfig(id);
        const current = workspaceSnapshot(id) ?? { id, name: saved.folders[0]?.name ?? id, effective: saved, saved };
        return saveAndRefresh(id, {
          ...overlayProfileOnConfig(saved, mapNodeWorkspace(current)),
          folders: [
            ...saved.folders.map((folder) => ({ id: folder.id, name: folder.name, path: folder.path })),
            { path, name },
          ],
        });
      },
      addWslFolder: notSupported("wslFolders"),
      async removeFolder(id, folderId) {
        const saved = await savedConfig(id);
        const folders = saved.folders
          .filter((folder) => folder.id !== folderId)
          .map((folder) => ({ id: folder.id, name: folder.name, path: folder.path }));
        if (folders.length === 0) throw new Error("A workspace must keep at least one folder");
        const profile = mapNodeWorkspace({
          id,
          name: workspaceSnapshot(id)?.name ?? id,
          effective: { ...saved, folders: saved.folders.filter((folder) => folder.id !== folderId) },
          saved,
        });
        return saveAndRefresh(id, { ...overlayProfileOnConfig(saved, profile), folders });
      },
      async openDirectory(folderPath) {
        await request("/admin/api/directories/open", {
          method: "POST",
          body: JSON.stringify({ path: folderPath }),
        });
      },
      async delete(id) {
        await request(`/admin/api/workspaces/${encodeURIComponent(id)}`, { method: "DELETE" });
      },
      startRuntime: notSupported("runtimeSupervisor"),
      stopRuntime: notSupported("runtimeSupervisor"),
      async getRuntimeStatus(id) {
        const saved = await savedConfig(id);
        return runtimeStatusFromConfig(saved);
      },
      startActionsRuntime: notSupported("actions"),
      stopActionsRuntime: notSupported("actions"),
      getActionsRuntimeStatus: notSupported("actions"),
      restartRuntime: notSupported("runtimeSupervisor"),
      restartActionsRuntime: notSupported("actions"),
    },

    settings: {
      listFrpProfiles: notSupported("frpManagement"),
      saveFrpProfile: notSupported("frpManagement"),
      deleteFrpProfile: notSupported("frpManagement"),
      async getLastWorkspaceId() {
        const config = snapshot ?? (await loadConfig());
        return config.primaryWorkspaceId ?? config.workspaces[0]?.id ?? "";
      },
      async setLastWorkspace() {},
      getProxy: notImplemented("settings.getProxy"),
      setProxy: notImplemented("settings.setProxy"),
    },

    telemetry: {
      async query(workspaceId, options: TelemetryQueryOptions = {}, signal) {
        const query = new URLSearchParams({
          scope: options.scope ?? "all",
          errorsOnly: String(options.errorsOnly ?? false),
          limit: String(options.limit ?? 100),
          minDurationMs: String(options.minDurationMs ?? 0),
          sortBy: options.sortBy ?? "calls",
        });
        const payload = await request<NodeTelemetryPayload>(
          `${workspaceRoute(workspaceId, "telemetry")}?${query}`,
          {},
          signal,
        );
        return mapTelemetry(workspaceId, payload);
      },
    },

    // The Node agent has no in-process live activity feed yet.
    activity: {
      async read(): Promise<ActivitySnapshot> {
        return {
          events: [],
          rev: 0,
          reset: true,
          stats: {
            total: 0,
            success: 0,
            errors: 0,
            running: 0,
            totalDurationMs: 0,
            avgDurationMs: 0,
            p95DurationMs: 0,
            bytesIn: 0,
            bytesOut: 0,
            reads: 0,
            edits: 0,
            execs: 0,
            searches: 0,
            filesChanged: 0,
            filesViewed: 0,
            bytesAdded: 0,
            bytesRemoved: 0,
            linesAdded: 0,
            linesRemoved: 0,
            firstMs: 0,
            lastMs: 0,
          },
        };
      },
      async detail() {
        return null;
      },
      async clear() {},
    },

    history: {
      async list(workspaceId, folderId, signal) {
        if (!folderId) throw new Error("history.list requires folderId on the Node backend");
        const query = new URLSearchParams({ folderId });
        const payload = await request<NodeHistoryListPayload>(
          `${workspaceRoute(workspaceId, "history")}?${query}`,
          {},
          signal,
        );
        return mapHistoryList(payload);
      },
      async read(workspaceId, number, folderId, signal) {
        if (!folderId) throw new Error("history.read requires folderId on the Node backend");
        const query = new URLSearchParams({ folderId });
        const payload = await request<NodeHistoryDetailPayload>(
          `${workspaceRoute(workspaceId, `history/${number}`)}?${query}`,
          {},
          signal,
        );
        return mapHistoryDetail(payload);
      },
    },

    health: {
      async run(workspaceId, signal) {
        const payload = await request<NodeHealthPayload>(
          workspaceRoute(workspaceId, "health"),
          { method: "POST" },
          signal,
        );
        return mapHealth(payload);
      },
    },

    logs: {
      readRaw: notSupported("rawRuntimeLogs"),
    },

    secrets: {
      async getWorkspaceSecret(id, key) {
        if (key !== "oauth_password") throw new UnimplementedError("secrets.getWorkspaceSecret");
        const payload = await request<NodeSecretResult>(workspaceRoute(id, "secrets/oauth-password"));
        return payload.value ?? null;
      },
      async setWorkspaceSecret(id, key, value) {
        const route = key === "oauth_password"
          ? "secrets/oauth-password"
          : key === "builtin_tunnel_enrollment_url"
            ? "secrets/builtin-tunnel-enrollment-url"
            : null;
        if (!route) throw new UnimplementedError("secrets.setWorkspaceSecret");
        await request(workspaceRoute(id, route), {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ value }),
        });
      },
      async regenerateWorkspaceSecret(id, key) {
        if (key !== "oauth_password") throw new UnimplementedError("secrets.regenerateWorkspaceSecret");
        const payload = await request<NodeSecretResult>(
          workspaceRoute(id, "secrets/oauth-password/regenerate"),
          { method: "POST" },
        );
        return payload.value ?? "";
      },
      getSharedSecret: notSupported("sharedSecretStore"),
      setSharedSecret: notSupported("sharedSecretStore"),
      regenerateSharedSecret: notSupported("sharedSecretStore"),
    },

    software: {
      list: notSupported("softwareManagement"),
      install: notSupported("softwareManagement"),
      uninstall: notSupported("softwareManagement"),
      getDownloadConfig: notSupported("softwareManagement"),
      setDownloadConfig: notSupported("softwareManagement"),
    },

    tunnel: {
      getFrpSnippet: notSupported("frpManagement"),
      async start(id, service) {
        assertMcpTunnel(service);
        return mapTunnelStatus(
          await request<TunnelStatus>(workspaceRoute(id, "tunnel/start"), {
            method: "POST",
            body: JSON.stringify({ service }),
          }),
        );
      },
      async stop(id, service) {
        assertMcpTunnel(service);
        return mapTunnelStatus(
          await request<TunnelStatus>(workspaceRoute(id, "tunnel/stop"), {
            method: "POST",
            body: JSON.stringify({ service }),
          }),
        );
      },
      async test(id, service) {
        assertMcpTunnel(service);
        return request<TunnelTestResult>(workspaceRoute(id, "tunnel/test"), {
          method: "POST",
          body: JSON.stringify({ service }),
        });
      },
      async restart(id, service) {
        assertMcpTunnel(service);
        return mapTunnelStatus(
          await request<TunnelStatus>(workspaceRoute(id, "tunnel/restart"), {
            method: "POST",
            body: JSON.stringify({ service }),
          }),
        );
      },
    },

    agent: {
      restart: () => request("/admin/api/restart", { method: "POST" }),
      status: (signal) => request("/admin/api/status", {}, signal),
      loadConfig: (signal) => loadConfig(signal),
      saveConfig: (workspaceId, payload) =>
        request(workspaceRoute(workspaceId, "config"), {
          method: "PUT",
          body: JSON.stringify(payload),
        }),
    },

    directories: {
      async browse(path, workspaceId, signal) {
        const query = new URLSearchParams();
        if (path) query.set("path", path);
        if (workspaceId) query.set("workspaceId", workspaceId);
        const suffix = query.size ? `?${query}` : "";
        return request<DirectoryBrowseResult>(`/admin/api/directories${suffix}`, {}, signal);
      },
    },

    operations: {
      query(workspaceId, filters, cursor = 0, signal) {
        const query = new URLSearchParams({
          folderId: filters.folderId,
          status: filters.status,
          tool: filters.tool,
          errorsOnly: String(filters.errorsOnly),
          limit: String(filters.limit),
          cursor: String(cursor),
        });
        return request<OperationLogPayload>(`${workspaceRoute(workspaceId, "logs")}?${query}`, {}, signal);
      },
    },

    workspaceFeatures: {
      skills(workspaceId, signal) {
        return request<SkillInventoryPayload>(workspaceRoute(workspaceId, "skills"), {}, signal);
      },
      setSkillsActive(workspaceId, active) {
        return request<SkillMasterToggleResult>(workspaceRoute(workspaceId, "skills"), {
          method: "PUT",
          body: JSON.stringify({ active }),
        });
      },
      setSkillEnabled(workspaceId, key, enabled) {
        return request<SkillToggleResult>(workspaceRoute(workspaceId, "skills"), {
          method: "PUT",
          body: JSON.stringify({ key, enabled }),
        });
      },
      extensions(workspaceId, signal) {
        return request<ExtensionInventoryPayload>(workspaceRoute(workspaceId, "extensions"), {}, signal);
      },
      setExtensionActive(workspaceId, kind: ExtensionKind, active) {
        return request<ExtensionMasterToggleResult>(workspaceRoute(workspaceId, "extensions"), {
          method: "PUT",
          body: JSON.stringify({ kind, active }),
        });
      },
      setExtensionEnabled(workspaceId, kind: ExtensionKind, key, enabled) {
        return request<ExtensionToggleResult>(workspaceRoute(workspaceId, "extensions"), {
          method: "PUT",
          body: JSON.stringify({ kind, key, enabled }),
        });
      },
    },
  };
}
