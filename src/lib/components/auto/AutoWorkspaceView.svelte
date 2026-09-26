<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { page } from "$app/stores";
  import { replaceState } from "$app/navigation";
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import Folder from "@lucide/svelte/icons/folder";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import X from "@lucide/svelte/icons/x";
  import { pickDirectory } from "$lib/api/native";
  import { getWorkspaceSecret, setWorkspaceSecret } from "$lib/api/secrets";
  import { addWorkspaceFolder, listWorkspaces, removeWorkspaceFolder, updateWorkspace } from "$lib/api/workspaces";
  import { getBackend, loadMcpAuthSecrets } from "$lib/backend";
  import { buildConnectionPrompt, isTemporaryEndpoint } from "$lib/connect/prompt";
  import { locale, t } from "$lib/i18n";
  import { workspaces } from "$lib/stores/app";
  import {
    clearError,
    isReachablePublic,
    messageOf,
    refreshSession,
    restartIfRunning,
    sessionOf,
    sessions,
    tunnelUsable,
    turnOff,
    turnOn,
  } from "$lib/stores/sessions";
  import { showToast } from "$lib/stores/toast";
  import { uiMode } from "$lib/stores/ui-mode";
  import { workspaceFolders, type WorkspaceProfile } from "$lib/types";

  interface Props {
    profile: WorkspaceProfile;
    onProfileChange?: (profile: WorkspaceProfile) => void;
  }

  let { profile: initialProfile, onProfileChange }: Props = $props();

  type TunnelKind = "quick" | "named" | "other";

  const capabilities = getBackend().capabilities;
  const canControl = capabilities.runtimeSupervisor;
  const canPickFolder = capabilities.nativeDirectoryPicker && capabilities.workspaceLifecycle;
  // The page re-creates this view per workspace ({#key profile.id}).
  const id = untrack(() => initialProfile.id);

  let copiedKey = $state<string | null>(null);
  let copying = $state(false);
  let modeView = $state<TunnelKind | null>(null);
  let namedToken = $state("");
  let namedDomain = $state("");
  let namedTokenSaved = $state(false);
  let saving = $state(false);
  let localError = $state("");
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  // Always render the freshest profile from the shared store.
  const profile = $derived($workspaces.find((item) => item.id === id) ?? initialProfile);
  const session = $derived(sessionOf($sessions, id));
  const status = $derived(session.status);
  const working = $derived(session.working);
  const errorMessage = $derived(localError || session.error);
  const running = $derived(status?.state === "running");
  const publicEndpoint = $derived(running && isReachablePublic(status?.publicEndpoint) ? status!.publicEndpoint : "");
  const localEndpoint = $derived(status?.localEndpoint || `http://127.0.0.1:${profile.runtime?.local_port ?? ""}/mcp`);
  const endpoint = $derived(publicEndpoint || (running ? localEndpoint : ""));
  const hasPublic = $derived(Boolean(publicEndpoint));
  const folders = $derived(workspaceFolders(profile));
  const authType = $derived(profile.auth.type);
  const configuredKind = $derived<TunnelKind>(
    profile.tunnel.type === "cloudflare"
      ? profile.tunnel.cloudflare_mode === "named"
        ? "named"
        : "quick"
      : tunnelUsable(profile)
        ? "other"
        : "quick",
  );
  const kind = $derived<TunnelKind>(modeView ?? configuredKind);
  const switchOn = $derived(working ? session.phase !== "Stopping…" : running);
  const busy = $derived(working || saving);

  function applyProfiles(items: WorkspaceProfile[]) {
    workspaces.set(items);
    const next = items.find((item) => item.id === id);
    if (next) onProfileChange?.(next);
  }

  function toggle() {
    localError = "";
    if (running) void turnOff(id);
    else void turnOn(id);
  }

  async function retry() {
    localError = "";
    clearError(id);
    await turnOn(id);
  }

  async function chooseQuick() {
    modeView = "quick";
    localError = "";
    if (profile.tunnel.type === "cloudflare" && profile.tunnel.cloudflare_mode !== "named") {
      modeView = null;
      return;
    }
    saving = true;
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
      applyProfiles(await listWorkspaces());
      modeView = null;
      void restartIfRunning(id);
    } catch (error) {
      localError = messageOf(error);
    } finally {
      saving = false;
    }
  }

  function chooseNamed() {
    modeView = "named";
    localError = "";
    if (!namedDomain && profile.tunnel.cloudflare_mode === "named") namedDomain = profile.tunnel.public_url;
  }

  function normalizedDomain(value: string): string {
    const raw = value.trim();
    const url = new URL(/^https?:\/\//i.test(raw) ? raw : `https://${raw}`);
    if (!url.hostname.includes(".")) throw new Error();
    return `https://${url.hostname}`;
  }

  async function saveNamed() {
    if (saving) return;
    localError = "";
    let domain = "";
    try {
      domain = normalizedDomain(namedDomain);
    } catch {
      localError = $t("Enter a domain such as mcp.example.com.");
      return;
    }
    if (!namedToken.trim() && !namedTokenSaved) {
      localError = $t("Enter the Tunnel Token and the domain first.");
      return;
    }
    saving = true;
    try {
      if (namedToken.trim()) {
        await setWorkspaceSecret(id, "cloudflare_token", namedToken.trim());
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
      applyProfiles(await listWorkspaces());
      namedDomain = domain;
      modeView = null;
      showToast($t("Saved"), { kind: "success" });
      void restartIfRunning(id);
    } catch (error) {
      localError = messageOf(error);
    } finally {
      saving = false;
    }
  }

  async function addFolder() {
    localError = "";
    try {
      const selected = await pickDirectory({ multiple: false });
      if (!selected || Array.isArray(selected)) return;
      await addWorkspaceFolder(id, selected);
      applyProfiles(await listWorkspaces());
    } catch (error) {
      localError = messageOf(error);
    }
  }

  async function removeFolder(folderId: string) {
    localError = "";
    try {
      await removeWorkspaceFolder(id, folderId);
      applyProfiles(await listWorkspaces());
    } catch (error) {
      localError = messageOf(error);
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

  function flash(key: string) {
    copiedKey = key;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copiedKey = null), 1600);
  }

  async function copyEndpoint() {
    if (!endpoint) return;
    await writeClipboard(endpoint);
    flash("endpoint");
  }

  async function copyPrompt() {
    if (!endpoint || copying) return;
    copying = true;
    try {
      // Secrets are only read here, so the one-time password is always fresh.
      const loaded = await loadMcpAuthSecrets(getBackend(), id, profile.auth);
      const text = buildConnectionPrompt(
        {
          workspaceName: profile.name,
          endpoint,
          authType,
          clientId: loaded.oauth_client_id || profile.auth.oauth_client_id,
          password: loaded.oauth_password ?? "",
          bearerToken: loaded.bearer_token ?? "",
          folders: folders.map((folder) => folder.path),
        },
        $locale,
      );
      await writeClipboard(text);
      flash("prompt");
    } catch (error) {
      localError = messageOf(error);
    } finally {
      copying = false;
    }
  }

  onMount(() => {
    void refreshSession(id);
    void getWorkspaceSecret(id, "cloudflare_token")
      .then((value) => (namedTokenSaved = Boolean(value?.trim())))
      .catch(() => undefined);
    if ($page.url.searchParams.get("autostart") === "1") {
      const url = new URL($page.url);
      url.searchParams.delete("autostart");
      replaceState(url, $page.state);
      void turnOn(id);
    }
    return () => clearTimeout(copiedTimer);
  });
</script>

<section class="page-scroll sx-page">
  <div class="sx-card ax-glass">
    <header class="sx-head">
      <h2 class="sx-title">{profile.name}</h2>
    </header>

    <!-- Folders served by this MCP -->
    <ul class="sx-folders">
      {#each folders as folder (folder.id)}
        <li class="sx-folder-row" title={folder.path}>
          <Folder size={12} aria-hidden="true" />
          <span class="truncate">{folder.path}</span>
          {#if folders.length > 1}
            <button type="button" class="sx-folder-x" title={$t("Remove")} aria-label={$t("Remove")} onclick={() => void removeFolder(folder.id)}>
              <X size={12} />
            </button>
          {/if}
        </li>
      {/each}
      {#if canPickFolder}
        <li>
          <button type="button" class="sx-folder-add" onclick={() => void addFolder()}>
            <FolderPlus size={12} aria-hidden="true" /> {$t("Add a directory to this MCP")}
          </button>
        </li>
      {/if}
    </ul>

    <!-- The one switch -->
    <div class="sx-power">
      <button
        type="button"
        role="switch"
        class="sx-switch"
        class:is-on={switchOn}
        aria-checked={switchOn}
        aria-label={$t("Online")}
        disabled={!canControl || working || status === null}
        onclick={toggle}
      >
        <span class="sx-switch-knob">
          {#if working}<LoaderCircle size={16} class="animate-spin" />{/if}
        </span>
      </button>
      <div class="sx-power-text">
        <p class="sx-state" class:is-on={running && hasPublic} class:is-local={running && !hasPublic} class:is-error={status?.state === "error"}>
          {#if working && session.phase}
            {$t(session.phase)}
          {:else if status === null}
            …
          {:else if running && hasPublic}
            {$t("Online")}
          {:else if running}
            {$t("Local only")}
          {:else if status.state === "starting"}
            {$t("Starting…")}
          {:else if status.state === "error"}
            {$t("Error")}
          {:else}
            {$t("Stopped")}
          {/if}
        </p>
        {#if endpoint}
          <button type="button" class="sx-endpoint tx-mono" title={$t("Copy")} onclick={() => void copyEndpoint()}>
            <span class="truncate">{endpoint}</span>
            {#if copiedKey === "endpoint"}<Check size={12} />{:else}<Copy size={12} />{/if}
          </button>
        {:else if status?.state === "error" && status.localMessage}
          <p class="sx-endpoint">{status.localMessage}</p>
        {/if}
      </div>
    </div>

    <!-- Tunnel mode -->
    <div class="sx-modes ax-segment" role="radiogroup" aria-label={$t("Tunnel")}>
      <button type="button" role="radio" aria-checked={kind === "quick"} class:active={kind === "quick"} disabled={busy} onclick={() => void chooseQuick()}>
        {$t("Temporary tunnel")}
      </button>
      <button type="button" role="radio" aria-checked={kind === "named"} class:active={kind === "named"} disabled={busy} onclick={chooseNamed}>
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
        <button type="submit" class="tx-btn-ghost" disabled={saving}>
          {#if saving}<LoaderCircle size={13} class="animate-spin" />{/if}
          {$t("Save")}
        </button>
      </form>
    {:else if kind === "named"}
      <button type="button" class="sx-hint sx-hint-btn" onclick={chooseNamed}>
        {profile.tunnel.public_url} · {$t("Edit")}
      </button>
    {/if}

    <!-- Copy prompt -->
    <button type="button" class="sx-copy" disabled={!endpoint || copying} onclick={() => void copyPrompt()}>
      {#if copiedKey === "prompt"}<Check size={17} />{:else if copying}<LoaderCircle size={17} class="animate-spin" />{:else}<Sparkles size={17} />{/if}
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
        <button type="button" class="tx-btn-ghost" disabled={working} onclick={() => void retry()}>
          <RefreshCw size={13} /> {$t("Try again")}
        </button>
      </div>
    {/if}

    <button type="button" class="sx-advanced" onclick={() => uiMode.set("advanced")}>
      <Settings2 size={14} /> {$t("Advanced settings")}
    </button>
  </div>
</section>
