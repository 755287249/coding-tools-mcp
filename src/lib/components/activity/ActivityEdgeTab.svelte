<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import ChevronLeft from "@lucide/svelte/icons/chevron-left";
  import { readWorkspaceActivity, type ActivityPlan } from "$lib/api/activity";
  import { t } from "$lib/i18n";
  import { planOverview } from "./plan";

  interface Props {
    workspaceId: string;
    /** MCP runtime is running (drives the poll rate). */
    live: boolean;
    onOpen: () => void;
  }

  let { workspaceId, live, onOpen }: Props = $props();

  let plan = $state<ActivityPlan | undefined>(undefined);
  let running = $state(0);
  let errors = $state(0);
  let rev = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;

  async function poll() {
    if (destroyed) return;
    try {
      const snapshot = await readWorkspaceActivity(workspaceId, rev);
      if (destroyed) return;
      rev = snapshot.rev;
      plan = snapshot.plan;
      running = snapshot.stats.running;
      errors = snapshot.stats.errors;
    } catch {
      // The tab is decorative; the drawer reports errors when opened.
    } finally {
      clearTimeout(timer);
      if (!destroyed) {
        const hidden = typeof document !== "undefined" && document.hidden;
        timer = setTimeout(() => void poll(), hidden ? 8000 : running > 0 ? 1500 : live ? 3000 : 8000);
      }
    }
  }

  onMount(() => void poll());
  onDestroy(() => {
    destroyed = true;
    clearTimeout(timer);
  });

  const overview = $derived(planOverview(plan));
  const label = $derived(
    overview.total > 0 ? `${$t("Expand task panel")} · ${overview.done}/${overview.total}` : $t("Expand task panel"),
  );
</script>

<button type="button" class="ad-edge-tab" class:is-busy={running > 0} onclick={onOpen} title={label} aria-label={label}>
  <ChevronLeft size={15} strokeWidth={2.4} />
  {#if running > 0}<span class="ad-edge-dot" aria-hidden="true"></span>{/if}
  {#if overview.total > 0}
    <svg class="ad-edge-meter" viewBox="0 0 4 40" preserveAspectRatio="none" aria-hidden="true">
      <rect class="ad-meter-track" x="0" y="0" width="4" height="40" rx="2" />
      <rect
        class="ad-meter-fill"
        class:is-done={overview.finished}
        x="0"
        y={40 - (overview.pct / 100) * 40}
        width="4"
        height={(overview.pct / 100) * 40}
        rx="2"
      />
    </svg>
    <span class="ad-edge-count">{`${overview.done}/${overview.total}`}</span>
  {:else if errors > 0}
    <span class="ad-edge-err" aria-hidden="true">{errors}</span>
  {/if}
</button>
