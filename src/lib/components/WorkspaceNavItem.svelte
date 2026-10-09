<script lang="ts">
  import { t } from "$lib/i18n";
  import { getBackend } from "$lib/backend";
  import ServiceStatusPair from "$lib/components/ServiceStatusPair.svelte";
  import StatusOrb from "$lib/components/StatusOrb.svelte";
  import { workspaceFolders, type RuntimeState, type WorkspaceProfile } from "$lib/types";

  interface Props {
    workspace: WorkspaceProfile;
    active: boolean;
    mcpState: RuntimeState;
    actionsState: RuntimeState;
    onClick: () => void;
  }

  let { workspace, active, mcpState, actionsState, onClick }: Props = $props();
  const capabilities = getBackend().capabilities;
  const folderCount = $derived(workspaceFolders(workspace).length);
</script>

<div class="tx-nav-item" class:active>
  <button
    type="button"
    class="tx-nav-button"
    onclick={onClick}
    title={workspace.name}
    aria-label={workspace.name}
    aria-current={active ? "page" : undefined}
  >
    {#if capabilities.actions}
      <ServiceStatusPair mcp={mcpState} actions={actionsState} />
    {:else}
      <StatusOrb state={mcpState} />
    {/if}
    <span class="tx-sidebar-text min-w-0 flex-1">
      <span class="block truncate text-[13px] font-medium leading-5">{workspace.name}</span>
      <span class="tx-nav-meta block truncate text-[11px] leading-4">
        {$t("{count} folders", { count: folderCount })}
      </span>
    </span>
  </button>
</div>
