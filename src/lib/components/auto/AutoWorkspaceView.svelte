<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { page } from "$app/stores";
  import { replaceState } from "$app/navigation";
  import Check from "@lucide/svelte/icons/check";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import Copy from "@lucide/svelte/icons/copy";
  import Eye from "@lucide/svelte/icons/eye";
  import EyeOff from "@lucide/svelte/icons/eye-off";
  import Folder from "@lucide/svelte/icons/folder";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Play from "@lucide/svelte/icons/play";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import Square from "@lucide/svelte/icons/square";
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
  import {
    buildClientConfigJson,
    buildConnectionPrompt,
    isTemporaryEndpoint,
    type ConnectionInfo,
    type PromptTarget,
  } from "$lib/connect/prompt";
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

  type StepId = "prepare" | "service" | "tunnel" | "details";
  type StepState = "pending" | "active" | "done" | "skipped" | "error";

  const STEP_LABELS: Record<StepId, MessageKey> = {
    prepare: "Prepare the public tunnel",
    service: "Start the MCP service",
    tunnel: "Connect the public tunnel",
    details: "Generate connection details",
  };

  const capabilities = getBackend().capabilities;
  const canControl = capabilities.runtimeSupervisor;
  const canInstallTunnel = capabilities.softwareManagement;

  let status = $state<RuntimeStatus | null>(null);
  let working = $state(false);
  let errorMessage = $state("");
  let steps = $state<Record<StepId, StepState>>(freshSteps());
  let secrets = $state({ oauth_client_id: "", oauth_password: "", bearer_token: "" });
  let target = $state<PromptTarget>("chatgpt");
  let showPrompt = $state(false);
  let revealSecret = $state(false);
  let copiedKey = $state<string | null>(null);
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
  const info = $derived<ConnectionInfo>({
    workspaceName: profile.name,
    endpoint,
    authType,
    clientId: secrets.oauth_client_id || profile.auth.oauth_client_id,
    password: secrets.oauth_password,
    bearerToken: secrets.bearer_token,
    folders,
  });
  const prompt = $derived(endpoint ? buildConnectionPrompt(info, target, $locale) : "");
  const clientConfig = $derived(endpoint ? buildClientConfigJson(info) : "");
  const secretValue = $derived(authType === "bearer" ? secrets.bearer_token : secrets.oauth_password);
  const stepOrder: StepId[] = ["prepare", "service", "tunnel", "details"];

  function freshSteps(): Record<StepId, StepState> {
    return { prepare: "pending", service: "pending", tunnel: "pending", details: "pending" };
  }

  function hasTunnel(type: string | undefined): boolean {
    return type === "cloudflare" || type === "frp" || type === "builtin";
  }

  function sleep(ms: number) {
    return new Promise<void>((resolve) => setTimeout(resolve, ms));
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

  async function runStep<T>(id: StepId, action: () => Promise<T>): Promise<T> {
    steps[id] = "active";
    try {
      const result = await action();
      steps[id] = "done";
      return result;
    } catch (error) {
      steps[id] = "error";
      throw error;
    }
  }

  async function autoStart() {
    if (!canControl || working) return;
    working = true;
    errorMessage = "";
    steps = freshSteps();
    try {
      const prepared = await runStep("prepare", () => ensureTunnelReady(profile));
      if (!hasTunnel(prepared.tunnel.type)) steps.prepare = "skipped";

      const started = await runStep("service", startService);
      setMcpState(started);
      if (started.state !== "running") throw new Error(serviceErrorMessage(started));

      if (hasTunnel(prepared.tunnel.type)) {
        const connected = await runStep("tunnel", async () => {
          let latest = await waitForPublicEndpoint(started.publicEndpoint ? 0 : 6000);
          if (!latest.publicEndpoint) {
            await startTunnel(profile.id, "mcp").catch(() => undefined);
            latest = await waitForPublicEndpoint(20000);
          }
          if (!latest.publicEndpoint) {
            throw new Error(
              $t("The MCP service is running, but the public tunnel did not connect. Check your network or proxy, then try again."),
            );
          }
          return latest;
        });
        setMcpState(connected);
      } else {
        steps.tunnel = "skipped";
      }

      await runStep("details", async () => {
        await refreshProfile();
        await refreshSecrets();
      });
      showToast($t("Ready. Copy the prompt and paste it into your AI assistant."), { kind: "success" });
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : String(error);
    } finally {
      working = false;
      await refreshStatus();
    }
  }

  async function stopService() {
    if (!canControl || working) return;
    working = true;
    errorMessage = "";
    try {
      setMcpState(await stopRuntime(profile.id));
      steps = freshSteps();
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : String(error);
    } finally {
      working = false;
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
    showToast($t("Prompt copied. Paste it into ChatGPT, Claude or any AI assistant."), { kind: "success" });
  }

  function maskSecret(value: string): string {
    if (!value) return "";
    return value.length <= 8 ? "••••••••" : `${value.slice(0, 4)}••••••••${value.slice(-4)}`;
  }

  onMount(() => {
    const shouldAutostart = $page.url.searchParams.get("autostart") === "1";
    void (async () => {
      await refreshAll();
      if (shouldAutostart) {
        const url = new URL($page.url);
        url.searchParams.delete("autostart");
        replaceState(url, $page.state);
        if (status?.state !== "running" || !status?.publicEndpoint) await autoStart();
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

<section class="page-scroll ax-page">
  <div class="ax-container">
    <header class="ax-header">
      <div class="min-w-0">
        <p class="page-kicker">{$t("Workspace")}</p>
        <h2 class="ax-title">{profile.name}</h2>
        <div class="ax-folders">
          {#each folders as folder (folder)}
            <span class="ax-folder" title={folder}><Folder size={12} aria-hidden="true" />{folder}</span>
          {/each}
        </div>
      </div>
      <span class="ax-status" class:is-on={running && hasPublic} class:is-local={running && !hasPublic}>
        <span class="ax-status-dot"></span>
        {#if working}
          {$t("Working…")}
        {:else if running && hasPublic}
          {$t("Online")}
        {:else if running}
          {$t("Local only")}
        {:else}
          {$t("Stopped")}
        {/if}
      </span>
    </header>

    <!-- Step 1: start -->
    <article class="ax-glass ax-hero">
      <div class="ax-hero-main">
        <div class="ax-hero-icon" class:is-on={running}>
          {#if working}
            <LoaderCircle size={22} class="animate-spin" />
          {:else if running}
            <Check size={22} />
          {:else}
            <Play size={22} />
          {/if}
        </div>
        <div class="min-w-0 flex-1">
          <h3 class="ax-hero-title">
            {#if working}
              {$t("Setting everything up…")}
            {:else if running && hasPublic}
              {$t("Ready to connect your AI")}
            {:else if running}
              {$t("Running on this computer")}
            {:else}
              {$t("One click to go online")}
            {/if}
          </h3>
          <p class="ax-hero-desc">
            {#if !canControl}
              {$t("This service is managed by the Node Agent. Copy the prompt below to connect.")}
            {:else if running && hasPublic}
              {$t("The MCP service and the public tunnel are running. Copy the prompt below and paste it into any AI assistant.")}
            {:else if running}
              {$t("The MCP service is running without a public address. Start again to connect the tunnel, or configure one in Advanced mode.")}
            {:else}
              {$t("Starts the MCP service, opens a secure public tunnel and prepares a ready-to-paste prompt. No configuration needed.")}
            {/if}
          </p>
        </div>
        {#if canControl}
          <div class="ax-hero-actions">
            {#if running && !working}
              {#if !hasPublic}
                <button type="button" class="tx-btn-primary" onclick={() => void autoStart()}>
                  <RefreshCw size={14} /> {$t("Connect tunnel")}
                </button>
              {/if}
              <button type="button" class="tx-btn-ghost" onclick={() => void stopService()}>
                <Square size={13} /> {$t("Stop")}
              </button>
            {:else}
              <button type="button" class="tx-btn-primary ax-btn-lg" disabled={working} onclick={() => void autoStart()}>
                {#if working}
                  <LoaderCircle size={15} class="animate-spin" /> {$t("Starting…")}
                {:else}
                  <Play size={15} /> {$t("Start")}
                {/if}
              </button>
            {/if}
          </div>
        {/if}
      </div>

      {#if working || errorMessage || stepOrder.some((id) => steps[id] !== "pending")}
        <ol class="ax-steps">
          {#each stepOrder as id (id)}
            <li class="ax-step" data-state={steps[id]}>
              <span class="ax-step-icon">
                {#if steps[id] === "active"}
                  <LoaderCircle size={13} class="animate-spin" />
                {:else if steps[id] === "done"}
                  <Check size={13} />
                {:else if steps[id] === "error"}
                  <CircleAlert size={13} />
                {:else}
                  <span class="ax-step-dot"></span>
                {/if}
              </span>
              <span>{$t(STEP_LABELS[id])}</span>
              {#if steps[id] === "skipped"}<span class="ax-step-note">{$t("Skipped")}</span>{/if}
            </li>
          {/each}
        </ol>
      {/if}

      {#if errorMessage}
        <div class="tx-alert tx-alert--error ax-error" role="alert">
          <p>{errorMessage}</p>
          <div class="mt-2 flex flex-wrap gap-2">
            <button type="button" class="tx-btn-ghost" onclick={() => void autoStart()}>
              <RefreshCw size={13} /> {$t("Try again")}
            </button>
            <button type="button" class="tx-btn-ghost" onclick={() => uiMode.set("advanced")}>
              <Settings2 size={13} /> {$t("Open Advanced mode")}
            </button>
          </div>
        </div>
      {/if}
    </article>

    <!-- Step 2: connect -->
    {#if endpoint}
      <article class="ax-glass ax-connect">
        <div class="ax-connect-head">
          <div>
            <p class="tx-section-label">{$t("Connect an AI assistant")}</p>
            <h3 class="ax-hero-title">{$t("Copy one prompt, paste it anywhere")}</h3>
            <p class="ax-hero-desc">
              {$t("Paste it into ChatGPT, Claude or any AI assistant. It contains the address and sign-in details, and tells the AI how to connect step by step.")}
            </p>
          </div>
        </div>

        <div class="ax-segment" role="tablist" aria-label={$t("Prompt target")}>
          <button type="button" role="tab" aria-selected={target === "chatgpt"} class:active={target === "chatgpt"} onclick={() => (target = "chatgpt")}>
            ChatGPT
          </button>
          <button type="button" role="tab" aria-selected={target === "generic"} class:active={target === "generic"} onclick={() => (target = "generic")}>
            {$t("Any AI assistant")}
          </button>
        </div>

        <button type="button" class="ax-copy-main" onclick={() => void copyPrompt()}>
          <span class="ax-copy-main-icon">
            {#if copiedKey === "prompt"}<Check size={18} />{:else}<Sparkles size={18} />{/if}
          </span>
          <span class="min-w-0 flex-1 text-left">
            <span class="block font-semibold">{copiedKey === "prompt" ? $t("Copied!") : $t("Copy connection prompt")}</span>
            <span class="ax-copy-main-sub">{$t("Includes the MCP address, Client ID and one-time password")}</span>
          </span>
          <Copy size={16} class="opacity-60" />
        </button>

        <button type="button" class="ax-link" onclick={() => (showPrompt = !showPrompt)} aria-expanded={showPrompt}>
          {showPrompt ? $t("Hide prompt") : $t("Preview prompt")}
        </button>
        {#if showPrompt}
          <pre class="ax-prompt tx-mono">{prompt}</pre>
        {/if}

        <div class="ax-fields">
          <div class="ax-field">
            <span class="ax-field-label">{hasPublic ? $t("MCP address") : $t("Local MCP address")}</span>
            <span class="ax-field-value tx-mono">{endpoint}</span>
            <button type="button" class="ax-icon-copy" title={$t("Copy")} onclick={() => void copy("endpoint", endpoint)}>
              {#if copiedKey === "endpoint"}<Check size={14} />{:else}<Copy size={14} />{/if}
            </button>
          </div>
          {#if authType === "oauth"}
            <div class="ax-field">
              <span class="ax-field-label">Client ID</span>
              <span class="ax-field-value tx-mono">{info.clientId}</span>
              <button type="button" class="ax-icon-copy" title={$t("Copy")} onclick={() => void copy("client", info.clientId)}>
                {#if copiedKey === "client"}<Check size={14} />{:else}<Copy size={14} />{/if}
              </button>
            </div>
          {/if}
          {#if authType === "oauth" || authType === "bearer"}
            <div class="ax-field">
              <span class="ax-field-label">{authType === "oauth" ? $t("One-time password") : "Bearer Token"}</span>
              <span class="ax-field-value tx-mono">{revealSecret ? secretValue : maskSecret(secretValue)}</span>
              <button type="button" class="ax-icon-copy" title={revealSecret ? $t("Hide") : $t("Show")} onclick={() => (revealSecret = !revealSecret)}>
                {#if revealSecret}<EyeOff size={14} />{:else}<Eye size={14} />{/if}
              </button>
              <button type="button" class="ax-icon-copy" title={$t("Copy")} onclick={() => void copy("secret", secretValue)}>
                {#if copiedKey === "secret"}<Check size={14} />{:else}<Copy size={14} />{/if}
              </button>
            </div>
          {/if}
        </div>

        {#if authType !== "oauth"}
          <button type="button" class="ax-link" onclick={() => void copy("json", clientConfig)}>
            {copiedKey === "json" ? $t("Copied!") : $t("Copy JSON config for MCP clients")}
          </button>
        {/if}

        <ul class="ax-notes">
          {#if authType === "oauth"}
            <li>{$t("The password works once and changes after each successful sign-in. Copy the prompt again for a new connection.")}</li>
          {/if}
          {#if isTemporaryEndpoint(endpoint)}
            <li>{$t("This temporary address changes every time the tunnel restarts. For a fixed address, use FRP or a Cloudflare Named Tunnel in Advanced mode.")}</li>
          {/if}
          {#if !hasPublic}
            <li>{$t("Only apps on this computer can reach a local address. Web AI assistants need the public tunnel.")}</li>
          {/if}
        </ul>
      </article>
    {/if}

    <p class="ax-footer">
      {$t("Need ports, tunnels, security policies or logs?")}
      <button type="button" class="ax-link ax-link-inline" onclick={() => uiMode.set("advanced")}>
        {$t("Switch to Advanced mode")}
      </button>
    </p>
  </div>
</section>
