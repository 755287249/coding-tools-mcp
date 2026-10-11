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
    getMcpConnection,
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

  type WizardStep = "provider" | "service" | "connect" | "complete";
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
    { value: "service", label: "Connection" },
    { value: "connect", label: "Enable" },
    { value: "complete", label: "Finish" },
  ];

  let step = $state<WizardStep>("provider");
  let provider = $state<TunnelProvider | null>(cloudflareAvailable ? "cloudflare" : "builtin");
  let profile = $state<WorkspaceProfile | null>(null);
  let service = $state<SetupService | null>("mcp");
  let workspaceName = $state("");
  let runtimeStatus = $state<RuntimeStatus | null>(null);
  let busy = $state(false);
  let errorMessage = $state("");
  let loadingWorkspace = $state(true);

  const currentStepIndex = $derived(steps.findIndex((item) => item.value === step));
  const selectedActions = $derived(profile ? actionsConfig(profile) : null);
  const publicMcpEndpoint = $derived(profile ? runtimeStatus?.publicEndpoint || profile.tunnel.public_url : "");

  async function refreshWorkspaceStore(_workspaceId: string): Promise<WorkspaceProfile> {
    return getMcpConnection();
  }

  async function loadExistingWorkspace() {
    loadingWorkspace = true;
    try {
      profile=await getMcpConnection();
      const savedProvider=profile.tunnel.type;
      if ((savedProvider==='builtin' && /^https:\/\//i.test(profile.tunnel.public_url)) || (savedProvider==='frp'&&frpAvailable) || (savedProvider==='cloudflare'&&cloudflareAvailable)) provider=savedProvider;
    } finally { loadingWorkspace=false; }
  }

  async function resumeExistingMcp() {
    if (!profile || busy) return;
    busy = true;
    errorMessage = "";
    const workspaceId = profile.id;
    try {
      if (isNodeHost) await startTunnel(workspaceId, "mcp");
      const status = isNodeHost ? await getRuntimeStatus(workspaceId) : await startRuntime(workspaceId);
      if (status.state !== "running") throw new Error(status.localMessage || status.publicMessage || $t("The service failed to start"));
      const tunnel = await testTunnel(workspaceId, "mcp");
      if (!tunnel.success || !tunnel.publicUrl || !tunnel.keptRunning) throw new Error(tunnel.message || $t("The tunnel could not be verified."));
      runtimeStatus = status;
      profile = (await refreshWorkspaceStore(workspaceId)) ?? profile;
      service = "mcp";
      mcpRuntimeStates.update(current => ({ ...current, [workspaceId]: status.state }));
      step = "complete";
    } catch (error) {
      errorMessage = String(error);
    } finally {
      busy = false;
    }
  }

  function chooseProvider(nextProvider: TunnelProvider) {
    provider = nextProvider;
    errorMessage = "";
    step="service";
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
    else if (step === "service") step = "provider";
  }

  function openWorkspace() {
    void goto(appUrl('/connection'));
  }

  function resetWizard() {
    step = "provider";
    provider = cloudflareAvailable ? "cloudflare" : "builtin";
    service = "mcp";
    runtimeStatus = null;
    errorMessage = "";
    void loadExistingWorkspace().catch(error => errorMessage = String(error));
  }

  onMount(() => {
    void loadExistingWorkspace().catch(error => errorMessage = String(error));
  });
</script>

<section class="page-scroll">
  <header class="page-header">
    <div class="mx-auto max-w-4xl">
      <p class="page-kicker">{$t("Guided setup")}</p>
      <h2 class="page-title">{$t('connection.setupTitle')}</h2>
      <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-secondary)]">
        {$t('connection.setupHelp')}
      </p>
      <ol class="mt-6 grid grid-cols-4 gap-2" aria-label={$t("Setup progress")}>
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
          {#if profile && (profile.tunnel.type === 'cloudflare' || profile.tunnel.type === 'frp' || (profile.tunnel.type === 'builtin' && /^https:\/\//i.test(profile.tunnel.public_url)))}
            <div class="tx-info-block mt-5 flex flex-wrap items-center justify-between gap-3">
              <span class="text-sm">{profile.name}</span>
              <button type="button" class="tx-btn-primary" disabled={busy || loadingWorkspace} onclick={()=>void resumeExistingMcp()}>{busy ? $t('Testing and starting…') : $t('setup.resumeMcp')}</button>
            </div>
          {/if}
          <div class="mt-6 grid gap-3 md:grid-cols-3">
            <button
              type="button"
              class="setup-choice choice-cloudflare rounded-xl border p-5 text-left"
              class:border-[var(--primary)]={provider === "cloudflare"}
              class:bg-[var(--primary-soft)]={provider === "cloudflare"}
              class:border-[var(--color-border)]={provider !== "cloudflare"}
              aria-pressed={provider === "cloudflare"}
              disabled={!cloudflareAvailable || busy || loadingWorkspace}
              onclick={() => chooseProvider("cloudflare")}
            >
              {#if provider === "cloudflare"}<Check size={18} class="choice-check" aria-hidden="true"/>{/if}
              <Cloud size={23} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("Cloudflare")}</span>
              <span class="mt-2 block text-sm text-[var(--color-text-secondary)]">
                {$t("Use a temporary Quick Tunnel or a stable Named Tunnel. The wizard can install cloudflared.")}
              </span>
            </button>
            <button
              type="button"
              class="setup-choice rounded-xl border p-5 text-left"
              class:border-[var(--primary)]={provider === "builtin"}
              class:bg-[var(--primary-soft)]={provider === "builtin"}
              class:border-[var(--color-border)]={provider !== "builtin"}
              aria-pressed={provider === "builtin"}
              disabled={busy || loadingWorkspace}
              onclick={() => chooseProvider("builtin")}
            >
              {#if provider === "builtin"}<Check size={18} class="choice-check" aria-hidden="true"/>{/if}
              <ShieldCheck size={23} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("Built-in WSS tunnel")}</span>
              <span class="mt-2 block text-sm text-[var(--color-text-secondary)]">
                {$t("No extra software. Continue with a one-time invitation link from your server administrator.")}
              </span>
            </button>
            <button
              type="button"
              class="setup-choice choice-frp rounded-xl border p-5 text-left"
              class:border-[var(--primary)]={provider === "frp"}
              class:bg-[var(--primary-soft)]={provider === "frp"}
              class:border-[var(--color-border)]={provider !== "frp"}
              aria-pressed={provider === "frp"}
              disabled={!frpAvailable || busy || loadingWorkspace}
              onclick={() => chooseProvider("frp")}
            >
              {#if provider === "frp"}<Check size={18} class="choice-check" aria-hidden="true"/>{/if}
              <Server size={23} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("FRP")}</span>
              <span class="mt-2 block text-sm text-[var(--color-text-secondary)]">
                {$t("For a self-hosted or company FRP server. The wizard can install frpc and save the server profile.")}
              </span>
            </button>
          </div>
        </div>
      {:else if step === "service"}
        <div class="tx-card p-6 sm:p-8">
          <div class="flex items-start justify-between gap-4">
            <div>
              <p class="tx-section-label">{$t("Connection method")}</p>
              <h3 class="text-xl font-semibold">{$t("How do you want to connect ChatGPT?")}</h3>
              <p class="mt-2 text-sm text-[var(--color-text-secondary)]">{$t('connection.setupSelected')}</p>
            </div>
            <button type="button" class="tx-btn-ghost setup-back" onclick={goBack}><ArrowLeft size={16} /> {$t("Back")}</button>
          </div>
          <div class="mt-6 grid gap-3 md:grid-cols-2">
            <button
              type="button"
              class="setup-choice choice-mcp rounded-xl border p-5 text-left"
              class:border-[var(--primary)]={service === "mcp"}
              class:bg-[var(--primary-soft)]={service === "mcp"}
              class:border-[var(--color-border)]={service !== "mcp"}
              aria-pressed={service === "mcp"}
              onclick={() => {service="mcp";step="connect";}}
            >
              {#if service === "mcp"}<Check size={18} class="choice-check" aria-hidden="true"/>{/if}
              <PlugZap size={22} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("MCP Connector")}</span>
              <span class="mt-1 block text-sm text-[var(--color-text-secondary)]">{$t("Recommended when your ChatGPT plan supports custom MCP connectors. Uses the public MCP endpoint and OAuth.")}</span>
            </button>
            <button
              type="button"
              class="setup-choice choice-actions rounded-xl border p-5 text-left"
              class:border-[var(--primary)]={service === "actions"}
              class:bg-[var(--primary-soft)]={service === "actions"}
              class:border-[var(--color-border)]={service !== "actions"}
              aria-pressed={service === "actions"}
              disabled={!capabilities.actions}
              onclick={() => {service="actions";step="connect";}}
            >
              {#if service === "actions"}<Check size={18} class="choice-check" aria-hidden="true"/>{/if}
              <Workflow size={22} class="text-[var(--primary)]" />
              <span class="mt-4 block font-semibold">{$t("GPT Actions")}</span>
              <span class="mt-1 block text-sm text-[var(--color-text-secondary)]">{$t("Use this when building a custom GPT. Import the OpenAPI schema and configure a Bearer API key.")}</span>
            </button>
          </div>
        </div>
      {:else if step === "connect" && profile && service && provider}
        <div class="tx-card p-6 sm:p-8">
          <div class="flex items-start justify-between gap-4">
            <div>
              <p class="tx-section-label">{provider === "builtin" ? $t("Built-in WSS tunnel") : provider === "frp" ? $t("FRP") : $t("Cloudflare")}</p>
              <h3 class="text-xl font-semibold">{$t("Prepare and enable {service}", { service: service === "mcp" ? $t("MCP") : $t("Actions") })}</h3>
              <p class="mt-2 text-sm text-[var(--color-text-secondary)]">{$t("The wizard checks every required value before it saves, tests, and starts the service.")}</p>
            </div>
            <button type="button" class="tx-btn-ghost setup-back" disabled={busy} onclick={goBack}><ArrowLeft size={16} /> {$t("Back")}</button>
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
          <p class="mt-2 text-sm text-[var(--color-text-secondary)]">{$t('connection.setupComplete')}</p>
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
            <button type="button" class="tx-btn-primary" onclick={openWorkspace}>{$t('connection.openSettings')}</button>
            <button type="button" class="tx-btn-ghost" onclick={resetWizard}>{$t("Start another setup")}</button>
          </div>
        </div>
      {/if}
    </div>
  </div>
</section>

<style>
  .setup-choice{--choice-hue:255;position:relative;background:color-mix(in srgb,hsl(var(--choice-hue) 65% 65%) 8%,var(--card-bg));border-color:color-mix(in srgb,hsl(var(--choice-hue) 65% 65%) 25%,var(--color-border));transition:background 140ms,border-color 140ms,box-shadow 140ms}
  .choice-frp{--choice-hue:210}.choice-cloudflare{--choice-hue:32}.choice-mcp{--choice-hue:155}.choice-actions{--choice-hue:270}
  .setup-choice:hover:not(:disabled){background:color-mix(in srgb,hsl(var(--choice-hue) 65% 65%) 14%,var(--card-bg))}
  .setup-choice[aria-pressed=true]{background:color-mix(in srgb,hsl(var(--choice-hue) 65% 65%) 18%,var(--card-bg));border-color:hsl(var(--choice-hue) 65% 58%);box-shadow:0 0 0 1px hsl(var(--choice-hue) 65% 58%)}
  .setup-choice:disabled{opacity:.45}.setup-choice:focus-visible{outline:2px solid #3b82f6;outline-offset:4px}
  :global(.choice-check){position:absolute;top:16px;right:16px;color:hsl(var(--choice-hue) 65% 58%)}
  .setup-back{background:color-mix(in srgb,#3b82f6 18%,var(--card-bg));border-color:#3b82f680;color:var(--color-text)}
  @media(prefers-reduced-motion:reduce){.setup-choice{transition:none}}
</style>
