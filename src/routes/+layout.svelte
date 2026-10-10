<script lang="ts">
  import "../app.css";
  import BrowserLogin from "$lib/components/BrowserLogin.svelte";
  import { browserAuthenticated, browserLogout } from "$lib/backend/browser-session";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
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
    getActionsRuntimeStatus,
    getRuntimeStatus,
    listWorkspaces,
  } from "$lib/api/workspaces";
  import { getLastWorkspaceId } from "$lib/api/settings";
  import { actionsRuntimeStates, mcpRuntimeStates, workspaces } from "$lib/stores/app";
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
  import Globe from "@lucide/svelte/icons/globe";
  import LogOut from "@lucide/svelte/icons/log-out";
  import type { RuntimeState } from "$lib/types";

  installHostBackend();

  let { children } = $props();
  const capabilities = getBackend().capabilities;
  const desktopWindow = isDesktopWindow();
  const remoteDesktop = capabilities.host === "desktop" && !desktopWindow;
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

    const mcpStates: Record<string, RuntimeState> = {};
    const actionsStates: Record<string, RuntimeState> = {};
    await Promise.all(
      items.map(async (item) => {
        try {
          const mcp = await getRuntimeStatus(item.id);
          mcpStates[item.id] = mcp.state;
        } catch {
          mcpStates[item.id] = "stopped";
        }
        if (!capabilities.actions) {
          actionsStates[item.id] = "stopped";
          return;
        }
        try {
          const actions = await getActionsRuntimeStatus(item.id);
          actionsStates[item.id] = actions.state;
        } catch {
          actionsStates[item.id] = "stopped";
        }
      }),
    );
    mcpRuntimeStates.set(mcpStates);
    actionsRuntimeStates.set(actionsStates);
  }

  async function finishAddWorkspace(selected: string) {
    const profile = await createWorkspace(selected);
    if (capabilities.agentRestart) {
      showToast($t("Workspace added. Restart the Agent to start its MCP listener."), { kind: "success" });
      await getBackend().agent.restart();
      window.setTimeout(() => window.location.reload(), 2500);
      return;
    }
    await refreshWorkspaces();
    const autostart = $uiMode === "auto" && capabilities.runtimeSupervisor ? "?autostart=1" : "";
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
    const currentPath = routePath($page.url.pathname);
    const workspaceMatch = currentPath.match(/^\/workspace\/([^/]+)$/);
    const target = workspaceMatch
      ? `/quick-setup?workspace=${encodeURIComponent(workspaceMatch[1]!)}`
      : "/quick-setup";
    goto(appUrl(target));
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
    if (remoteDesktop && !$browserAuthenticated) return;
    await refreshWorkspaces();
    if (capabilities.runtimeSupervisor) startSessionPolling();

  });
</script>

{#if remoteDesktop && !$browserAuthenticated}
  <BrowserLogin onLogin={refreshWorkspaces}/>
{:else}
<div class="win-root" class:is-desktop={desktopWindow}>
  <div class="win-body">
    <SimpleShell onQuickSetup={capabilities.guidedSetup ? openQuickSetup : undefined} onAddWorkspace={capabilities.workspaceLifecycle ? addWorkspace : undefined}>
        {#snippet settingsNav()}
          {#if remoteDesktop}<button type="button" class="tx-settings-link" onclick={browserLogout}><LogOut size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t("sharing.logout")}</span></button>{/if}
          <button type="button" class="tx-settings-link {routePath($page.url.pathname) === '/settings/updates' ? 'active' : ''}" onclick={()=>goto(appUrl("/settings/updates"))}><Download size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t("updates.title")}</span></button>
          <button type="button" class="tx-settings-link {routePath($page.url.pathname) === '/settings/sharing' ? 'active' : ''}" onclick={()=>goto(appUrl("/settings/sharing"))}><Globe size={15} aria-hidden="true"/><span class="tx-sidebar-text">{$t("sharing.title")}</span></button>
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
