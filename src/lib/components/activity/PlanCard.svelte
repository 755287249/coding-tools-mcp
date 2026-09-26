<script lang="ts">
  import Play from "@lucide/svelte/icons/play";
  import Target from "@lucide/svelte/icons/target";
  import type { ActivityPlan } from "$lib/api/activity";
  import { formatDuration } from "$lib/activity/diff";
  import { t } from "$lib/i18n";
  import { planOverview, TODO_GLYPH } from "./plan";

  interface Props {
    plan: ActivityPlan;
    /** A tool call is running right now (animates the current step). */
    busy: boolean;
    now: number;
  }

  let { plan, busy, now }: Props = $props();

  let showAll = $state(false);
  const COLLAPSED_ROWS = 8;

  const overview = $derived(planOverview(plan));
  const progress = $derived(plan.progress);
  const hasChecklist = $derived(overview.total > 0);
  const statusKind = $derived(overview.finished ? "done" : overview.current ? "active" : "idle");
  const statusLabel = $derived(
    overview.finished ? $t("Completed") : overview.current ? $t("In progress") : hasChecklist ? $t("Not started") : $t("Progress"),
  );
  /** Keep the current step visible even when the list is collapsed. */
  const visibleTodos = $derived.by(() => {
    if (showAll || overview.todos.length <= COLLAPSED_ROWS) return overview.todos;
    const currentIndex = overview.todos.findIndex((todo) => todo.status === "in_progress");
    const firstOpen = overview.todos.findIndex((todo) => todo.status !== "completed");
    const anchor = currentIndex >= 0 ? currentIndex : Math.max(0, firstOpen);
    const start = Math.max(0, Math.min(anchor - 2, overview.todos.length - COLLAPSED_ROWS));
    return overview.todos.slice(start, start + COLLAPSED_ROWS);
  });
  const hiddenCount = $derived(overview.todos.length - visibleTodos.length);
  const reportedTitle = $derived(
    progress?.todoId ? overview.todos.find((todo) => todo.id === progress.todoId)?.title : undefined,
  );
</script>

<section class="ad-goal" class:is-done={overview.finished} aria-label={$t("Build goal")}>
  <header class="ad-goal-head">
    <span class="ad-goal-label"><Target size={13} /> {$t("Build goal")}</span>
    {#if plan.source === "task"}<span class="ad-chip">{$t("Task")}</span>{/if}
    <span class="ad-chip ad-chip--{statusKind}">{statusLabel}</span>
  </header>

  {#if plan.objective}
    <p class="ad-goal-text" title={plan.objective}>{plan.objective}</p>
  {/if}

  {#if hasChecklist}
    <div class="ad-goal-meter">
      <svg class="ad-meter" viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
        <rect class="ad-meter-track" x="0" y="0" width="100" height="6" rx="3" />
        <rect class="ad-meter-fill" class:is-done={overview.finished} x="0" y="0" width={overview.pct} height="6" rx="3" />
      </svg>
      <span class="ad-goal-pct">{`${overview.pct}%`}</span>
    </div>
    <p class="ad-goal-count">{$t("{done} / {total} steps done", { done: overview.done, total: overview.total })}</p>

    {#if overview.current}
      <p class="ad-current" title={overview.current.title}>
        <Play size={11} class={busy ? "ad-current-ico is-busy" : "ad-current-ico"} />
        <span class="ad-current-label">{$t("Current step")}</span>
        <span class="ad-current-title">{overview.current.title}</span>
      </p>
    {:else if overview.finished}
      <p class="ad-current is-done">
        <span class="ad-current-label">{$t("All steps completed")}</span>
      </p>
    {/if}

    <ol class="ad-todos">
      {#each visibleTodos as todo (todo.id)}
        <li
          class="ad-todo ad-todo--{todo.status}"
          class:is-reported={progress?.todoId === todo.id}
          aria-current={todo.status === "in_progress" ? "step" : undefined}
        >
          <span class="ad-todo-glyph" aria-hidden="true">{TODO_GLYPH[todo.status]}</span>
          <span class="ad-todo-title">{todo.title}</span>
        </li>
      {/each}
    </ol>
    {#if overview.todos.length > COLLAPSED_ROWS}
      <button type="button" class="ad-link" onclick={() => (showAll = !showAll)}>
        {showAll ? $t("Show less") : $t("Show all ({n})", { n: overview.todos.length })}
        {#if !showAll && hiddenCount > 0}<span class="ad-muted">{`+${hiddenCount}`}</span>{/if}
      </button>
    {/if}
  {/if}

  {#if progress}
    <div class="ad-report">
      <p class="ad-report-line">
        {#if progress.phase}<strong class="ad-report-phase">{progress.phase}</strong><span class="ad-report-sep" aria-hidden="true">·</span>{/if}
        <span class="ad-report-msg">{progress.message}</span>
      </p>
      {#if progress.percent !== undefined}
        <div class="ad-goal-meter">
          <svg class="ad-meter ad-meter--thin" viewBox="0 0 100 4" preserveAspectRatio="none" aria-hidden="true">
            <rect class="ad-meter-track" x="0" y="0" width="100" height="4" rx="2" />
            <rect class="ad-meter-fill ad-meter-fill--agent" x="0" y="0" width={progress.percent} height="4" rx="2" />
          </svg>
          <span class="ad-goal-pct">{`${progress.percent}%`}</span>
        </div>
      {/if}
      <p class="ad-report-meta">
        {#if progress.percent !== undefined}<span>{$t("Agent estimate")}</span>{/if}
        {#if reportedTitle}<span class="truncate" title={reportedTitle}>{reportedTitle}</span>{/if}
        <span>{$t("{time} ago", { time: formatDuration(Math.max(0, now - progress.updatedMs)) })}</span>
      </p>
    </div>
  {/if}
</section>
