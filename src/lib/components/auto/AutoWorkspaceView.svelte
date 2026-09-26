<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { page } from "$app/stores";
  import { replaceState } from "$app/navigation";
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import Folder from "@lucide/svelte/icons/folder";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import { getWorkspaceSecret, setWorkspaceSecret } from "$lib/api/secrets";
  import { installSoftware, listSoftware } from "$lib/api/software";
  import { startTunnel } from "$lib/api/tunnel";
  import {
    getRuntimeStatus,
    listWorkspaces,
    startRuntime,
    stopRuntime,
    updateWorkspace,
  } from "$lib/api/workspaces";
  import { getBackend, loadMcpAuthSecrets } from "$lib/backend";
  import { buildConnectionPrompt, isTemporaryEndpoint, type ConnectionInfo } from "$lib/connect/prompt";
  import { locale, t, type MessageKey } from "$lib/i18n";
  import { isPortConflictError, serviceErrorMessage } from "$lib/runtime/service";
  import { mcpRuntimeStates, workspaces } from "$lib/stores/app";
  import { showToast } from "$lib/stores/toast";
  import { uiMode } from "$lib/stores/ui-mode";
  import { workspaceFolders, type RuntimeStatus, type WorkspaceProfile } from "$lib/types";

  interface Props {
    profile: WorkspaceProfile;
    onProfileChange?: (profile: WorkspaceProfile) => void;
  }

  let { profile, onProfileChange }: Props = $props();

  type TunnelKind = "quick" | "named" | "other";

  const capabilities = getBackend().capabilities;
  const canControl = capabilities.runtimeSupervisor;
  const canInstallTunnel = capabilities.softwareManagement;

  let status = $state<RuntimeStatus | null>(null);
  let working = $state(false);
  let phase = $state<MessageKey | null>(null);
  let errorMessage = $state("");
  let secrets = $state({ oauth_client_id: "", oauth_password: "", bearer_token: "" });
  let copiedKey = $state<string | null>(null);
  let modeView = $state<TunnelKind | null>(null);
  let namedToken = $state("");
  let namedDomain = $state("");
  let namedTokenSaved = $state(false);
  let savingNamed = $state(false);
  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;

  const running = $derived(status?.state === "running");
  const publicEndpoint = $derived(running ? (status?.publicEndpoint ?? "") : "");
  const localEndpoint = $derived(status?.localEndpoint || `http://127.0.0.1:${profile.runtime?.local_port ?? ""}/mcp`);
  const endpoint = $derived(publicEndpoint || (running ? localEndpoint : ""));
  const hasPublic = $derived(Boolean(publicEndpoint));
  const folders = $derived(workspaceFolders(profile).map((folder) => folder.path));
  const authType = $derived(profile.auth.type);
  const configuredKind = $derived<TunnelKind>(
    profile.tunnel.type === "cloudflare"
      ? profile.tunnel.cloudflare_mode === "named"
        ? "named"
        : "quick"
      : hasTunnel(profile.tunnel.type)
        ? "other"
        : "quick",
  );
  const kind = $derived<TunnelKind>(modeView ?? configuredKind);
  const info = $derived<ConnectionInfo>({
    workspaceName: profile.name,
    endpoint,
    authType,
    clientId: secrets.oauth_client_id || profile.auth.oauth_client_id,
    password: secrets.oauth_password,
    bearerToken: secrets.bearer_token,
    folders,
  });
  const prompt = $derived(endpoint ? buildConnectionPrompt(info, $locale) : "");
  const switchOn = $derived(working ? phase !== "Stopping…" : running);

  function hasTunnel(type: string | undefined): boolean {
    return type === "cloudflare" || type === "frp" || type === "builtin";
  }

  function sleep(ms: number) {
    return new Promise<void>((resolve) => setTimeout(resolve, ms));
  }

  function messageOf(error: unknown): string {
    if (error instanceof Error) return error.message;
    if (typeof error === "object" && error && "message" in error) {
      return String((error as { message: unknown }).message);
    }
    return String(error ?? "");
  }

  function setMcpState(next: RuntimeStatus) {
    status = next;
    mcpRuntimeStates.update((current) => ({ ...current, [profile.id]: next.state }));
  }

  async function refreshStatus() {
    try {
      setMcpState(await getRuntimeStatus(profile.id));
    } catch {
      // Keep the last known status; the next poll will retry.
    }
  }

  async function refreshSecrets() {
    try {
      const loaded = await loadMcpAuthSecrets(getBackend(), profile.id, profile.auth);
      secrets = {
        oauth_client_id: loaded.oauth_client_id ?? "",
        oauth_password: loaded.oauth_password ?? "",
        bearer_token: loaded.bearer_token ?? "",
      };
    } catch {
      // Secrets are optional for display; the advanced view shows detailed errors.
    }
  }

  async function refreshNamedState() {
    namedDomain = profile.tunnel.cloudflare_mode === "named" ? profile.tunnel.public_url : namedDomain;
    try {
      namedTokenSaved = Boolean((await getWorkspaceSecret(profile.id, "cloudflare_token"))?.trim());
    } catch {
      namedTokenSaved = false;
    }
  }

  async function refreshProfile(): Promise<WorkspaceProfile> {
    const items = await listWorkspaces();
    workspaces.set(items);
    const next = items.find((item) => item.id === profile.id) ?? profile;
    onProfileChange?.(next);
    return next;
  }

  async function refreshAll() {
    await Promise.all([refreshStatus(), refreshSecrets()]);
  }

  async function ensureTunnelReady(current: WorkspaceProfile): Promise<WorkspaceProfile> {
    let next = current;
    if (!hasTunnel(next.tunnel.type)) {
      if (!canInstallTunnel) return next;
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
      next = await refreshProfile();
    }
    if (next.tunnel.type === "cloudflare" && canInstallTunnel) {
      const software = await listSoftware();
      const cloudflared = software.find((item) => item.kind === "cloudflared");
      if (!cloudflared?.installed) await installSoftware("cloudflared");
    }
    return next;
  }

  async function startService(): Promise<RuntimeStatus> {
    const current = await getRuntimeStatus(profile.id);
    if (current.state === "running") return current;
    try {
      return await startRuntime(profile.id);
    } catch (error) {
      if (!isPortConflictError(error)) throw error;
      // The previous listener of this app may still be draining; give it a moment.
      await sleep(3000);
      const retry = await getRuntimeStatus(profile.id);
      if (retry.state === "running") return retry;
      return await startRuntime(profile.id);
    }
  }

  async function waitForPublicEndpoint(timeoutMs: number): Promise<RuntimeStatus> {
    const deadline = Date.now() + timeoutMs;
    let latest = await getRuntimeStatus(profile.id);
    while (!latest.publicEndpoint && Date.now() < deadline && !destroyed) {
      await sleep(1500);
      latest = await getRuntimeStatus(profile.id);
    }
    return latest;
  }

  async function turnOn() {
    if (!canControl || working) return;
    working = true;
    errorMessage = "";
    try {
      phase = "Preparing the tunnel…";
      const prepared = await ensureTunnelReady(profile);
      if (prepared.tunnel.type === "cloudflare" && prepared.tunnel.cloudflare_mode === "named") {
        if (!prepared.tunnel.public_url || !namedTokenSaved) {
          throw new Error($t("Enter the Tunnel Token and the domain first."));
        }
      }

      phase = "Starting the service…";
      const started = await startService();
      setMcpState(started);
      if (started.state !== "running") throw new Error(serviceErrorMessage(started));

      if (hasTunnel(prepared.tunnel.type)) {
        phase = "Connecting the public tunnel…";
        let latest = await waitForPublicEndpoint(started.publicEndpoint ? 0 : 6000);
        let tunnelError = "";
        if (!latest.publicEndpoint) {
          await startTunnel(profile.id, "mcp").catch((error: unknown) => {
            tunnelError = messageOf(error);
          });
          latest = await waitForPublicEndpoint(tunnelError ? 3000 : 30000);
        }
        if (!latest.publicEndpoint) {
          const base = $t(
            "The MCP service is running, but the public tunnel did not connect. Check your network or proxy, then try again.",
          );
          throw new Error(tunnelError ? `${base}\n\n${tunnelError}` : base);
        }
        setMcpState(latest);
      }

      await refreshProfile();
      await refreshSecrets();
    } catch (error) {
      errorMessage = messageOf(error);
    } finally {
      working = false;
      phase = null;
      await refreshStatus();
    }
  }

  async function turnOff() {
    if (!canControl || working) return;
    working = true;
    phase = "Stopping…";
    errorMessage = "";
    try {
      setMcpState(await stopRuntime(profile.id));
    } catch (error) {
      errorMessage = messageOf(error);
    } finally {
      working = false;
      phase = null;
    }
  }

  function toggle() {
    if (running) void turnOff();
    else void turnOn();
  }

  async function restartIfRunning() {
    if (!running) return;
    await turnOff();
    await turnOn();
  }

  async function chooseQuick() {
    modeView = "quick";
    errorMessage = "";
    if (configuredKind === "quick" && hasTunnel(profile.tunnel.type)) return;
    try {
      await updateWorkspace({
        ...profile,
        tunnel: {
          ...profile.tunnel,
          type: "cloudflare",
          cloudflare_mode: "quick",
          public_url: "",
          use_proxy: profile.tunnel.use_proxy ?? true,
        },
      });
      await refreshProfile();
      modeView = null;
      await restartIfRunning();
    } catch (error) {
      errorMessage = messageOf(error);
    }
  }

  function chooseNamed() {
    modeView = "named";
    errorMessage = "";
  }

  function normalizedDomain(value: string): string {
    const raw = value.trim();
    const url = new URL(/^https?:\/\//i.test(raw) ? raw : `https://${raw}`);
    if (!url.hostname.includes(".")) throw new Error();
    return `https://${url.hostname}`;
  }

  async function saveNamed() {
    if (savingNamed) return;
    errorMessage = "";
    let domain = "";
    try {
      domain = normalizedDomain(namedDomain);
    } catch {
      errorMessage = $t("Enter a domain such as mcp.example.com.");
      return;
    }
    if (!namedToken.trim() && !namedTokenSaved) {
      errorMessage = $t("Enter the Tunnel Token and the domain first.");
      return;
    }
    savingNamed = true;
    try {
      if (namedToken.trim()) {
        await setWorkspaceSecret(profile.id, "cloudflare_token", namedToken.trim());
        namedTokenSaved = true;
        namedToken = "";
      }
      await updateWorkspace({
        ...profile,
        tunnel: {
          ...profile.tunnel,
          type: "cloudflare",
          cloudflare_mode: "named",
          public_url: domain,
          use_proxy: profile.tunnel.use_proxy ?? true,
        },
      });
      await refreshProfile();
      namedDomain = domain;
      modeView = null;
      showToast($t("Saved"), { kind: "success" });
      await restartIfRunning();
    } catch (error) {
      errorMessage = messageOf(error);
    } finally {
      savingNamed = false;
    }
  }

  async function writeClipboard(value: string) {
    try {
      await navigator.clipboard.writeText(value);
    } catch {
      const area = document.createElement("textarea");
      area.value = value;
      area.setAttribute("readonly", "");
      area.className = "ax-offscreen";
      document.body.appendChild(area);
      area.select();
      document.execCommand("copy");
      area.remove();
    }
  }

  async function copy(key: string, value: string) {
    if (!value) return;
    await writeClipboard(value);
    copiedKey = key;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copiedKey = null), 1600);
  }

  async function copyPrompt() {
    // Always copy with the freshest one-time password.
    await refreshSecrets();
    await copy("prompt", prompt);
  }

  onMount(() => {
    const shouldAutostart = $page.url.searchParams.get("autostart") === "1";
    void refreshNamedState();
    void (async () => {
      await refreshAll();
      if (shouldAutostart) {
        const url = new URL($page.url);
        url.searchParams.delete("autostart");
        replaceState(url, $page.state);
        if (status?.state !== "running" || !status?.publicEndpoint) await turnOn();
      }
    })();
    pollTimer = setInterval(() => {
      if (!working) void refreshAll();
    }, 8000);
  });

  onDestroy(() => {
    destroyed = true;
    clearInterval(pollTimer);
    clearTimeout(copiedTimer);
  });
</script>

<section class="page-scroll sx-page">
  <div class="sx-card ax-glass">
    <header class="sx-head">
      <h2 class="sx-title">{profile.name}</h2>
      {#each folders as folder (folder)}
        <p class="sx-folder" title={folder}><Folder size={12} aria-hidden="true" />{folder}</p>
      {/each}
    </header>

    <!-- The one switch -->
    <div class="sx-power">
      <button
        type="button"
        role="switch"
        class="sx-switch"
        class:is-on={switchOn}
        class:is-busy={working}
        aria-checked={switchOn}
        aria-label={$t("Online")}
        disabled={!canControl || working}
        onclick={toggle}
      >
        <span class="sx-switch-knob">
          {#if working}<LoaderCircle size={16} class="animate-spin" />{/if}
        </span>
      </button>
      <div class="sx-power-text">
        <p class="sx-state" class:is-on={running && hasPublic} class:is-local={running && !hasPublic}>
          {#if working && phase}
            {$t(phase)}
          {:else if running && hasPublic}
            {$t("Online")}
          {:else if running}
            {$t("Local only")}
          {:else}
            {$t("Stopped")}
          {/if}
        </p>
        {#if endpoint}
          <button type="button" class="sx-endpoint tx-mono" title={$t("Copy")} onclick={() => void copy("endpoint", endpoint)}>
            <span class="truncate">{endpoint}</span>
            {#if copiedKey === "endpoint"}<Check size={12} />{:else}<Copy size={12} />{/if}
          </button>
        {/if}
      </div>
    </div>

    <!-- Tunnel mode -->
    <div class="sx-modes ax-segment" role="radiogroup" aria-label={$t("Tunnel")}>
      <button
        type="button"
        role="radio"
        aria-checked={kind === "quick"}
        class:active={kind === "quick"}
        disabled={working || savingNamed}
        onclick={() => void chooseQuick()}
      >
        {$t("Temporary tunnel")}
      </button>
      <button
        type="button"
        role="radio"
        aria-checked={kind === "named"}
        class:active={kind === "named"}
        disabled={working || savingNamed}
        onclick={chooseNamed}
      >
        {$t("Fixed domain")}
      </button>
    </div>

    {#if kind === "other"}
      <p class="sx-hint">{$t("A custom tunnel is configured in Advanced settings.")}</p>
    {:else if modeView === "named" || (kind === "named" && (!namedTokenSaved || !profile.tunnel.public_url))}
      <form class="sx-named" onsubmit={(event) => { event.preventDefault(); void saveNamed(); }}>
        <input
          class="tx-input"
          type="password"
          autocomplete="off"
          placeholder={namedTokenSaved ? $t("Token saved — leave empty to keep it") : $t("Cloudflare Tunnel Token")}
          bind:value={namedToken}
        />
        <input class="tx-input" type="text" autocomplete="off" placeholder="mcp.example.com" bind:value={namedDomain} />
        <button type="submit" class="tx-btn-ghost" disabled={savingNamed}>
          {#if savingNamed}<LoaderCircle size={13} class="animate-spin" />{/if}
          {$t("Save")}
        </button>
      </form>
    {:else if kind === "named"}
      <button type="button" class="sx-hint sx-hint-btn" onclick={chooseNamed}>
        {profile.tunnel.public_url} · {$t("Edit")}
      </button>
    {/if}

    <!-- Copy prompt -->
    <button type="button" class="sx-copy" disabled={!endpoint} onclick={() => void copyPrompt()}>
      {#if copiedKey === "prompt"}<Check size={17} />{:else}<Sparkles size={17} />{/if}
      <span>{copiedKey === "prompt" ? $t("Copied!") : $t("Copy prompt")}</span>
    </button>
    {#if endpoint && (authType === "oauth" || isTemporaryEndpoint(endpoint))}
      <p class="sx-note">
        {authType === "oauth" ? $t("The password is one-time; copy again for each new connection.") : ""}
        {isTemporaryEndpoint(endpoint) ? $t("Temporary address changes on restart.") : ""}
      </p>
    {/if}

    {#if errorMessage}
      <div class="tx-alert tx-alert--error sx-error" role="alert">
        <p>{errorMessage}</p>
        <button type="button" class="tx-btn-ghost" onclick={() => void turnOn()}>
          <RefreshCw size={13} /> {$t("Try again")}
        </button>
      </div>
    {/if}

    <button type="button" class="sx-advanced" onclick={() => uiMode.set("advanced")}>
      <Settings2 size={14} /> {$t("Advanced settings")}
    </button>
  </div>
</section>
