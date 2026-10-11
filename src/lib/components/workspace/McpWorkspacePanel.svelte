<script lang="ts">
  import {goto} from "$app/navigation";
  import {appUrl} from "$lib/app-path";
  import {GLOBAL_MCP_ID} from "$lib/api/workspaces";
  import AuthConfigForm, { type SaveAuthOptions } from "$lib/components/AuthConfigForm.svelte";
  import GptQuickCopy from "$lib/components/GptQuickCopy.svelte";
  import HealthPanel from "$lib/components/HealthPanel.svelte";
  import LogViewer from "$lib/components/LogViewer.svelte";
  import RuntimePolicyForm, { type RuntimePolicyDraft } from "$lib/components/RuntimePolicyForm.svelte";
  import ServicePanel from "$lib/components/ServicePanel.svelte";
  import Tabs from "$lib/components/Tabs.svelte";
  import TunnelConfigForm, {
    type SaveTunnelOptions,
    type TunnelFormConfig,
  } from "$lib/components/TunnelConfigForm.svelte";
  import type { FrpProfileDto } from "$lib/api/settings";
  import { getBackend } from "$lib/backend";
  import { t } from "$lib/i18n";
  import {
    mcpLocalEndpoint,
    securityPolicyConfig,
    type AuthConfig,
    type RuntimeState,
    type WorkspaceProfile,
  } from "$lib/types";

  export type ServiceSection = "service" | "tunnel" | "auth" | "policy" | "logs" | "health";

  interface Props {
    workspaceId: string;
    profile: WorkspaceProfile;
    section: ServiceSection;
    status: RuntimeState;
    statusMessage: string;
    localEndpoint: string;
    publicEndpoint: string;
    busy: boolean;
    frpProfiles: FrpProfileDto[];
    onSectionChange: (section: ServiceSection) => void;
    onToggle: () => void | Promise<void>;
    onPortChange: (port: number) => void | Promise<void>;
    onBindAddressChange: (address: string) => void | Promise<void>;
    onSaveTunnel: (config: TunnelFormConfig, options?: SaveTunnelOptions) => void | Promise<void>;
    onSavePolicy: (draft: RuntimePolicyDraft) => void | Promise<void>;
    onSaveAuth: (auth: AuthConfig, options?: SaveAuthOptions) => void | Promise<void>;
  }

  let {
    workspaceId,
    profile,
    section,
    status,
    statusMessage,
    localEndpoint,
    publicEndpoint,
    busy,
    frpProfiles,
    onSectionChange,
    onToggle,
    onPortChange,
    onBindAddressChange,
    onSaveTunnel,
    onSavePolicy,
    onSaveAuth,
  }: Props = $props();

  const capabilities = getBackend().capabilities;
  const tabs = $derived(
    [
      { value: "service", label: $t("Service") },
      { value: "tunnel", label: $t("Tunnel") },
      { value: "auth", label: $t("Authentication") },
      workspaceId!==GLOBAL_MCP_ID ? { value: "policy", label: $t("Policy") } : null,
      capabilities.rawRuntimeLogs ? { value: "logs", label: $t("Logs") } : null,
      { value: "health", label: $t("Health") },
    ].filter((item): item is { value: string; label: string } => item != null),
  );

  const tunnelForm = $derived<TunnelFormConfig>({
    type: profile.tunnel.type ?? "none",
    public_url: profile.tunnel.public_url ?? "",
    frp_server: profile.tunnel.frp_server ?? "",
    frp_subdomain: profile.tunnel.frp_subdomain ?? "",
    frp_profile_id: profile.tunnel.frp_profile_id ?? "",
    frp_server_port: profile.tunnel.frp_server_port ?? 443,
    cloudflare_mode: profile.tunnel.cloudflare_mode ?? "quick",
    use_proxy: profile.tunnel.use_proxy ?? true,
  });
</script>

<Tabs
  items={tabs}
  value={section}
  idPrefix="mcp-tabs"
  ariaLabel={$t("MCP features")}
  onchange={(value) => onSectionChange(value as ServiceSection)}
/>

<div
  class="tx-tabpanel mt-4"
  role="tabpanel"
  id={`mcp-tabs-panel-${section}`}
  aria-labelledby={`mcp-tabs-tab-${section}`}
  tabindex="0"
>
  {#if workspaceId!==GLOBAL_MCP_ID && section!=="policy" && section!=="logs"}
    <section class="tx-card p-5"><p>{$t('connection.projectMcp')}</p><button class="tx-btn-primary mt-4" onclick={()=>void goto(appUrl('/connection'))}>{$t('connection.globalSettings')}</button></section>
  {:else if section === "service"}
    <div class="flex flex-col gap-3">
      <ServicePanel
        title="MCP"
        subtitle={`Streamable HTTP · ${$t("Tool runtime")}`}
        {status}
        {statusMessage}
        port={profile.runtime.local_port}
        portEditable={capabilities.runtimeSupervisor}
        bindAddress={profile.runtime.bind_address ?? "127.0.0.1"}
        bindAddressEditable={capabilities.runtimeSupervisor}
        {busy}
        toggleable={capabilities.runtimeSupervisor}
        tunnelType={profile.tunnel.type}
        localEndpoint={localEndpoint || mcpLocalEndpoint(profile.runtime.local_port, profile.runtime.bind_address)}
        {publicEndpoint}
        publicLabel={$t("Public MCP")}
        {onToggle}
        {onPortChange}
        {onBindAddressChange}
      />
      <GptQuickCopy
        {workspaceId}
        service="mcp"
        {profile}
        publicMcpEndpoint={publicEndpoint}
        {frpProfiles}
      />
    </div>
  {:else if section === "tunnel"}
    <section class="tx-card p-5">
      <p class="tx-section-label">{$t("MCP tunnel")}</p>
      <div class="mt-3">
        <TunnelConfigForm {workspaceId} service="mcp" config={tunnelForm} onSave={onSaveTunnel} />
      </div>
    </section>
  {:else if section === "auth"}
    <section class="tx-card p-5">
      <p class="tx-section-label">{$t("MCP authentication")}</p>
      <div class="mt-3">
        <AuthConfigForm {workspaceId} auth={profile.auth} onSaveProfile={onSaveAuth} />
      </div>
    </section>
  {:else if section === "policy"}
    <section class="tx-card p-5">
      <p class="tx-section-label">{$t("MCP policy")}</p>
      <div class="mt-3">
        <RuntimePolicyForm
          transportMode={profile.runtime.transport_mode ?? "streamable-http"}
          securityPolicy={securityPolicyConfig(profile.runtime)}
          allowedCommands={profile.runtime.allowed_commands ?? ""}
          workspaceLocalEntries={profile.runtime.workspace_local_entries ?? true}
          workspaceScriptExtensions={profile.runtime.workspace_script_extensions ?? ".exe,.bat,.cmd,.ps1"}
          blockingAdmissionLimit={profile.runtime.blocking_admission_limit ?? 8}
          processAdmissionLimit={profile.runtime.process_admission_limit ?? 4}
          globalBlockingAdmissionLimit={profile.runtime.global_blocking_admission_limit ?? 16}
          globalProcessAdmissionLimit={profile.runtime.global_process_admission_limit ?? 8}
          activeSessionLimit={profile.runtime.active_session_limit ?? 16}
          onSave={onSavePolicy}
        />
      </div>
    </section>
  {:else if section === "logs" && capabilities.rawRuntimeLogs}
    <LogViewer {workspaceId} service="mcp" />
  {:else}
    <HealthPanel {workspaceId} />
  {/if}
</div>
