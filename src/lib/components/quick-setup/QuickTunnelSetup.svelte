<script lang="ts">
  import { onMount } from "svelte";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import Download from "@lucide/svelte/icons/download";
  import { listFrpProfiles, saveFrpProfile, type FrpProfileDto } from "$lib/api/settings";
  import { installSoftware, listSoftware, type SoftwareStatus } from "$lib/api/software";
  import SecretInput from "$lib/components/SecretInput.svelte";
  import { t } from "$lib/i18n";

  export type TunnelProvider = "builtin" | "frp" | "cloudflare";
  export type SetupService = "mcp" | "actions";
  export type CloudflareMode = "quick" | "named";

  export interface QuickTunnelSubmission {
    provider: TunnelProvider;
    enrollmentUrl: string;
    frpProfileId: string;
    frpServer: string;
    frpSubdomain: string;
    cloudflareMode: CloudflareMode;
    cloudflareToken: string;
    cloudflarePublicUrl: string;
    useProxy: boolean;
  }

  interface Props {
    provider: TunnelProvider;
    service: SetupService;
    workspaceId: string;
    busy?: boolean;
    onEnable: (input: QuickTunnelSubmission) => void | Promise<void>;
  }

  let { provider, service, workspaceId, busy = false, onEnable }: Props = $props();

  let enrollmentUrl = $state("");
  let software = $state<SoftwareStatus | null>(null);
  let profiles = $state<FrpProfileDto[]>([]);
  let frpProfileId = $state("");
  let frpSubdomain = $state("");
  let cloudflareMode = $state<CloudflareMode>("quick");
  let cloudflareToken = $state("");
  let cloudflarePublicUrl = $state("");
  let useProxy = $state(true);
  let loading = $state(false);
  let installing = $state(false);
  let savingProfile = $state(false);
  let createProfileOpen = $state(false);
  let profileName = $state("");
  let profileServer = $state("");
  let profilePort = $state(443);
  let profileToken = $state("");
  let localError = $state("");

  const softwareKind = $derived(provider === "frp" ? "frpc" : "cloudflared");
  const selectedProfile = $derived(
    profiles.find((item) => item.id === frpProfileId) ?? null,
  );
  const expectedFrpUrl = $derived.by(() => {
    if (!selectedProfile) return "";
    return service === "mcp"
      ? `https://${selectedProfile.server}/clients/${workspaceId}/mcp`
      : frpSubdomain.trim()
        ? `https://${frpSubdomain.trim()}.${selectedProfile.server}`
        : "";
  });
  const ready = $derived.by(() => {
    if (provider === "builtin") return Boolean(enrollmentUrl.trim());
    if (!software?.installed) return false;
    if (provider === "frp") {
      if (!selectedProfile) return false;
      return service === "mcp" || validSubdomain(frpSubdomain);
    }
    if (cloudflareMode === "quick") return true;
    return Boolean(cloudflareToken.trim() && cloudflarePublicUrl.trim());
  });

  onMount(() => {
    if (provider !== "builtin") void loadPrerequisites();
  });

  function validSubdomain(value: string): boolean {
    return /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(value.trim());
  }

  function normalizeServer(value: string): string {
    const input = value.trim();
    if (!input) throw new Error($t("Please enter a configuration name and server hostname."));
    try {
      const parsed = new URL(input.includes("://") ? input : `https://${input}`);
      if (parsed.username || parsed.password || parsed.port || parsed.pathname !== "/" || parsed.search || parsed.hash) {
        throw new Error();
      }
      return parsed.hostname;
    } catch {
      throw new Error($t("Enter only the FRP server hostname, such as frp.example.com."));
    }
  }

  async function loadPrerequisites() {
    loading = true;
    localError = "";
    try {
      const requests: [Promise<SoftwareStatus[]>, Promise<FrpProfileDto[]>] = [
        listSoftware(),
        provider === "frp" ? listFrpProfiles() : Promise.resolve([]),
      ];
      const [items, savedProfiles] = await Promise.all(requests);
      software = items.find((item) => item.kind === softwareKind) ?? null;
      profiles = savedProfiles;
      if (!frpProfileId && profiles.length > 0) frpProfileId = profiles[0].id;
      createProfileOpen = provider === "frp" && profiles.length === 0;
    } catch (error) {
      localError = String(error);
    } finally {
      loading = false;
    }
  }

  async function installRequiredSoftware() {
    if (installing || busy) return;
    installing = true;
    localError = "";
    try {
      software = await installSoftware(softwareKind);
    } catch (error) {
      localError = String(error);
    } finally {
      installing = false;
    }
  }

  async function createFrpProfile() {
    if (savingProfile || busy) return;
    localError = "";
    try {
      if (!profileName.trim()) {
        throw new Error($t("Please enter a configuration name and server hostname."));
      }
      if (!Number.isInteger(profilePort) || profilePort < 1 || profilePort > 65535) {
        throw new Error($t("Enter a valid FRP server port from 1 to 65535."));
      }
      savingProfile = true;
      const saved = await saveFrpProfile(
        {
          id: "",
          name: profileName.trim(),
          server: normalizeServer(profileServer),
          serverPort: profilePort,
        },
        profileToken.trim() || undefined,
      );
      profiles = [...profiles.filter((item) => item.id !== saved.id), saved];
      frpProfileId = saved.id;
      profileName = "";
      profileServer = "";
      profilePort = 443;
      profileToken = "";
      createProfileOpen = false;
    } catch (error) {
      localError = String(error);
    } finally {
      savingProfile = false;
    }
  }

  async function submit() {
    if (!ready || busy) return;
    localError = "";
    if (provider === "frp" && service === "actions" && !validSubdomain(frpSubdomain)) {
      localError = $t("Use a valid subdomain containing only letters, numbers, and hyphens.");
      return;
    }
    await onEnable({
      provider,
      enrollmentUrl,
      frpProfileId,
      frpServer: selectedProfile?.server ?? "",
      frpSubdomain: frpSubdomain.trim(),
      cloudflareMode,
      cloudflareToken,
      cloudflarePublicUrl,
      useProxy,
    });
  }
</script>

{#if localError}
  <div class="tx-alert tx-alert--error mt-4" role="alert">{localError}</div>
{/if}

{#if provider === "builtin"}
  <label class="tx-field mt-6">
    <span class="tx-label">{$t("One-time enrollment link")}</span>
    <input
      class="tx-input tx-mono"
      type="password"
      autocomplete="off"
      placeholder="https://example.com/_tunnel/enroll/abc123"
      bind:value={enrollmentUrl}
      disabled={busy}
    />
    <span class="text-[11px] text-[var(--color-text-muted)]">
      {$t("Expected format: https://server/_tunnel/enroll/code")}
    </span>
    <span class="text-[11px] text-[var(--color-text-muted)]">
      {$t("Ask your server administrator for a fresh link. It registers this computer securely and is cleared after use.")}
    </span>
  </label>
{:else}
  <div class="mt-6 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-hover)] p-4">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div>
        <p class="text-sm font-semibold">{$t("Required software")}: {softwareKind}</p>
        <p class="mt-1 text-xs text-[var(--color-text-secondary)]">
          {#if loading}
            {$t("Checking installation…")}
          {:else if software?.installed}
            {$t("Installed and ready")}
          {:else}
            {$t("Not installed")}
          {/if}
        </p>
      </div>
      {#if software?.installed}
        <CircleCheck size={22} class="text-[var(--success)]" aria-label={$t("Installed and ready")} />
      {:else}
        <button
          type="button"
          class="tx-btn-primary"
          disabled={loading || installing || busy}
          onclick={() => void installRequiredSoftware()}
        >
          <Download size={16} /> {installing ? $t("Installing…") : $t("Install")}
        </button>
      {/if}
    </div>
    <p class="mt-2 text-[11px] text-[var(--color-text-muted)]">
      {$t("The app downloads the verified client into its managed cache and starts it automatically when needed.")}
    </p>
  </div>
{/if}

{#if provider === "frp"}
  <div class="mt-5 grid gap-4">
    <div class="tx-info-block text-sm text-[var(--color-text-secondary)]">
      {$t("Ask your FRP administrator for the server hostname, port, and optional token, then save them as a reusable configuration.")}
    </div>
    <label class="tx-field">
      <span class="tx-label">{$t("FRP configuration")}</span>
      <select class="tx-input" bind:value={frpProfileId} disabled={busy || profiles.length === 0}>
        {#if profiles.length === 0}<option value="">{$t("No FRP configurations.")}</option>{/if}
        {#each profiles as item (item.id)}
          <option value={item.id}>{item.name} · {item.server}:{item.serverPort}</option>
        {/each}
      </select>
    </label>

    <button
      type="button"
      class="w-fit text-sm font-medium text-[var(--primary)] hover:underline"
      disabled={busy}
      onclick={() => (createProfileOpen = !createProfileOpen)}
    >
      {createProfileOpen ? $t("Cancel") : $t("Create configuration")}
    </button>

    {#if createProfileOpen}
      <div class="grid gap-3 rounded-xl border border-[var(--color-border)] p-4 sm:grid-cols-2">
        <label class="tx-field">
          <span class="tx-label">{$t("Name")}</span>
          <input class="tx-input" placeholder={$t("Company FRP")} bind:value={profileName} disabled={busy} />
        </label>
        <label class="tx-field">
          <span class="tx-label">{$t("Server hostname")}</span>
          <input class="tx-input tx-mono" placeholder="frp.example.com" bind:value={profileServer} disabled={busy} />
        </label>
        <label class="tx-field">
          <span class="tx-label">{$t("Port")}</span>
          <input class="tx-input" type="number" min="1" max="65535" bind:value={profilePort} disabled={busy} />
        </label>
        <label class="tx-field">
          <span class="tx-label">{$t("FRP token (optional)")}</span>
          <SecretInput bind:value={profileToken} placeholder="frp auth token" showCopy={false} />
        </label>
        <button
          type="button"
          class="tx-btn-primary w-fit sm:col-span-2"
          disabled={savingProfile || busy}
          onclick={() => void createFrpProfile()}
        >
          {savingProfile ? $t("Saving…") : $t("Add")}
        </button>
      </div>
    {/if}

    {#if service === "actions"}
      <label class="tx-field">
        <span class="tx-label">{$t("Subdomain")}</span>
        <input class="tx-input tx-mono" placeholder="my-project" bind:value={frpSubdomain} disabled={busy} />
        <span class="text-[11px] text-[var(--color-text-muted)]">
          {$t("Use a unique subdomain for this workspace, using only letters, numbers, and hyphens.")}
        </span>
      </label>
    {/if}

    <div class="tx-info-block">
      <p class="tx-label">{service === "mcp" ? $t("Public MCP URL") : $t("Public URL")}</p>
      <p class="tx-mono mt-1 break-all text-sm">{expectedFrpUrl || $t("Select a configuration to generate this value.")}</p>
    </div>
  </div>
{/if}

{#if provider === "cloudflare"}
  <div class="mt-5 grid gap-4">
    <fieldset class="tunnel-modes" disabled={busy}>
      <legend class="tx-label">{$t("Cloudflare mode")}</legend>
      <div class="mode-options">
        <label class:chosen={cloudflareMode==='quick'}>
          <input type="radio" name="cloudflare-mode" value="quick" bind:group={cloudflareMode}/>
          <span><strong>{$t('setup.temporaryTunnel')}</strong><small>Quick Tunnel</small></span>
        </label>
        <label class:chosen={cloudflareMode==='named'}>
          <input type="radio" name="cloudflare-mode" value="named" bind:group={cloudflareMode}/>
          <span><strong>{$t('setup.fixedTunnel')}</strong><small>Named Tunnel</small></span>
        </label>
      </div>
    </fieldset>

    {#if cloudflareMode === "quick"}
      <div class="tx-info-block text-sm text-[var(--color-text-secondary)]">
        {$t("Quick Tunnel needs no token. The temporary trycloudflare.com URL is generated during the connection test.")}
      </div>
    {:else}
      <div class="tx-info-block text-sm text-[var(--color-text-secondary)]">
        {$t("In Cloudflare Zero Trust, create a Named Tunnel, add its public hostname, then copy the Tunnel Token and hostname here.")}
      </div>
      <label class="tx-field">
        <span class="tx-label">{$t("Tunnel Token")}</span>
        <SecretInput bind:value={cloudflareToken} placeholder="eyJhIjoi..." showCopy={false} />
      </label>
      <label class="tx-field">
        <span class="tx-label">{$t("Public URL")}</span>
        <input
          class="tx-input tx-mono"
          type="url"
          placeholder="https://tools.example.com"
          bind:value={cloudflarePublicUrl}
          disabled={busy}
        />
        <span class="text-[11px] text-[var(--color-text-muted)]">
          {$t("Enter the hostname already routed to this Named Tunnel in Cloudflare.")}
        </span>
      </label>
    {/if}
  </div>
{/if}

{#if provider !== "builtin"}
  <label class="mt-5 flex items-start gap-2 rounded-xl border border-[var(--color-border)] p-3">
    <input class="mt-0.5 h-4 w-4" type="checkbox" bind:checked={useProxy} disabled={busy} />
    <span>
      <span class="block text-sm font-medium">{$t("Use network proxy")}</span>
      <span class="block text-[11px] text-[var(--color-text-muted)]">
        {$t("When enabled, tunnels use the global proxy under Settings → General. Disable it for a direct connection.")}
      </span>
    </span>
  </label>
{/if}

<div class="mt-6 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-hover)] p-4">
  <p class="text-sm font-semibold">{$t("What happens next")}</p>
  <ol class="mt-3 grid gap-2 text-sm text-[var(--color-text-secondary)]">
    <li>1. {$t("Save and test the selected reverse proxy configuration.")}</li>
    <li>2. {$t("Start the selected local service and secure public tunnel.")}</li>
    <li>3. {$t("Show the exact values to paste into ChatGPT.")}</li>
  </ol>
</div>

<button
  type="button"
  class="tx-btn-primary mt-6"
  disabled={busy || !ready}
  onclick={() => void submit()}
>
  {busy
    ? $t("Testing and starting…")
    : $t("Configure and enable {service}", { service: service === "mcp" ? $t("MCP") : $t("Actions") })}
</button>

<style>
.tunnel-modes{border:0;padding:0;margin:0;min-width:0}.mode-options{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px;margin-top:8px}.mode-options label{display:flex;align-items:center;gap:10px;padding:16px;border:1px solid var(--color-border);border-radius:12px;background:color-mix(in srgb,#60a5fa 7%,var(--card-bg));cursor:pointer}.mode-options label+label{background:color-mix(in srgb,#a78bfa 7%,var(--card-bg))}.mode-options label.chosen{border-color:#3b82f6;background:color-mix(in srgb,#60a5fa 17%,var(--card-bg));box-shadow:0 0 0 1px #3b82f6}.mode-options label:focus-within{outline:2px solid #3b82f6;outline-offset:3px}.mode-options input{accent-color:#2563eb}.mode-options strong,.mode-options small{display:block}.mode-options strong{font-size:13px}.mode-options small{margin-top:4px;color:var(--color-text-secondary);font-size:11px}.tunnel-modes:disabled{opacity:.5}
</style>
