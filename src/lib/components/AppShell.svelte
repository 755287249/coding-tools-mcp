<script lang="ts">
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import PanelLeftClose from "@lucide/svelte/icons/panel-left-close";
  import PanelLeftOpen from "@lucide/svelte/icons/panel-left-open";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import SquareTerminal from "@lucide/svelte/icons/square-terminal";
  import LanguageSelect from "$lib/components/LanguageSelect.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import { APP_VERSION } from "$lib/app-version";
  import { t } from "$lib/i18n";
  import { onMount, type Snippet } from "svelte";

  interface Props {
    children: Snippet;
    sidebar: Snippet;
    onAddWorkspace?: () => void | Promise<void>;
    onQuickSetup?: () => void | Promise<void>;
    settingsNav?: Snippet;
    workspaceCount?: number;
  }

  let { children, sidebar, onAddWorkspace, onQuickSetup, settingsNav, workspaceCount }: Props = $props();

  const COLLAPSE_KEY = "sidebar-collapsed";
  let collapsed = $state(false);

  onMount(() => {
    try {
      collapsed = localStorage.getItem(COLLAPSE_KEY) === "1";
    } catch {
      collapsed = false;
    }
  });

  function toggleSidebar() {
    collapsed = !collapsed;
    try {
      localStorage.setItem(COLLAPSE_KEY, collapsed ? "1" : "0");
    } catch {
      // Storage may be unavailable; the toggle still works for this session.
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "b") {
      event.preventDefault();
      toggleSidebar();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="app-layout" class:sidebar-collapsed={collapsed}>
  <aside class="tx-sidebar">
    <div class="tx-sidebar-header">
      <div class="tx-brand">
        <span class="tx-brand-mark" aria-hidden="true">
          <SquareTerminal size={17} strokeWidth={2.2} />
        </span>
        <div class="tx-brand-text">
          <p class="tx-brand-kicker">Coding Tools</p>
          <h1 class="tx-brand-title">{$t("Desktop Console")}</h1>
        </div>
        <button
          type="button"
          class="tx-icon-btn"
          onclick={toggleSidebar}
          title={`${$t(collapsed ? "Expand sidebar" : "Collapse sidebar")} (Ctrl+B)`}
          aria-label={$t(collapsed ? "Expand sidebar" : "Collapse sidebar")}
          aria-expanded={!collapsed}
        >
          {#if collapsed}
            <PanelLeftOpen size={16} />
          {:else}
            <PanelLeftClose size={16} />
          {/if}
        </button>
      </div>
      {#if onQuickSetup}
        <button
          type="button"
          class="tx-sidebar-action tx-sidebar-action--primary"
          onclick={onQuickSetup}
          title={$t("Quick setup")}
        >
          <Sparkles size={15} aria-hidden="true" />
          <span class="tx-sidebar-text">{$t("Quick setup")}</span>
        </button>
      {/if}
      {#if onAddWorkspace}
        <button type="button" class="tx-sidebar-action" onclick={onAddWorkspace} title={$t("Add workspace")}>
          <FolderPlus size={15} aria-hidden="true" />
          <span class="tx-sidebar-text">{$t("Add workspace")}</span>
        </button>
      {/if}
    </div>

    <div class="tx-sidebar-body">
      {#if onAddWorkspace}
        <p class="tx-sidebar-section-label">
          <span>{$t("Workspaces")}</span>
          {#if workspaceCount !== undefined}
            <span class="tx-sidebar-count">{workspaceCount}</span>
          {/if}
        </p>
      {/if}
      {@render sidebar()}
    </div>

    <div class="tx-sidebar-footer">
      {#if settingsNav}
        <p class="tx-sidebar-section-label"><span>{$t("Settings")}</span></p>
        {@render settingsNav()}
      {/if}
      <div class="tx-sidebar-toolbar">
        <LanguageSelect />
        <ThemeToggle />
        <p class="tx-app-version">v{APP_VERSION}</p>
      </div>
    </div>
  </aside>

  <main class="tx-main">
    {@render children()}
  </main>
</div>

<svelte:head>
  <title>Coding Tools MCP</title>
</svelte:head>
