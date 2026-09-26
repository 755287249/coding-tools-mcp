<script lang="ts">
  import { onMount } from "svelte";
  import { listFrpProfiles, type FrpProfileDto } from "$lib/api/settings";
  import { getBackend } from "$lib/backend";
  import { testTunnel as invokeTunnelTest } from "$lib/api/tunnel";
  import SecretTokenField from "$lib/components/SecretTokenField.svelte";
  import CloudflareRouteGuide from "$lib/components/CloudflareRouteGuide.svelte";
  import { workspaces } from "$lib/stores/app";
  import { actionsConfig } from "$lib/types";
  import { showToast } from "$lib/stores/toast";
  import { t } from "$lib/i18n";

  export interface TunnelFormConfig {
    type: string;
    public_url: string;
    frp_server: string;
    frp_subdomain: string;
    frp_profile_id: string;
    frp_server_port: number;
    cloudflare_mode: string;
    use_proxy: boolean;
  }

  export interface SaveTunnelOptions {
    skipTunnelRestart?: boolean;
    skipServicePrompt?: boolean;
  }

  interface Props {
    workspaceId: string;
    service: "mcp" | "actions";
    config: TunnelFormConfig;
    onSave: (config: TunnelFormConfig, options?: SaveTunnelOptions) => void | Promise<void>;
  }

  let { workspaceId, service, config, onSave }: Props = $props();
  const capabilities = getBackend().capabilities;

  let draft = $state<TunnelFormConfig>({
    type: "none",
    public_url: "",
    frp_server: "",
    frp_subdomain: "",
    frp_profile_id: "",
    frp_server_port: 443,
    cloudflare_mode: "quick",
    use_proxy: true,
  });
  let saving = $state(false);
  let testing = $state(false);
  let tokenField = $state<SecretTokenField | null>(null);
  let tokenPending = $state(false);
  let frpProfiles = $state<FrpProfileDto[]>([]);
  let legacyFrpOpen = $state(false);


  const secretKey = $derived(
    draft.type === "builtin"
      ? ("builtin_tunnel_enrollment_url" as const)
      : service === "mcp"
        ? draft.type === "frp"
          ? ("frp_token" as const)
          : ("cloudflare_token" as const)
        : draft.type === "frp"
          ? ("actions_frp_token" as const)
          : ("actions_cloudflare_token" as const),
  );

  const selectedProfile = $derived(
    frpProfiles.find((profile) => profile.id === draft.frp_profile_id) ?? null,
  );

  const useGlobalProfile = $derived(Boolean(draft.frp_profile_id && selectedProfile));

  const dirty = $derived(
    draft.type !== config.type ||
      draft.public_url !== config.public_url ||
      draft.frp_server !== config.frp_server ||
      draft.frp_subdomain !== config.frp_subdomain ||
      draft.frp_profile_id !== config.frp_profile_id ||
      draft.frp_server_port !== config.frp_server_port ||
      draft.cloudflare_mode !== config.cloudflare_mode ||
      draft.use_proxy !== config.use_proxy ||
      tokenPending,
  );

  const showFrp = $derived(draft.type === "frp");
  const showBuiltin = $derived(draft.type === "builtin");
  const showCloudflare = $derived(draft.type === "cloudflare");
  const showCloudflareToken = $derived(showCloudflare && draft.cloudflare_mode === "named");
  const workspaceProfile = $derived($workspaces.find((item) => item.id === workspaceId) ?? null);
  /** Local listener the named tunnel's dashboard route must point at. */
  const routeTarget = $derived.by(() => {
    if (!workspaceProfile) return null;
    if (service === "actions") {
      const actions = actionsConfig(workspaceProfile);
      return { port: actions.local_port, bindAddress: actions.bind_address };
    }
    const runtime = workspaceProfile.runtime;
    return runtime ? { port: runtime.local_port, bindAddress: runtime.bind_address } : null;
  });
  const mcpUrlScoped = $derived(showBuiltin || (showFrp && service === "mcp"));
  const showLegacyFrpToken = $derived(showFrp && service === "actions" && !useGlobalProfile);
  const canTest = $derived(showBuiltin || draft.type === "frp" || draft.type === "cloudflare");
  const showProxyOption = $derived(draft.type === "frp" || draft.type === "cloudflare");

  $effect(() => {
    draft = {
      ...config,
      frp_profile_id: config.frp_profile_id ?? "",
      use_proxy: config.use_proxy ?? true,
    };
  });

  onMount(async () => {
    if (!capabilities.frpManagement) return;
    frpProfiles = await listFrpProfiles();
  });

  function normalizeMcpFrpUrl(value: string): string {
    const input = value.trim();
    let url: URL;
    try {
      url = new URL(input);
    } catch {
      throw new Error($t("Enter a valid public MCP URL."));
    }
    if (url.protocol !== "https:") {
      throw new Error($t("The public MCP URL must use HTTPS."));
    }
    if (url.username || url.password || url.search || url.hash) {
      throw new Error($t("The public MCP URL cannot contain credentials, a query, or a fragment."));
    }
    const path = url.pathname;
    if (path === "/mcp" || !path.endsWith("/mcp")) {
      throw new Error($t("Use a unique path such as /clients/pc-a/mcp."));
    }
    url.pathname = path;
    return url.toString().replace(/\/$/, "");
  }

  function parseEnrollmentOrigin(value: string): URL {
    let url: URL;
    try {
      url = new URL(value.trim());
    } catch {
      throw new Error($t("Enter a valid one-time enrollment link."));
    }
    if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) {
      throw new Error($t("Enter a valid one-time enrollment link."));
    }
    const path = url.pathname.replace(/\/+$/, "");
    if (!/^\/_tunnel\/enroll\/[A-Za-z0-9]{1,128}$/.test(path)) {
      throw new Error($t("Use /_tunnel/enroll/<code> for the one-time enrollment link."));
    }
    url.pathname = path;
    return url;
  }

  function builtinUrlFromEnrollment(value: string): string {
    const enrollment = parseEnrollmentOrigin(value);
    const clientId = currentClientId();
    return service === "mcp"
      ? `${enrollment.origin}/builtin/clients/${clientId}/mcp`
      : `${enrollment.origin}/builtin/actions/${clientId}`;
  }

  function normalizeBuiltinUrlValue(value: string): string {
    let url: URL;
    try {
      url = new URL(value.trim());
    } catch {
      throw new Error($t("Enter a valid built-in tunnel public URL."));
    }
    if (url.protocol !== "https:") {
      throw new Error($t("The built-in tunnel public URL must use HTTPS."));
    }
    if (url.username || url.password || url.search || url.hash) {
      throw new Error($t("The built-in tunnel public URL cannot contain credentials, a query, or a fragment."));
    }
    const path = url.pathname.replace(/\/+$/, "");
    const expected =
      service === "mcp"
        ? /^\/builtin\/clients\/[A-Za-z0-9_-]+\/mcp$/
        : /^\/builtin\/actions\/[A-Za-z0-9_-]+$/;
    if (!expected.test(path)) {
      throw new Error(
        service === "mcp"
          ? $t("Use /builtin/clients/<client-id>/mcp for the built-in MCP tunnel.")
          : $t("Use /builtin/actions/<client-id> for the built-in Actions tunnel."),
      );
    }
    url.pathname = path;
    return url.toString().replace(/\/$/, "");
  }

  function normalizeBuiltinUrl(value: string, enrollmentValue = ""): string {
    const source = enrollmentValue.trim() ? builtinUrlFromEnrollment(enrollmentValue) : value;
    return normalizeBuiltinUrlValue(source);
  }


  async function saveDraft(options?: SaveTunnelOptions) {
    const next = { ...draft };
    if (showBuiltin) {
      const enrollmentValue = tokenField?.pendingValue() ?? "";
      next.public_url = normalizeBuiltinUrl(next.public_url, enrollmentValue);
      next.frp_server = "";
      next.frp_subdomain = "";
      next.frp_profile_id = "";
      next.frp_server_port = 443;
      next.use_proxy = false;
      draft = { ...next };
    } else if (showFrp && service === "mcp") {
      next.public_url = normalizeMcpFrpUrl(next.public_url);
      next.frp_server = "";
      next.frp_subdomain = "";
      next.frp_profile_id = "";
      next.frp_server_port = 443;
      draft = { ...next };
    }
    if (tokenField && (showBuiltin || showLegacyFrpToken || showCloudflareToken)) {
      await tokenField.saveIfDirty();
    }
    await onSave(next, options);
  }

  function tunnelOrigin(): string {
    try {
      return new URL(draft.public_url.trim()).origin;
    } catch {
      return "http://127.0.0.1:8088";
    }
  }

  function currentClientId(): string {
    try {
      const segments = new URL(draft.public_url.trim()).pathname.split("/").filter(Boolean);
      const marker = service === "mcp" ? "clients" : "actions";
      const index = segments.indexOf(marker);
      const value = index >= 0 ? segments[index + 1] : "";
      return /^[A-Za-z0-9_-]+$/.test(value) ? value : workspaceId;
    } catch {
      return workspaceId;
    }
  }

  function autoFillBuiltinUrl(enrollmentValue: string) {
    if (!showBuiltin || !enrollmentValue.trim()) return;
    try {
      const publicUrl = builtinUrlFromEnrollment(enrollmentValue);
      if (draft.public_url === publicUrl) return;
      draft = { ...draft, public_url: publicUrl };
    } catch {
      // Keep the existing value so save can show the precise enrollment error.
    }
  }

  function changeTunnelType(event: Event) {
    const nextType = (event.currentTarget as HTMLSelectElement).value;
    const previousType = draft.type;
    if (nextType === previousType) return;

    const origin = tunnelOrigin();
    const clientId = currentClientId();
    let publicUrl = draft.public_url;

    if (nextType === "builtin") {
      publicUrl =
        service === "mcp"
          ? `${origin}/builtin/clients/${clientId}/mcp`
          : `${origin}/builtin/actions/${clientId}`;
    } else if (previousType === "builtin") {
      publicUrl =
        nextType === "frp" && service === "mcp"
          ? `${origin}/clients/${clientId}/mcp`
          : "";
    }

    draft = {
      ...draft,
      type: nextType,
      public_url: publicUrl,
      use_proxy: nextType === "builtin" ? false : draft.use_proxy,
    };
  }

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    try {
      await saveDraft();
      showToast($t("Tunnel settings saved."), { title: $t("Save successful"), kind: "success" });
    } catch (error) {
      showToast(String(error), { title: $t("Failed to save"), kind: "error", duration: 8000 });
    } finally {
      saving = false;
    }
  }

  async function testTunnelConnection() {
    if (!canTest || testing) return;
    testing = true;
    try {
      if (dirty) {
        await saveDraft({ skipTunnelRestart: true, skipServicePrompt: true });
      }

      const result = await invokeTunnelTest(workspaceId, service);
      if (result.publicUrl && (showBuiltin || (showCloudflare && draft.cloudflare_mode === "quick"))) {
        draft = { ...draft, public_url: result.publicUrl };
        await onSave(draft, { skipTunnelRestart: true, skipServicePrompt: true });
      }

      if (result.success && result.publicUrl) {
        const detail = `${result.message}\n${result.publicUrl}${
          result.keptRunning ? "" : `\n\n${$t("For long-term use, start the service first.")}`
        }`;
        showToast(detail, { title: $t("Test successful"), kind: "success", duration: 8000 });
      } else if (result.success) {
        showToast(result.message, { title: $t("Test successful"), kind: "success" });
      } else {
        showToast(result.message, { title: $t("Test incomplete"), kind: "warning", duration: 7000 });
      }
    } catch (error) {
      showToast(String(error), { title: $t("Test failed"), kind: "error", duration: 8000 });
    } finally {
      testing = false;
    }
  }
</script>

<form
  class="grid gap-3"
  onsubmit={(event) => {
    event.preventDefault();
    void save();
  }}
>
  <label class="grid gap-1">
    <span class="text-xs text-[var(--color-text-muted)]">{$t("Tunnel type")}</span>
    <select
      class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
      value={draft.type}
      onchange={changeTunnelType}
    >
      <option value="none">{$t("Not configured")}</option>
      <option value="builtin">{$t("Built-in WSS tunnel")}</option>
      {#if capabilities.frpManagement}
        <option value="frp">FRP</option>
        <option value="cloudflare">Cloudflare</option>
      {/if}
    </select>
  </label>

  {#if showProxyOption}
    <label class="flex items-start gap-2 rounded-md border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2.5">
      <input
        type="checkbox"
        class="mt-0.5 h-4 w-4"
        bind:checked={draft.use_proxy}
      />
      <span class="grid gap-0.5">
        <span class="text-xs font-medium text-[var(--color-text-secondary)]">{$t("Use network proxy")}</span>
        <span class="text-[11px] text-[var(--color-text-muted)]">
          {$t("When enabled, tunnels use the global proxy under Settings → General. Disable it for a direct connection.")}
        </span>
      </span>
    </label>
  {/if}


  {#if showBuiltin}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">
        {service === "mcp" ? $t("Built-in MCP public URL") : $t("Built-in Actions public URL")}
      </span>
      <input
        type="url"
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
        placeholder={service === "mcp"
          ? "http://127.0.0.1:8088/builtin/clients/pc-a/mcp"
          : "http://127.0.0.1:8088/builtin/actions/pc-a"}
        bind:value={draft.public_url}
        readonly
      />
      <p class="text-[11px] text-[var(--color-text-muted)]">
        {$t("The server assigns the client ID during enrollment and the public URL is updated automatically.")}
      </p>
    </label>
    <SecretTokenField
      bind:this={tokenField}
      bind:hasPending={tokenPending}
      {workspaceId}
      secretKey={secretKey}
      label={$t("One-time enrollment link")}
      placeholder={$t("Paste one-time enrollment link")}
      onValueChange={autoFillBuiltinUrl}
    />
    <p class="text-[11px] text-[var(--color-text-muted)]">
      {$t("The link is consumed once and cleared after this workspace registers its local device key. MCP and Actions share the same device identity.")}
    </p>
  {/if}

  {#if showFrp}
    {#if service === "mcp"}
      <label class="grid gap-1">
        <span class="text-xs text-[var(--color-text-muted)]">{$t("Public MCP URL")}</span>
        <input
          type="url"
          class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
          placeholder="https://example.com/clients/pc-a/mcp"
          bind:value={draft.public_url}
        />
        <p class="text-[11px] text-[var(--color-text-muted)]">
          {$t("The host, WSS port, FRP domain, and client path are derived automatically. Store the token once in the matching global FRP configuration.")}
        </p>
      </label>
    {:else}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("FRP configuration")}</span>
      <select
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
        bind:value={draft.frp_profile_id}
      >
        <option value="">{$t("Manual configuration (legacy)")}</option>
        {#each frpProfiles as profile (profile.id)}
          <option value={profile.id}>
            {profile.name} · {profile.server}:{profile.serverPort}
          </option>
        {/each}
      </select>
      {#if frpProfiles.length === 0}
        <p class="text-[11px] text-[var(--color-text-muted)]">
          {$t("Add a global server configuration under FRP configuration in the sidebar first.")}
        </p>
      {/if}
    </label>

    {#if useGlobalProfile && selectedProfile}
      <div class="rounded-md border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-xs">
        <p class="text-[var(--color-text-secondary)]">
          {$t("Server")}: {selectedProfile.server}:{selectedProfile.serverPort}
        </p>
        <p class="mt-1 text-[var(--color-text-muted)]">
          Token: {selectedProfile.hasToken ? $t("Configured") : $t("Not configured")}
        </p>
      </div>
    {/if}

    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Subdomain")}</span>
      <input
        type="text"
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
        placeholder="my-mcp"
        bind:value={draft.frp_subdomain}
      />
      <p class="text-[11px] text-[var(--color-text-muted)]">
        {$t("Each workspace uses a separate subdomain. Saving restarts frpc automatically if the tunnel is connected.")}
      </p>
    </label>

    {#if !useGlobalProfile}
      <button
        type="button"
        class="text-left text-xs text-[var(--color-accent)] hover:underline"
        onclick={() => {
          legacyFrpOpen = !legacyFrpOpen;
        }}
      >
        {legacyFrpOpen ? $t("Hide manual FRP configuration") : $t("Show manual FRP configuration")}
      </button>
    {/if}

    {#if !useGlobalProfile && legacyFrpOpen}
      <label class="grid gap-1">
        <span class="text-xs text-[var(--color-text-muted)]">{$t("FRP server")}</span>
        <input
          type="text"
          class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
          placeholder="example.com"
          bind:value={draft.frp_server}
        />
      </label>

      <label class="grid gap-1">
        <span class="text-xs text-[var(--color-text-muted)]">{$t("FRP server port")}</span>
        <input
          type="number"
          min="1"
          max="65535"
          class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
          bind:value={draft.frp_server_port}
        />
      </label>

      {#if showLegacyFrpToken}
        <SecretTokenField
          bind:this={tokenField}
          bind:hasPending={tokenPending}
          {workspaceId}
          secretKey={secretKey}
          label={$t("FRP token (optional)")}
        />
      {/if}
    {/if}
    {/if}
  {/if}

  {#if showCloudflare}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Cloudflare mode")}</span>
      <select
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
        bind:value={draft.cloudflare_mode}
      >
        <option value="quick">Quick Tunnel</option>
        <option value="named">Named Tunnel</option>
      </select>
    </label>

    {#if showCloudflareToken}
      <SecretTokenField
        bind:this={tokenField}
        bind:hasPending={tokenPending}
        {workspaceId}
        secretKey={secretKey}
      />
    {/if}
  {/if}

  {#if !mcpUrlScoped}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">
        {$t("Public URL")}
        {#if service === "actions"}
          <span class="text-[var(--color-text-muted)]">({$t("OpenAPI root URL")})</span>
        {/if}
      </span>
      <input
        type="url"
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
        placeholder="https://..."
        bind:value={draft.public_url}
      />
    </label>
  {/if}

  {#if showCloudflareToken && routeTarget}
    <CloudflareRouteGuide hostname={draft.public_url} port={routeTarget.port} bindAddress={routeTarget.bindAddress} />
  {/if}

  <div class="flex justify-end gap-2 pt-1">
    {#if canTest}
      <button
        type="button"
        class="tx-btn-ghost px-3 py-1.5 text-sm disabled:opacity-50"
        disabled={testing || saving}
        onclick={() => void testTunnelConnection()}
      >
        {testing ? $t("Testing…") : $t("Test connection")}
      </button>
    {/if}
    <button
      type="submit"
      class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-[var(--primary-contrast)] transition-opacity hover:opacity-90 disabled:opacity-50"
      disabled={saving || testing || !dirty}
    >
      {saving ? $t("Saving…") : $t("Save configuration")}
    </button>
  </div>
</form>
