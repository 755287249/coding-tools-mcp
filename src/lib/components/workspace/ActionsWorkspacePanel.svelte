<script lang="ts">
  import {goto} from "$app/navigation";
  import {appUrl} from "$lib/app-path";
  import {GLOBAL_MCP_ID} from "$lib/api/workspaces";
  import ActionsAuthForm from "$lib/components/ActionsAuthForm.svelte";
  import ActionsPolicyForm, { type ActionsPolicyDraft } from "$lib/components/ActionsPolicyForm.svelte";
  import GptQuickCopy from "$lib/components/GptQuickCopy.svelte";
  import HealthPanel from "$lib/components/HealthPanel.svelte";
  import LogViewer from "$lib/components/LogViewer.svelte";
  import ServicePanel from "$lib/components/ServicePanel.svelte";
  import Tabs from "$lib/components/Tabs.svelte";
  import TunnelConfigForm, {
    type SaveTunnelOptions,
    type TunnelFormConfig,
  } from "$lib/components/TunnelConfigForm.svelte";
  import type { FrpProfileDto } from "$lib/api/settings";
  import { t } from "$lib/i18n";
  import {
    actionsConfig,
    actionsLocalEndpoint,
    actionsOAuthAuthorizeUrl,
    actionsOAuthTokenUrl,
    actionsOpenApiUrl,
    actionsPrivacyUrl,
    type ActionsAuthDraft,
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
    onSavePolicy: (draft: ActionsPolicyDraft) => void | Promise<void>;
    onSaveAuth: (draft: ActionsAuthDraft) => void | Promise<void>;
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

  const actions = $derived(actionsConfig(profile));
  const tabs = $derived([
    { value: "service", label: $t("Service") },
    { value: "tunnel", label: $t("Tunnel") },
    { value: "auth", label: $t("Authentication") },
    { value: "logs", label: $t("Logs") },
    { value: "health", label: $t("Health") },
  ]);

  const tunnelForm = $derived<TunnelFormConfig>({
    type: actions.tunnel_type ?? "none",
    public_url: actions.public_url ?? "",
    frp_server: actions.frp_server ?? "",
    frp_subdomain: actions.frp_subdomain ?? "",
    frp_profile_id: actions.frp_profile_id ?? "",
    frp_server_port: actions.frp_server_port ?? 443,
    cloudflare_mode: actions.cloudflare_mode ?? "quick",
    use_proxy: actions.use_proxy ?? true,
  });
</script>

<Tabs
  items={tabs}
  value={section}
  idPrefix="actions-tabs"
  ariaLabel={$t("Actions features")}
  onchange={(value) => onSectionChange(value as ServiceSection)}
/>

<div
  class="tx-tabpanel mt-4"
  role="tabpanel"
  id={`actions-tabs-panel-${section}`}
  aria-labelledby={`actions-tabs-tab-${section}`}
  tabindex="0"
>
  {#if workspaceId!==GLOBAL_MCP_ID}
    <section class="tx-card p-5"><p>{$t('connection.projectActions')}</p><button class="tx-btn-primary mt-4" onclick={()=>void goto(appUrl('/connection?service=actions'))}>{$t('connection.globalSettings')}</button></section>
  {:else if section === "service"}
    <div class="flex flex-col gap-3">
      <ServicePanel
        title="Actions"
        subtitle={$t("OpenAPI gateway · ChatGPT Actions")}
        {status}
        {statusMessage}
        port={actions.local_port}
        portEditable={true}
        bindAddress={actions.bind_address ?? "127.0.0.1"}
        bindAddressEditable={true}
        {busy}
        tunnelType={actions.tunnel_type}
        localEndpoint={localEndpoint || actionsLocalEndpoint(actions.local_port, actions.bind_address)}
        publicEndpoint={publicEndpoint || actionsOpenApiUrl(profile, frpProfiles)}
        publicLabel="OpenAPI"
        {onToggle}
        {onPortChange}
        {onBindAddressChange}
      />
      <GptQuickCopy {workspaceId} service="actions" {profile} {frpProfiles} />
    </div>
  {:else if section === "tunnel"}
    <section class="tx-card p-5">
      <p class="tx-section-label">{$t("Actions tunnel")}</p>
      <div class="mt-3">
        <TunnelConfigForm {workspaceId} service="actions" config={tunnelForm} onSave={onSaveTunnel} />
      </div>
    </section>
  {:else if section === "auth"}
    <section class="tx-card p-5">
      <p class="tx-section-label">{$t("Actions authentication")}</p>
      <div class="mt-3">
        <ActionsAuthForm
          {workspaceId}
          authType={actions.auth_type}
          oauthClientId={actions.oauth_client_id ?? ""}
          oauthScopes={actions.oauth_scopes ?? ""}
          openapiUrl={actionsOpenApiUrl(profile, frpProfiles)}
          privacyUrl={actionsPrivacyUrl(profile, frpProfiles)}
          oauthAuthorizeUrl={actionsOAuthAuthorizeUrl(profile, frpProfiles)}
          oauthTokenUrl={actionsOAuthTokenUrl(profile, frpProfiles)}
          useSharedSecrets={actions.use_shared_secrets ?? false}
          onSave={onSaveAuth}
        />
      </div>
    </section>
  {:else if section === "policy"}
    <section class="tx-card p-5">
      <p class="tx-section-label">{$t("Actions policy")}</p>
      <div class="mt-3">
        <ActionsPolicyForm
          allowedCommands={actions.allowed_commands ?? ""}
          maxPatchBytes={actions.max_patch_bytes ?? 200_000}
          permissionMode={actions.permission_mode}
          onSave={onSavePolicy}
        />
      </div>
    </section>
  {:else if section === "logs"}
    <LogViewer {workspaceId} service="actions" />
  {:else}
    <HealthPanel {workspaceId} />
  {/if}
</div>
