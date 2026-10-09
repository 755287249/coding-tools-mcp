<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import { appUrl } from "$lib/app-path";
  import { pickDirectory } from "$lib/api/native";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Check from "@lucide/svelte/icons/check";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import Cloud from "@lucide/svelte/icons/cloud";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import PlugZap from "@lucide/svelte/icons/plug-zap";
  import Server from "@lucide/svelte/icons/server";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Workflow from "@lucide/svelte/icons/workflow";
  import { setWorkspaceSecret, type WorkspaceSecretKey } from "$lib/api/secrets";
  import { startTunnel, testTunnel } from "$lib/api/tunnel";
  import {
    addWorkspaceFolder,
    createWorkspace,
    getRuntimeStatus,
    listWorkspaces,
    startActionsRuntime,
    startRuntime,
    updateWorkspace,
  } from "$lib/api/workspaces";
  import { getBackend } from "$lib/backend";
  import GptQuickCopy from "$lib/components/GptQuickCopy.svelte";
  import QuickTunnelSetup from "$lib/components/quick-setup/QuickTunnelSetup.svelte";
  import { t } from "$lib/i18n";
  import { actionsRuntimeStates, mcpRuntimeStates, workspaces } from "$lib/stores/app";
  import { actionsConfig, workspaceFolders, type RuntimeStatus, type WorkspaceProfile } from "$lib/types";

  type WizardStep = "provider" | "workspace" | "service" | "connect" | "complete";
  type SetupService = "mcp" | "actions";
  type TunnelProvider = "builtin" | "frp" | "cloudflare";
  type CloudflareMode = "quick" | "named";

  interface QuickTunnelSubmission {
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

  const capabilities = getBackend().capabilities;
  const isNodeHost = capabilities.host === "node";
  const frpAvailable = capabilities.frpManagement && capabilities.softwareManagement;
  const cloudflareAvailable = capabilities.softwareManagement;

  const steps: Array<{ value: WizardStep; label: "Tunnel" | "Project" | "Connection" | "Enable" | "Finish" }> = [
    { value: "provider", label: "Tunnel" },
    { value: "workspace", label: "Project" },
    { value: "service", label: "Connection" },
    { value: "connect", label: "Enable" },
    { value: "complete", label: "Finish" },
  ];

  let step = $state<WizardStep>("provider");
  let provider = $state<TunnelProvider | null>(null);
  let profile = $state<WorkspaceProfile | null>(null);
  let service = $state<SetupService | null>(null);
  let workspaceName = $state("");
  let runtimeStatus = $state<RuntimeStatus | null>(null);
  let busy = $state(false);
  let errorMessage = $state("");

  const currentStepIndex = $derived(steps.findIndex((item) => item.value === step));
  const selectedActions = $derived(profile ? actionsConfig(profile) : null);
  const publicMcpEndpoint = $derived(profile ? runtimeStatus?.publicEndpoint || profile.tunnel.public_url : "");

  async function refreshWorkspaceStore(workspaceId: string): Promise<WorkspaceProfile | null> {
    const items = await listWorkspaces();
    workspaces.set(items);
    return items.find((item) => item.id === workspaceId) ?? null;
  }

  async function loadNodeWorkspace() {
    if (!isNodeHost) return;
    const items = await listWorkspaces();
    workspaces.set(items);
    const requestedId = $page.url.searchParams.get("workspace");
    profile = items.find((item) => item.id === requestedId) ?? items[0] ?? null;
    workspaceName = profile?.name ?? "";
  }

  function chooseProvider(nextProvider: TunnelProvider) {
    provider = nextProvider;
    errorMessage = "";
  }

  async function selectProjectFolders() {
    if (busy) return;
    errorMessage = "";
    try {
      const selected = await pickDirectory({ multiple: true });
      if (!selected) return;
      const selectedPaths = (Array.isArray(selected) ? selected : [selected]).map((path) => path.trim()).filter(
        (path, index, paths) => path.length > 0 && paths.findIndex((item) => item.toLocaleLowerCase() === path.toLocaleLowerCase()) === index,
      );
      if (selectedPaths.length === 0) return;
      busy = true;
      const primaryFolder = selectedPaths[0]!;
      const additionalFolders = selectedPaths.slice(1);
      let created = await createWorkspace(primaryFolder, workspaceName.trim() || undefined);
      profile = created;
      workspaceName = created.name;
      for (const folderPath of additionalFolders) {
        created = await addWorkspaceFolder(created.id, folderPath);
        profile = created;
      }
      await refreshWorkspaceStore(created.id);
      step = "service";
    } catch (error) {
      errorMessage = String(error);
    } finally {
      busy = false;
    }
  }

  function normalizedEnrollmentUrl(value: string): URL {
    let url: URL;
    try {
      url = new URL(value.trim());
    } catch {
      throw new Error($t("Enter a valid one-time enrollment link."));
    }
    const path = url.pathname.replace(/\/+$/, "");
    if (
      url.protocol !== "https:" || url.username || url.password || url.search || url.hash ||
      !/^\/_tunnel\/enroll\/[A-Za-z0-9]{1,128}$/.test(path)
    ) {
      throw new Error($t("Use /_tunnel/enroll/<code> for the one-time enrollment link."));
    }
    url.pathname = path;
    return url;
  }

  function normalizedHostname(value: string): string {
    try {
      const url = new URL(value.includes("://") ? value : `https://${value}`);
      if (url.username || url.password || url.port || url.pathname !== "/" || url.search || url.hash) throw new Error();
      return url.hostname;
    } catch {
      throw new Error($t("Select a valid FRP server configuration."));
    }
  }

  function normalizedCloudflareUrl(value: string): string {
    try {
      const url = new URL(value.trim());
      if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash || url.pathname !== "/") {
        throw new Error();
      }
      return url.origin;
    } catch {
      throw new Error($t("Enter the HTTPS hostname routed to the Named Tunnel, without a path."));
    }
  }

  function publicUrlFor(input: QuickTunnelSubmission, target: SetupService, workspaceId: string): string {
    if (input.provider === "builtin") {
      const enrollment = normalizedEnrollmentUrl(input.enrollmentUrl);
      return target === "mcp"
        ? `${enrollment.origin}/builtin/clients/${workspaceId}/mcp`
        : `${enrollment.origin}/builtin/actions/${workspaceId}`;
    }
    if (input.provider === "frp") {
      const server = normalizedHostname(input.frpServer);
      if (target === "mcp") return `https://${server}/clients/${workspaceId}/mcp`;
      return `https://${input.frpSubdomain}.${server}`;
    }
    return input.cloudflareMode === "named" ? normalizedCloudflareUrl(input.cloudflarePublicUrl) : "";
  }

  function configuredProfile(
    current: WorkspaceProfile,
    target: SetupService,
    input: QuickTunnelSubmission,
    publicUrl: string,
  ): WorkspaceProfile {
    const frpProfileId = input.provider === "frp" ? input.frpProfileId : "";
    const frpSubdomain = input.provider === "frp" && target === "actions" ? input.frpSubdomain : "";
    const cloudflareMode = input.provider === "cloudflare" ? input.cloudflareMode : "quick";
    if (target === "mcp") {
      return {
        ...current,
        tunnel: {
          ...current.tunnel,
          type: input.provider,
          public_url: publicUrl,
          frp_server: "",
          frp_subdomain: "",
          frp_profile_id: frpProfileId,
          frp_server_port: input.provider === "frp" ? 443 : current.tunnel.frp_server_port,
          cloudflare_mode: cloudflareMode,
          use_proxy: input.provider !== "builtin" && input.useProxy,
        },
      };
    }
    return {
      ...current,
      actions: {
        ...actionsConfig(current),
        tunnel_type: input.provider,
        public_url: publicUrl,
        frp_server: "",
        frp_subdomain: frpSubdomain,
        frp_profile_id: frpProfileId,
        frp_server_port: input.provider === "frp" ? 443 : actionsConfig(current).frp_server_port,
        cloudflare_mode: cloudflareMode,
        use_proxy: input.provider !== "builtin" && input.useProxy,
      },
    };
  }

  async function saveProviderSecret(workspaceId: string, target: SetupService, input: QuickTunnelSubmission) {
    let key: WorkspaceSecretKey | null = null;
    let value = "";
    if (input.provider === "builtin") {
      key = "builtin_tunnel_enrollment_url";
      value = normalizedEnrollmentUrl(input.enrollmentUrl).toString();
    } else if (input.provider === "cloudflare" && input.cloudflareMode === "named") {
      key = target === "mcp" ? "cloudflare_token" : "actions_cloudflare_token";
      value = input.cloudflareToken.trim();
    }
    if (key) await setWorkspaceSecret(workspaceId, key, value);
  }

  async function verifyAndEnable(input: QuickTunnelSubmission) {
    if (!profile || !service || busy) return;
    errorMessage = "";
    busy = true;
    const workspaceId = profile.id;
    const targetService = service;
    try {
      const nextProfile = configuredProfile(profile, targetService, input, publicUrlFor(input, targetService, workspaceId));
      await updateWorkspace(nextProfile);
      await saveProviderSecret(workspaceId, targetService, input);
      let status: RuntimeStatus;
      if (isNodeHost) {
        await startTunnel(workspaceId, targetService);
        status = await getRuntimeStatus(workspaceId);
      } else {
        status = targetService === "mcp"
          ? await startRuntime(workspaceId)
          : await startActionsRuntime(workspaceId);
      }
      if (status.state !== "running") {
        throw new Error(status.localMessage || status.publicMessage || $t("The service failed to start"));
      }
      const tunnel = await testTunnel(workspaceId, targetService);
      if (!tunnel.success || !tunnel.publicUrl || !tunnel.keptRunning) {
        throw new Error(tunnel.message || $t("The tunnel could not be verified."));
      }
      runtimeStatus = status;
      profile = (await refreshWorkspaceStore(workspaceId)) ?? nextProfile;
      const states = targetService === "mcp" ? mcpRuntimeStates : actionsRuntimeStates;
      states.update((current) => ({ ...current, [workspaceId]: status.state }));
      step = "complete";
    } catch (error) {
      errorMessage = String(error);
      profile = (await refreshWorkspaceStore(workspaceId)) ?? profile;
    } finally {
      busy = false;
    }
  }

  function goBack() {
    if (busy) return;
    errorMessage = "";
    if (step === "connect") step = "service";
    else if (step === "service") step = "workspace";
    else if (step === "workspace") step = "provider";
  }

  function openWorkspace() {
    if (profile) void goto(appUrl(`/workspace/${profile.id}`));
  }

  function resetWizard() {
    step = "provider";
    provider = null;
    service = null;
    runtimeStatus = null;
    errorMessage = "";
    if (isNodeHost) {
      void loadNodeWorkspace();
    } else {
      profile = null;
      workspaceName = "";
    }
  }

  onMount(() => {
    if (isNodeHost) void loadNodeWorkspace();
  });
</script>

<section class="page-scroll">
  <header class="page-header">
    <div class="mx-auto max-w-4xl">
      <p class="page-kicker">{$t("Guided setup")}</p>
      <h2 class="page-title">{$t("Connect your project to ChatGPT")}</h2>
      <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-secondary)]">
        {$t("Choose how this computer is exposed first, then create a workspace and copy the exact values into ChatGPT.")}
      </p>
      <ol class="mt-6 grid grid-cols-5 gap-2" aria-label={$t("Setup progress")}>
        {#each steps as item, index (item.value)}
          <li
            class="flex min-w-0 items-center gap-2 rounded-lg border px-2 py-2 text-xs"
            class:border-[var(--primary)]={index === currentStepIndex}
            class:bg-[var(--primary-soft)]={index === currentStepIndex}
            class:border-[var(--color-border)]={index !== currentStepIndex}
            aria-current={index === currentStepIndex ? "step" : undefined}
          >
            <span
              class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold"
              class:bg-[var(--primary)]={index <= currentStepIndex}
              class:text-[var(--primary-contrast)]={index <= currentStepIndex}
              class:bg-[var(--color-surface-hover)]={index > currentStepIndex}
            >
              {#if index < currentStepIndex}<Check size={12} />{:else}{index + 1}{/if}
            </span>
            <span class="truncate font-medium">{$t(item.label)}</span>
          </li>
        {/each}
      </ol>
    </div>
  </header>

  <div class="page-body">
    <div class="mx-auto max-w-4xl">
      {#if errorMessage}<div class="tx-alert tx-alert--error mb-4" role="alert">{errorMessage}</div>{/if}

      {#if step === "provider"}
        <div class="tx-card p-6 sm:p-8">
          <p class="tx-section-label">{$t("Reverse proxy")}</p>
          <h3 class="text-xl font-semibold">{$t("Choose how this computer gets a public URL")}</h3>
          <p class="mt-2 text-sm text-[var(--color-text-secondary)]">
            {$t("This choice determines whether you need an invitation link, frpc, or cloudflared.")}
          </p>
          <div class="mt-6 grid gap-3 md:grid-cols-3">
            <button
              type="button"
              class="rounded-xl border p-5 text-left hover:bg-[var(--color-surface-hover)]"
              class:border-[var(--primary)]={provider === "builtin"}
              class:bg-[var(--primary-soft)]={provider === "builtin"}
              class:border-[var(--color-border)]={provider !== "builtin"}
              aria-pressed={provider === "builtin"}
              onclick={() => chooseProvider("builtin")}
            >
              <ShieldCheck size={23} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("Built-in WSS tunnel (recommended)")}</span>
              <span class="mt-2 block text-sm text-[var(--color-text-secondary)]">
                {$t("No extra software. Continue with a one-time invitation link from your server administrator.")}
              </span>
            </button>
            <button
              type="button"
              class="rounded-xl border p-5 text-left hover:bg-[var(--color-surface-hover)]"
              class:border-[var(--primary)]={provider === "frp"}
              class:bg-[var(--primary-soft)]={provider === "frp"}
              class:border-[var(--color-border)]={provider !== "frp"}
              aria-pressed={provider === "frp"}
              disabled={!frpAvailable}
              onclick={() => chooseProvider("frp")}
            >
              <Server size={23} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("FRP")}</span>
              <span class="mt-2 block text-sm text-[var(--color-text-secondary)]">
                {$t("For a self-hosted or company FRP server. The wizard can install frpc and save the server profile.")}
              </span>
            </button>
            <button
              type="button"
              class="rounded-xl border p-5 text-left hover:bg-[var(--color-surface-hover)]"
              class:border-[var(--primary)]={provider === "cloudflare"}
              class:bg-[var(--primary-soft)]={provider === "cloudflare"}
              class:border-[var(--color-border)]={provider !== "cloudflare"}
              aria-pressed={provider === "cloudflare"}
              disabled={!cloudflareAvailable}
              onclick={() => chooseProvider("cloudflare")}
            >
              <Cloud size={23} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("Cloudflare")}</span>
              <span class="mt-2 block text-sm text-[var(--color-text-secondary)]">
                {$t("Use a temporary Quick Tunnel or a stable Named Tunnel. The wizard can install cloudflared.")}
              </span>
            </button>
          </div>
          <button type="button" class="tx-btn-primary mt-6" disabled={!provider} onclick={() => (step = "workspace")}>
            {$t("Continue")}
          </button>
        </div>
      {:else if step === "workspace"}
        <div class="tx-card p-6 sm:p-8">
          <div class="flex items-start justify-between gap-4">
            <div>
              <FolderOpen size={23} class="text-[var(--primary)]" />
              <h3 class="mt-4 text-xl font-semibold">{$t("Name the workspace and choose project folders")}</h3>
              <p class="mt-2 max-w-xl text-sm text-[var(--color-text-secondary)]">
                {$t("One workspace can include multiple project folders. They share one connection while keeping separate project context.")}
              </p>
            </div>
            <button type="button" class="tx-btn-ghost" disabled={busy} onclick={goBack}><ArrowLeft size={16} /> {$t("Back")}</button>
          </div>
          {#if !isNodeHost}
            <label class="tx-field mt-6 max-w-xl">
              <span class="tx-label">{$t("Workspace name")}</span>
              <input class="tx-input" type="text" placeholder={$t("For example: Client projects")} bind:value={workspaceName} disabled={busy || Boolean(profile)} />
              <span class="text-[11px] text-[var(--color-text-muted)]">
                {$t("Leave blank to use the first selected folder name.")}
              </span>
            </label>
          {/if}
          {#if profile}
            <div class="tx-info-block mt-5">
              <p class="text-sm font-semibold">{profile.name}</p>
              <div class="mt-3 grid gap-2">
                {#each workspaceFolders(profile) as folder (folder.id)}
                  <p class="tx-mono break-all text-xs text-[var(--color-text-secondary)]">{folder.path}</p>
                {/each}
              </div>
            </div>
            <div class="mt-6 flex flex-wrap gap-2">
              <button type="button" class="tx-btn-primary" onclick={() => (step = "service")}>{$t("Continue with this workspace")}</button>
              {#if !isNodeHost}
                <button type="button" class="tx-btn-ghost" disabled={busy} onclick={() => void selectProjectFolders()}>{$t("Choose different folders")}</button>
              {/if}
            </div>
          {:else if isNodeHost}
            <button type="button" class="tx-btn-primary mt-6" disabled>{$t("Continue with this workspace")}</button>
          {:else}
            <button type="button" class="tx-btn-primary mt-6" disabled={busy} onclick={() => void selectProjectFolders()}>
              {busy ? $t("Creating workspace…") : $t("Select project folders")}
            </button>
          {/if}
        </div>
      {:else if step === "service"}
        <div class="tx-card p-6 sm:p-8">
          <div class="flex items-start justify-between gap-4">
            <div>
              <p class="tx-section-label">{$t("Connection method")}</p>
              <h3 class="text-xl font-semibold">{$t("How do you want to connect ChatGPT?")}</h3>
              <p class="mt-2 text-sm text-[var(--color-text-secondary)]">{$t("Choose one for this guided setup. You can enable the other later from the workspace page.")}</p>
            </div>
            <button type="button" class="tx-btn-ghost" onclick={goBack}><ArrowLeft size={16} /> {$t("Back")}</button>
          </div>
          <div class="mt-6 grid gap-3 md:grid-cols-2">
            <button
              type="button"
              class="rounded-xl border p-5 text-left hover:bg-[var(--color-surface-hover)]"
              class:border-[var(--primary)]={service === "mcp"}
              class:bg-[var(--primary-soft)]={service === "mcp"}
              class:border-[var(--color-border)]={service !== "mcp"}
              aria-pressed={service === "mcp"}
              onclick={() => (service = "mcp")}
            >
              <PlugZap size={22} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("MCP Connector")}</span>
              <span class="mt-1 block text-sm text-[var(--color-text-secondary)]">{$t("Recommended when your ChatGPT plan supports custom MCP connectors. Uses the public MCP endpoint and OAuth.")}</span>
            </button>
            <button
              type="button"
              class="rounded-xl border p-5 text-left hover:bg-[var(--color-surface-hover)]"
              class:border-[var(--primary)]={service === "actions"}
              class:bg-[var(--primary-soft)]={service === "actions"}
              class:border-[var(--color-border)]={service !== "actions"}
              aria-pressed={service === "actions"}
              disabled={!capabilities.actions}
              onclick={() => (service = "actions")}
            >
              <Workflow size={22} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("GPT Actions")}</span>
              <span class="mt-1 block text-sm text-[var(--color-text-secondary)]">{$t("Use this when building a custom GPT. Import the OpenAPI schema and configure a Bearer API key.")}</span>
            </button>
          </div>
          <button type="button" class="tx-btn-primary mt-6" disabled={!service} onclick={() => (step = "connect")}>{$t("Continue")}</button>
        </div>
      {:else if step === "connect" && profile && service && provider}
        <div class="tx-card p-6 sm:p-8">
          <div class="flex items-start justify-between gap-4">
            <div>
              <p class="tx-section-label">{provider === "builtin" ? $t("Built-in WSS tunnel") : provider === "frp" ? $t("FRP") : $t("Cloudflare")}</p>
              <h3 class="text-xl font-semibold">{$t("Prepare and enable {service}", { service: service === "mcp" ? $t("MCP") : $t("Actions") })}</h3>
              <p class="mt-2 text-sm text-[var(--color-text-secondary)]">{$t("The wizard checks every required value before it saves, tests, and starts the service.")}</p>
            </div>
            <button type="button" class="tx-btn-ghost" disabled={busy} onclick={goBack}><ArrowLeft size={16} /> {$t("Back")}</button>
          </div>
          <QuickTunnelSetup
            {provider}
            {service}
            workspaceId={profile.id}
            {busy}
            onEnable={verifyAndEnable}
          />
        </div>
      {:else if step === "complete" && profile && service && selectedActions}
        <div class="tx-card p-6 sm:p-8">
          <CircleCheck size={34} class="text-[var(--success)]" />
          <p class="page-kicker mt-5">{$t("Service enabled")}</p>
          <h3 class="mt-1 text-2xl font-semibold">{$t("Now finish the setup in ChatGPT")}</h3>
          <p class="mt-2 text-sm text-[var(--color-text-secondary)]">{$t("Use the steps and values below. Every value is generated for {workspace}.", { workspace: profile.name })}</p>
          <div class="mt-6 grid gap-4 lg:grid-cols-[0.8fr_1.2fr]">
            <article class="rounded-xl border border-[var(--color-border)] p-5">
              <p class="tx-section-label">{$t("ChatGPT steps")}</p>
              {#if service === "mcp"}
                <ol class="grid gap-4 text-sm">
                  <li><strong>1.</strong> {$t("Open ChatGPT Settings → Connectors, then create a custom MCP connector.")}</li>
                  <li><strong>2.</strong> {$t("Paste the Public MCP endpoint shown here and choose OAuth authentication.")}</li>
                  <li><strong>3.</strong> {$t("Expand Advanced OAuth settings, enter the Client ID shown here, leave Client Secret empty, and keep the other OAuth settings at their defaults.")}</li>
                  <li><strong>4.</strong> {$t("Select Next, click Connect, then enter the authorization password shown here.")}</li>
                </ol>
              {:else}
                <ol class="grid gap-4 text-sm">
                  <li><strong>1.</strong> {$t("Open the GPT editor → Configure → Actions, then create a new action.")}</li>
                  <li><strong>2.</strong> {$t("Choose Import from URL and paste the OpenAPI Schema URL shown here.")}</li>
                  <li><strong>3.</strong> {$t("Choose API Key authentication, set Auth Type to Bearer, and paste the generated key.")}</li>
                  <li><strong>4.</strong> {$t("Paste the privacy policy URL, save the GPT, and run a test action.")}</li>
                </ol>
              {/if}
            </article>
            <GptQuickCopy workspaceId={profile.id} {service} {profile} {publicMcpEndpoint} guidedMcp={service === "mcp"} />
          </div>
          <div class="mt-6 flex flex-wrap gap-2">
            <button type="button" class="tx-btn-primary" onclick={openWorkspace}>{$t("Open workspace")}</button>
            <button type="button" class="tx-btn-ghost" onclick={resetWizard}>{$t("Start another setup")}</button>
          </div>
        </div>
      {/if}
    </div>
  </div>
</section>
