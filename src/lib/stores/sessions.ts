import { get, writable } from "svelte/store";
import { installSoftware, listSoftware } from "$lib/api/software";
import { getWorkspaceSecret } from "$lib/api/secrets";
import { startTunnel } from "$lib/api/tunnel";
import { getRuntimeStatus, listWorkspaces, startRuntime, stopRuntime, updateWorkspace } from "$lib/api/workspaces";
import { getBackend } from "$lib/backend";
import { t, type MessageKey } from "$lib/i18n";
import { isPortConflictError, serviceErrorMessage } from "$lib/runtime/service";
import { mcpRuntimeStates, workspaces } from "$lib/stores/app";
import type { RuntimeStatus, WorkspaceProfile } from "$lib/types";

/**
 * Per-workspace connection sessions for Simple mode.
 *
 * Start/stop flows live here (not in a page component) so they keep running
 * when the user switches to another workspace, several workspaces can be
 * online at once, and switching back shows the real state immediately.
 */
export interface SessionState {
  status: RuntimeStatus | null;
  working: boolean;
  phase: MessageKey | null;
  error: string;
}

const EMPTY: SessionState = { status: null, working: false, phase: null, error: "" };

export const sessions = writable<Record<string, SessionState>>({});

export function sessionOf(all: Record<string, SessionState>, id: string): SessionState {
  return all[id] ?? EMPTY;
}

function patch(id: string, next: Partial<SessionState>) {
  sessions.update((all) => ({ ...all, [id]: { ...sessionOf(all, id), ...next } }));
  if (next.status) {
    const state = next.status.state;
    mcpRuntimeStates.update((current) => (current[id] === state ? current : { ...current, [id]: state }));
  }
}

function sleep(ms: number) {
  return new Promise<void>((resolve) => setTimeout(resolve, ms));
}

export function messageOf(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error ?? "");
}

function tr(key: MessageKey): string {
  return get(t)(key);
}

/** A public endpoint the internet can actually reach (placeholders are http://127.0.0.1…). */
export function isReachablePublic(url: string | undefined | null): boolean {
  return Boolean(url && /^https:\/\//i.test(url));
}

/**
 * New workspaces default to a "builtin" relay tunnel with a local placeholder
 * URL. That only works after enrolling a relay in Advanced mode.
 */
export function tunnelUsable(profile: WorkspaceProfile): boolean {
  const type = profile.tunnel.type;
  if (type === "cloudflare" || type === "frp") return true;
  if (type === "builtin") return isReachablePublic(profile.tunnel.public_url);
  return false;
}

async function refreshWorkspaceList(id: string): Promise<WorkspaceProfile | null> {
  const items = await listWorkspaces();
  workspaces.set(items);
  return items.find((item) => item.id === id) ?? null;
}

export async function refreshSession(id: string): Promise<void> {
  try {
    patch(id, { status: await getRuntimeStatus(id) });
  } catch {
    // Keep the last known status; the next poll retries.
  }
}

async function ensureTunnelReady(current: WorkspaceProfile): Promise<WorkspaceProfile> {
  const canInstall = getBackend().capabilities.softwareManagement;
  let next = current;
  if (!tunnelUsable(next)) {
    if (!canInstall) return next;
    next = {
      ...next,
      tunnel: {
        ...next.tunnel,
        type: "cloudflare",
        cloudflare_mode: "quick",
        public_url: "",
        use_proxy: next.tunnel.use_proxy ?? true,
      },
    };
    await updateWorkspace(next);
    next = (await refreshWorkspaceList(next.id)) ?? next;
  }
  if (next.tunnel.type === "cloudflare" && canInstall) {
    const software = await listSoftware();
    const cloudflared = software.find((item) => item.kind === "cloudflared");
    if (!cloudflared?.installed) await installSoftware("cloudflared");
  }
  if (next.tunnel.type === "cloudflare" && next.tunnel.cloudflare_mode === "named") {
    const token = (await getWorkspaceSecret(next.id, "cloudflare_token").catch(() => null))?.trim();
    if (!token || !isReachablePublic(next.tunnel.public_url)) {
      throw new Error(tr("Enter the Tunnel Token and the domain first."));
    }
  }
  return next;
}

async function startService(id: string): Promise<RuntimeStatus> {
  const current = await getRuntimeStatus(id);
  if (current.state === "running") return current;
  try {
    return await startRuntime(id);
  } catch (error) {
    if (!isPortConflictError(error)) throw error;
    // The previous listener may still be draining; give it a moment.
    await sleep(3000);
    const retry = await getRuntimeStatus(id);
    if (retry.state === "running") return retry;
    return await startRuntime(id);
  }
}

async function waitForPublic(id: string, timeoutMs: number): Promise<RuntimeStatus> {
  const deadline = Date.now() + timeoutMs;
  let latest = await getRuntimeStatus(id);
  patch(id, { status: latest });
  while (!isReachablePublic(latest.publicEndpoint) && Date.now() < deadline) {
    await sleep(1500);
    latest = await getRuntimeStatus(id);
    patch(id, { status: latest });
  }
  return latest;
}

export async function turnOn(id: string): Promise<void> {
  if (sessionOf(get(sessions), id).working) return;
  patch(id, { working: true, phase: "Preparing the tunnel…", error: "" });
  try {
    const profile = get(workspaces).find((item) => item.id === id) ?? (await refreshWorkspaceList(id));
    if (!profile) throw new Error("workspace not found");
    const prepared = await ensureTunnelReady(profile);

    patch(id, { phase: "Starting the service…" });
    const started = await startService(id);
    patch(id, { status: started });
    if (started.state !== "running") throw new Error(serviceErrorMessage(started));

    if (tunnelUsable(prepared)) {
      patch(id, { phase: "Connecting the public tunnel…" });
      let latest = await waitForPublic(id, isReachablePublic(started.publicEndpoint) ? 0 : 3000);
      let tunnelError = "";
      if (!isReachablePublic(latest.publicEndpoint)) {
        await startTunnel(id, "mcp").catch((error: unknown) => {
          tunnelError = messageOf(error);
        });
        latest = await waitForPublic(id, tunnelError ? 2000 : 30000);
      }
      if (!isReachablePublic(latest.publicEndpoint)) {
        const base = tr(
          "The MCP service is running, but the public tunnel did not connect. Check your network or proxy, then try again.",
        );
        throw new Error(tunnelError ? `${base}\n\n${tunnelError}` : base);
      }
    }
    await refreshWorkspaceList(id);
  } catch (error) {
    patch(id, { error: messageOf(error) });
  } finally {
    patch(id, { working: false, phase: null });
    await refreshSession(id);
  }
}

export async function turnOff(id: string): Promise<void> {
  if (sessionOf(get(sessions), id).working) return;
  patch(id, { working: true, phase: "Stopping…", error: "" });
  try {
    patch(id, { status: await stopRuntime(id) });
  } catch (error) {
    patch(id, { error: messageOf(error) });
  } finally {
    patch(id, { working: false, phase: null });
  }
}

export async function restartIfRunning(id: string): Promise<void> {
  if (sessionOf(get(sessions), id).status?.state !== "running") return;
  await turnOff(id);
  await turnOn(id);
}

export function clearError(id: string) {
  patch(id, { error: "" });
}

let poller: ReturnType<typeof setInterval> | undefined;

/** One lightweight poll for every workspace (paused while the window is hidden). */
export function startSessionPolling(intervalMs = 5000): () => void {
  if (poller) return () => undefined;
  const tick = () => {
    if (typeof document !== "undefined" && document.hidden) return;
    const all = get(sessions);
    for (const item of get(workspaces)) {
      if (!sessionOf(all, item.id).working) void refreshSession(item.id);
    }
  };
  tick();
  poller = setInterval(tick, intervalMs);
  return () => {
    clearInterval(poller);
    poller = undefined;
  };
}
