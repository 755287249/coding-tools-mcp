<script lang="ts">
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Globe from "@lucide/svelte/icons/globe";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import { getBackend } from "$lib/backend";
  import { t } from "$lib/i18n";
  import { workspaces } from "$lib/stores/app";
  import { connectionActions, uiMode } from "$lib/stores/ui-mode";
  import { DEFAULT_SERVICE_PORT } from "$lib/types";

  const canCreate = getBackend().capabilities.workspaceLifecycle;
</script>

{#if $uiMode === "auto"}
  <section class="page-scroll ax-page">
    <div class="ax-container ax-welcome">
      <p class="page-kicker">Coding Tools MCP</p>
      <h2 class="ax-welcome-title">{$t("Let any AI work on your project")}</h2>
      <p class="ax-hero-desc ax-welcome-desc">
        {$t("Pick a project folder. Everything else is automatic: the service starts, a secure public address is created, and you get a prompt to paste into your AI assistant.")}
      </p>

      {#if canCreate}
        <button type="button" class="tx-btn-primary ax-btn-lg ax-welcome-cta" onclick={() => connectionActions.newConnection?.()}>
          <FolderOpen size={16} /> {$t("Choose a project folder")}
        </button>
      {/if}

      <ol class="ax-welcome-steps">
        <li class="ax-glass">
          <span class="ax-welcome-num">1</span>
          <FolderOpen size={18} class="ax-welcome-icon" />
          <p class="font-medium">{$t("Choose a folder")}</p>
          <p class="ax-welcome-sub">{$t("The project the AI can read and edit")}</p>
        </li>
        <li class="ax-glass">
          <span class="ax-welcome-num">2</span>
          <Globe size={18} class="ax-welcome-icon" />
          <p class="font-medium">{$t("Goes online automatically")}</p>
          <p class="ax-welcome-sub">{$t("MCP service and secure tunnel start for you")}</p>
        </li>
        <li class="ax-glass">
          <span class="ax-welcome-num">3</span>
          <Sparkles size={18} class="ax-welcome-icon" />
          <p class="font-medium">{$t("Copy the prompt")}</p>
          <p class="ax-welcome-sub">{$t("Paste into ChatGPT, Claude or any AI")}</p>
        </li>
      </ol>
    </div>
  </section>
{:else}
  <div class="page-scroll flex flex-col items-center justify-center p-8">
    {#if $workspaces.length === 0}
      <EmptyState />
      <p class="mt-6 max-w-md text-center text-sm text-[var(--color-text-muted)]">
        {$t(
          "Each workspace can run MCP and Actions as independent local services. The default port is {port}; you can change it on the workspace page.",
          { port: DEFAULT_SERVICE_PORT },
        )}
      </p>
    {:else}
      <div class="max-w-lg text-center">
        <p class="text-[11px] font-medium uppercase tracking-[0.18em] text-[var(--color-text-muted)]">
          {$t("Get started")}
        </p>
        <h2 class="mt-2 text-2xl font-semibold">{$t("Select a workspace from the sidebar")}</h2>
        <p class="mt-3 text-sm leading-relaxed text-[var(--color-text-secondary)]">
          {$t(
            "The console uses sidebar navigation and separate service panels. MCP and Actions listen on independent ports and check for conflicts before starting.",
          )}
        </p>
      </div>
    {/if}
  </div>
{/if}
