<script lang="ts">
  import "../app.css";
  import BrowserLogin from "$lib/components/BrowserLogin.svelte";
  import { browserAuthenticated, browserLogout } from "$lib/backend/browser-session";
  import { onMount, onDestroy } from "svelte";
  import { goto, afterNavigate } from "$app/navigation";
  import { resetActivityPanel } from "$lib/stores/activity-panel";
  import { page } from "$app/stores";
  import { appUrl, routePath } from "$lib/app-path";
  import { pickDirectory, confirm } from "$lib/api/native";
  import { installHostBackend } from "$lib/backend/host";
  import { getBackend } from "$lib/backend";
  import AppShell from "$lib/components/AppShell.svelte";
  import GlassControl from "$lib/components/GlassControl.svelte";
  import SimpleShell from "$lib/components/SimpleShell.svelte";
  import { isDesktopWindow } from "$lib/stores/glass";
  import { startSessionPolling } from "$lib/stores/sessions";
  import DirectoryPicker from "$lib/components/DirectoryPicker.svelte";
  import ToastHost from "$lib/components/ToastHost.svelte";
  import WorkspaceNavItem from "$lib/components/WorkspaceNavItem.svelte";
  import {
    createWorkspace,
    getMcpConnection,
    getActionsRuntimeStatus,
    getRuntimeStatus,
    listWorkspaces,
  } from "$lib/api/workspaces";
  import { getLastWorkspaceId } from "$lib/api/settings";
  import { startupSetupTarget } from "$lib/quick-setup/startup";
  import { actionsRuntimeStates, mcpRuntimeStates, workspaces, mcpConnection } from "$lib/stores/app";
  import { showToast } from "$lib/stores/toast";
  import { t } from "$lib/i18n";
  import { connectionActions, uiMode } from "$lib/stores/ui-mode";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Network from "@lucide/svelte/icons/network";
  import Package from "@lucide/svelte/icons/package";
  import RotateCw from "@lucide/svelte/icons/rotate-cw";
  import Search from "@lucide/svelte/icons/search";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import Download from "@lucide/svelte/icons/download";
  import LogOut from "@lucide/svelte/icons/log-out";
  import type { RuntimeState } from "$lib/types";

  installHostBackend();
  afterNavigate(resetActivityPanel);

  let { children } = $props();
  const capabilities = getBackend().capabilities;
  const desktopWindow = isDesktopWindow();
  const remoteDesktop = capabilities.host === "desktop" && !desktopWindow;
  let setupPrompt=$state(false);
  function showSetupPrompt(node:HTMLDialogElement){node.showModal();return{destroy:()=>node.close()};}
  let stopSessionPolling:(()=>void)|undefined;
  onDestroy(()=>stopSessionPolling?.());
  let addWorkspacePickerOpen = $state(false);
  let workspaceFilter = $state("");
  const WORKSPACE_FILTER_THRESHOLD = 6;
  const filteredWorkspaces = $derived.by(() => {
    const query = workspaceFilter.trim().toLowerCase();
    if (!query) return $workspaces;
    return $workspaces.filter((item) => item.name.toLowerCase().includes(query));
  });

  async function refreshWorkspaces() {
    const items = await listWorkspaces();
    workspaces.set(items);

    const connection=await getMcpConnection();
    const runtime=await getRuntimeStatus(connection.id);
    mcpRuntimeStates.set(Object.fromEntries([connection,...items].map(item=>[item.id,runtime.state])));
    const actionsStates:Record<string,RuntimeState>={};
    if(capabilities.actions)await Promise.all(items.map(async item=>{try{actionsStates[item.id]=(await getActionsRuntimeStatus(item.id)).state;}catch{actionsStates[item.id]='error';}}));
    actionsRuntimeStates.set(actionsStates);
    return {items,connectionState:runtime.state,connectionConfigured:Boolean(connection.tunnel.public_url?.trim())};
  }

  async function initializeWorkspaceNavigation() {
    const initialUrl = $page.url.href;
    try {
      const { items, connectionState, connectionConfigured } = await refreshWorkspaces();
      if (capabilities.runtimeSupervisor && !stopSessionPolling) stopSessionPolling=startSessionPolling();
      if (!capabilities.guidedSetup) return;
      // A slow status request must not override navigation the user already made.
      if ($page.url.href !== initialUrl) return;
      const path=routePath($page.url.pathname);
      if(items.length===0 && (!connectionConfigured||connectionState==='stopped'||connectionState==='error') && (path==='/'||path.startsWith('/workspace/')))setupPrompt=true;
    } catch (error) {
      showToast(String(error), { kind: "error" });
    }
  }

  async function finishAddWorkspace(selected: string) {
    const profile = await createWorkspace(selected);
    await refreshWorkspaces();
    const autostart = "";
    goto(appUrl(`/workspace/${profile.id}${autostart}`));
  }

  async function addWorkspace() {
    try {
      if (!capabilities.nativeDirectoryPicker) {
        addWorkspacePickerOpen = true;
        return;
      }
      const selected = await pickDirectory({ multiple: false });
      if (!selected || Array.isArray(selected)) return;
      await finishAddWorkspace(selected);
    } catch (error) {
      showToast(String(error), {
        title: $t("Failed to add workspace"),
        kind: "error",
        duration: 8000,
      });
    }
  }

  function openWorkspace(id: string) {
    goto(appUrl(`/workspace/${id}`));
  }

  function openQuickSetup() {
    setupPrompt=false;
    void goto(appUrl('/quick-setup'));
  }

  function openFrpSettings() {
    goto(appUrl("/settings/frp"));
  }

  function openSoftwareSettings() {
    goto(appUrl("/settings/software"));
  }

  function openGeneralSettings() {
    goto(appUrl("/settings/general"));
  }

  function openKeysSettings() {
    goto(appUrl("/settings/keys"));
  }

  async function restartAgent() {
    try {
      const confirmed = await confirm(
        $t("Restart the Agent now? Active tool calls and command sessions will be stopped."),
        { kind: "warning", title: $t("Restart Agent") },
      );
      if (!confirmed) return;
      await getBackend().agent.restart();
      window.setTimeout(() => window.location.reload(), 2500);
    } catch (error) {
      showToast(String(error), { title: $t("Agent restart failed"), kind: "error", duration: 8000 });
    }
  }

  connectionActions.newConnection = () => void addWorkspace();

  onMount(async () => {
    // The shell is mounted; initial workspace requests must not trap users
    // behind the pre-JavaScript loading screen if a backend is slow or offline.
    document.getElementById("startup-splash")?.remove();
    if (remoteDesktop && !$browserAuthenticated) return;
    await initializeWorkspaceNavigation();
  });
</script>

{#if remoteDesktop && !$browserAuthenticated}
  <BrowserLogin onLogin={initializeWorkspaceNavigation}/>
{:else}
{#if setupPrompt}<dialog use:showSetupPrompt class="setup-prompt" aria-labelledby="setup-prompt-title" oncancel={()=>setupPrompt=false}><div class="tx-card p-8"><h2 id="setup-prompt-title" class="text-xl font-semibold">{$t('connection.firstTitle')}</h2><p class="mt-3 text-sm text-[var(--color-text-secondary)]">{$t('connection.firstHelp')}</p><div class="mt-6 flex justify-end gap-3"><button class="tx-btn-ghost" onclick={()=>setupPrompt=false}>{$t('connection.later')}</button><button class="tx-btn-primary" onclick={openQuickSetup}>{$t('connection.begin')}</button></div></div></dialog>{/if}
<div class="win-root" class:is-desktop={desktopWindow}>
  <div class="win-body">
    <SimpleShell onQuickSetup={capabilities.guidedSetup ? openQuickSetup : undefined} onAddWorkspace={capabilities.workspaceLifecycle ? addWorkspace : undefined}>
        {#snippet settingsNav()}
          {#if remoteDesktop}<button type="button" class="tx-settings-link" onclick={browserLogout}><LogOut size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t("sharing.logout")}</span></button>{/if}
          <button type="button" class="tx-settings-link {routePath($page.url.pathname) === '/settings/updates' ? 'active' : ''}" onclick={()=>goto(appUrl("/settings/updates"))}><Download size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t("updates.title")}</span></button>
          {#if capabilities.host === "desktop"}
            <button
              type="button"
              class="tx-settings-link {routePath($page.url.pathname) === '/settings/general' ? 'active' : ''}"
              onclick={openGeneralSettings}
              title={$t("General")}
            >
              <SlidersHorizontal size={15} aria-hidden="true" />
              <span class="tx-sidebar-text">{$t("General")}</span>
            </button>
          {/if}
          {#if capabilities.sharedSecretStore}
            <button
              type="button"
              class="tx-settings-link {routePath($page.url.pathname) === '/settings/keys' ? 'active' : ''}"
              onclick={openKeysSettings}
              title={$t("Shared secrets")}
            >
              <KeyRound size={15} aria-hidden="true" />
              <span class="tx-sidebar-text">{$t("Shared secrets")}</span>
            </button>
          {/if}
          {#if capabilities.frpManagement}
            <button
              type="button"
              class="tx-settings-link {routePath($page.url.pathname) === '/settings/frp' ? 'active' : ''}"
              onclick={openFrpSettings}
              title={$t("FRP configuration")}
            >
              <Network size={15} aria-hidden="true" />
              <span class="tx-sidebar-text">{$t("FRP configuration")}</span>
            </button>
          {/if}
          {#if capabilities.softwareManagement}
            <button
              type="button"
              class="tx-settings-link {routePath($page.url.pathname) === '/settings/software' ? 'active' : ''}"
              onclick={openSoftwareSettings}
              title={$t("Software management")}
            >
              <Package size={15} aria-hidden="true" />
              <span class="tx-sidebar-text">{$t("Software management")}</span>
            </button>
          {/if}
          {#if capabilities.agentRestart}
            <button
              type="button"
              class="tx-settings-link"
              onclick={() => void restartAgent()}
              title={$t("Restart Agent")}
            >
              <RotateCw size={15} aria-hidden="true" />
              <span class="tx-sidebar-text">{$t("Restart Agent")}</span>
            </button>
          {/if}
        {/snippet}
      {@render children()}
    </SimpleShell>
  </div>
  {#if desktopWindow}
    <GlassControl />
  {/if}
</div>

<DirectoryPicker
  open={addWorkspacePickerOpen}
  onCancel={() => (addWorkspacePickerOpen = false)}
  onSelect={async (path) => {
    addWorkspacePickerOpen = false;
    try {
      await finishAddWorkspace(path);
    } catch (error) {
      showToast(String(error), {
        title: $t("Failed to add workspace"),
        kind: "error",
        duration: 8000,
      });
    }
  }}
/>

<ToastHost />

{/if}

<style>.setup-prompt{position:fixed;margin:auto;width:min(480px,calc(100vw - 40px));padding:0;border:0;border-radius:16px;background:transparent;color:var(--color-text)}.setup-prompt::backdrop{background:#0007}</style>
