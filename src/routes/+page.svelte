<script lang="ts">
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import EmptyState from "$lib/components/EmptyState.svelte";
  import { getBackend } from "$lib/backend";
  import { t } from "$lib/i18n";
  import { workspaces } from "$lib/stores/app";
  import { connectionActions, uiMode } from "$lib/stores/ui-mode";
  import { DEFAULT_SERVICE_PORT } from "$lib/types";

  const canCreate = getBackend().capabilities.workspaceLifecycle;
</script>

{#if $uiMode === "auto"}
  <section class="page-scroll sx-page">
    <div class="sx-card ax-glass sx-welcome">
      <h2 class="sx-title">{$t("Let any AI work on your project")}</h2>
      <p class="sx-hint">{$t("Add a folder, flip the switch, copy the prompt.")}</p>
      {#if canCreate}
        <button type="button" class="sx-copy" onclick={() => connectionActions.newConnection?.()}>
          <FolderOpen size={17} /> <span>{$t("Choose a project folder")}</span>
        </button>
      {/if}
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
