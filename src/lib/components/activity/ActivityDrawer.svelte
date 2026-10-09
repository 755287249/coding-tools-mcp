<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import CheckCheck from "@lucide/svelte/icons/check-check";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronsRight from "@lucide/svelte/icons/chevrons-right";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import FilePenLine from "@lucide/svelte/icons/file-pen-line";
  import FilePlus from "@lucide/svelte/icons/file-plus";
  import FileText from "@lucide/svelte/icons/file-text";
  import FileX from "@lucide/svelte/icons/file-x";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import ListChecks from "@lucide/svelte/icons/list-checks";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Maximize2 from "@lucide/svelte/icons/maximize-2";
  import Minimize2 from "@lucide/svelte/icons/minimize-2";
  import MousePointerClick from "@lucide/svelte/icons/mouse-pointer-click";
  import Search from "@lucide/svelte/icons/search";
  import SquareTerminal from "@lucide/svelte/icons/square-terminal";
  import Target from "@lucide/svelte/icons/target";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import Wrench from "@lucide/svelte/icons/wrench";
  import {
    clearWorkspaceActivity,
    emptyActivityStats,
    readWorkspaceActivity,
    readWorkspaceActivityDetail,
    type ActivityChange,
    type ActivityEvent,
    type ActivityKind,
    type ActivityPlan,
    type ActivityStats,
  } from "$lib/api/activity";
  import {
    baseName,
    dirName,
    formatBytes,
    formatClock,
    formatDelta,
    formatDuration,
    parseUnifiedDiff,
    type DiffFile,
  } from "$lib/activity/diff";
  import { t, type MessageKey } from "$lib/i18n";
  import DiffView from "./DiffView.svelte";
  import PlanCard from "./PlanCard.svelte";
  import { CATEGORY_META, changedFiles, groupEvents, type ActivityCategory } from "./plan";

  interface Props {
    resizable?: boolean;
    embedded?: boolean;
    workspaceId: string;
    /** MCP runtime is running (drives the idle poll rate). */
    live: boolean;
    expanded: boolean;
    onToggleExpanded: () => void;
    onClose: () => void;
  }

  let { embedded = false, resizable = false, workspaceId, live, expanded, onToggleExpanded, onClose }: Props = $props();

  let drawer = $state<HTMLElement>();
  let panelWidth = $state(360);
  let panelMax = $state(720);
  let resizing = $state<{x:number; width:number} | null>(null);
  function resizePanel(width: number) { panelWidth = Math.round(Math.max(Math.min(280, panelMax), Math.min(panelMax, width))); }
  $effect(() => { const wide = expanded; if (resizable) untrack(() => resizePanel(wide ? 640 : 360)); });
  $effect(() => {
    const parent = drawer?.parentElement;
    if (!resizable || !parent) return;
    const update = () => { const width = parent.clientWidth; panelMax = Math.max(220, width < 700 ? width - 24 : Math.min(900, width * .65)); resizePanel(panelWidth); };
    const observer = new ResizeObserver(update); observer.observe(parent); untrack(update);
    return () => observer.disconnect();
  });

  const MAX_EVENTS = 240;
  type Filter = "all" | "read" | "edit" | "exec" | "error";
  type View = "timeline" | "groups" | "files";

  let events = $state<ActivityEvent[]>([]);
  let stats = $state<ActivityStats>(emptyActivityStats());
  let plan = $state<ActivityPlan | undefined>(undefined);
  let loadError = $state("");
  let filter = $state<Filter>("all");
  let view = $state<View>("timeline");
  let collapsedGroups = $state<ActivityCategory[]>([]);
  let selectedSeq = $state<number | null>(null);
  let detail = $state<ActivityEvent | null>(null);
  let detailLoading = $state(false);
  let diffMode = $state<"unified" | "split">("unified");
  let diffFileIndex = $state(0);
  let now = $state(Date.now());

  let rev = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;
  let inFlight = false;

  const KIND_ICON = {
    read: FileText,
    search: Search,
    edit: FilePenLine,
    check: CheckCheck,
    exec: SquareTerminal,
    plan: ListChecks,
    git: GitBranch,
    desktop: MousePointerClick,
    other: Wrench,
  } satisfies Record<ActivityKind, unknown>;

  const TOOL_VERB: Record<string, MessageKey> = {
    read_file: "Read",
    read_many: "Read files",
    view_image: "View image",
    search_text: "Search",
    list_files: "List files",
    project_map: "Project map",
    list_workspace_folders: "List files",
    edit: "Edit",
    apply_patch: "Patch",
    file_ops: "File operations",
    format_files: "Format",
    patch_check: "Check patch",
    exec_command: "Run",
    exec_many: "Run batch",
    wait_command: "Wait",
    send_input: "Input",
    read_output: "Output",
    start_task: "Start task",
    update_task: "Update plan",
    finish_task: "Finish task",
    set_todos: "Set checklist",
    update_plan: "Update plan",
    report_progress: "Report progress",
  };

  function verbOf(event: ActivityEvent): string {
    if (event.tool === "file_ops" && event.changes.length > 0) {
      const ops = new Set(event.changes.map((change) => change.operation));
      if (ops.size === 1 && ops.has("add")) return $t("Create");
      if (ops.size === 1 && ops.has("delete")) return $t("Delete");
    }
    const key = TOOL_VERB[event.tool];
    if (key) return $t(key);
    if (event.kind === "git") return `Git ${event.tool.slice(4)}`;
    if (event.kind === "plan") return $t("Task");
    if (event.kind === "desktop") return $t("Desktop");
    return event.tool;
  }

  function isFileKind(event: ActivityEvent): boolean {
    return event.kind === "read" || event.kind === "edit" || event.kind === "check";
  }

  function primaryText(event: ActivityEvent): string {
    if (isFileKind(event) && event.paths.length > 0) {
      const extra = event.paths.length > 1 ? ` +${event.paths.length - 1}` : "";
      return `${baseName(event.paths[0])}${extra}`;
    }
    return event.title || event.paths[0] || "";
  }

  function secondaryText(event: ActivityEvent): string {
    if (event.status === "error" && event.error) return event.error;
    if (isFileKind(event) && event.paths.length > 0) {
      const dir = dirName(event.paths[0]);
      const range = event.kind === "read" && event.title.includes(":") ? event.title.slice(event.title.lastIndexOf(":")) : "";
      return [dir, range, event.kind === "read" ? event.detail : ""].filter(Boolean).join(" · ");
    }
    return event.detail ?? "";
  }

  function changeTotals(event: ActivityEvent) {
    let added = 0;
    let removed = 0;
    let before = 0;
    let after = 0;
    let sized = false;
    for (const change of event.changes) {
      added += change.added;
      removed += change.removed;
      if (change.bytesBefore !== undefined && change.bytesAfter !== undefined) {
        before += change.bytesBefore;
        after += change.bytesAfter;
        sized = true;
      }
    }
    return { added, removed, before, after, sized };
  }

  function opIcon(change: ActivityChange) {
    if (change.operation === "add") return FilePlus;
    if (change.operation === "delete") return FileX;
    return FilePenLine;
  }

  function opLetter(operation: string): string {
    if (operation === "add") return "A";
    if (operation === "delete") return "D";
    if (operation === "move") return "R";
    return "M";
  }

  function merge(incoming: ActivityEvent[], reset: boolean) {
    const map = new Map<number, ActivityEvent>();
    if (!reset) for (const event of events) map.set(event.seq, event);
    for (const event of incoming) map.set(event.seq, event);
    events = [...map.values()].sort((a, b) => b.seq - a.seq).slice(0, MAX_EVENTS);
  }

  async function poll() {
    if (inFlight || destroyed) return;
    inFlight = true;
    try {
      const snapshot = await readWorkspaceActivity(workspaceId, rev);
      if (destroyed) return;
      if (snapshot.reset || snapshot.events.length > 0) merge(snapshot.events, snapshot.reset);
      rev = snapshot.rev;
      stats = snapshot.stats;
      plan = snapshot.plan;
      loadError = "";
      if (selectedSeq !== null) {
        const fresh = snapshot.events.find((event) => event.seq === selectedSeq);
        if (fresh && (!detail || fresh.rev !== detail.rev)) void loadDetail(selectedSeq);
      }
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally {
      inFlight = false;
      now = Date.now();
      schedule();
    }
  }

  function schedule() {
    clearTimeout(timer);
    if (destroyed) return;
    const hidden = typeof document !== "undefined" && document.hidden;
    const recent = stats.running > 0 || (stats.lastMs > 0 && Date.now() - stats.lastMs < 15_000);
    const delay = hidden ? 5000 : recent ? 900 : live ? 2000 : 6000;
    timer = setTimeout(() => void poll(), delay);
  }

  async function loadDetail(seq: number) {
    const base = events.find((event) => event.seq === seq) ?? null;
    if (!base?.hasDiff) {
      detail = base;
      return;
    }
    detailLoading = !detail || detail.seq !== seq;
    try {
      const full = await readWorkspaceActivityDetail(workspaceId, seq);
      if (selectedSeq === seq) detail = full ?? base;
    } catch {
      if (selectedSeq === seq) detail = base;
    } finally {
      if (selectedSeq === seq) detailLoading = false;
    }
  }

  function select(seq: number) {
    selectedSeq = seq;
    detail = events.find((event) => event.seq === seq) ?? null;
    diffFileIndex = 0;
    diffMode = expanded ? "split" : "unified";
    void loadDetail(seq);
  }

  function back() {
    selectedSeq = null;
    detail = null;
  }

  async function clearAll() {
    try {
      await clearWorkspaceActivity(workspaceId);
    } catch {
      // Clearing is best effort.
    }
    events = [];
    rev = 0;
    back();
    void poll();
  }

  function toggleGroup(category: ActivityCategory) {
    collapsedGroups = collapsedGroups.includes(category)
      ? collapsedGroups.filter((item) => item !== category)
      : [...collapsedGroups, category];
  }

  function onVisibility() {
    if (!document.hidden) void poll();
  }

  onMount(() => {
    void poll();
    document.addEventListener("visibilitychange", onVisibility);
  });

  onDestroy(() => {
    destroyed = true;
    clearTimeout(timer);
    if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onVisibility);
  });

  const filtered = $derived(
    events.filter((event) => {
      if (filter === "all") return true;
      if (filter === "error") return event.status === "error";
      if (filter === "read") return event.kind === "read" || event.kind === "search";
      if (filter === "edit") return event.kind === "edit" || event.kind === "check";
      return event.kind === "exec";
    }),
  );
  const finished = $derived(stats.success + stats.errors);
  const successRate = $derived(finished > 0 ? Math.round((stats.success / finished) * 1000) / 10 : null);
  const spanMinutes = $derived(stats.firstMs > 0 ? Math.max(1, (Math.max(stats.lastMs, now) - stats.firstMs) / 60_000) : 1);
  const callsPerMinute = $derived(stats.total > 0 ? stats.total / spanMinutes : 0);
  const running = $derived(events.filter((event) => event.status === "running"));
  const groups = $derived(groupEvents(events));
  const files = $derived(changedFiles(events));
  const diffFiles = $derived<DiffFile[]>(detail?.diff ? parseUnifiedDiff(detail.diff) : []);
  const activeDiff = $derived(diffFiles[Math.min(diffFileIndex, Math.max(0, diffFiles.length - 1))]);
  const ringDash = $derived(successRate === null ? 0 : (successRate / 100) * 94.25);
  const maxChangeBytes = $derived(
    Math.max(1, ...(detail?.changes ?? []).flatMap((change) => [change.bytesBefore ?? 0, change.bytesAfter ?? 0])),
  );
</script>

<aside bind:this={drawer} class="ad-drawer ax-glass" class:embedded class:resizable class:resizing={!!resizing} style:width={resizable ? `${panelWidth}px` : undefined} class:is-expanded={expanded} aria-label={$t(embedded?"chat.projectActivity":"Task panel")}>
  {#if resizable}<button class="task-resize-handle" aria-label={$t('chat.resizeTasks')} title={$t('chat.resizeTasks')}
    onpointerdown={event=>{event.preventDefault();resizing={x:event.clientX,width:panelWidth};event.currentTarget.setPointerCapture(event.pointerId)}}
    onpointermove={event=>{if(resizing)resizePanel(resizing.width+resizing.x-event.clientX)}}
    onpointerup={event=>{resizing=null;event.currentTarget.releasePointerCapture(event.pointerId)}} onpointercancel={()=>resizing=null} onlostpointercapture={()=>resizing=null}
    onkeydown={event=>{if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();resizePanel(panelWidth+(event.key==='ArrowLeft'?20:-20))}else if(event.key==='Home'){event.preventDefault();resizePanel(280)}else if(event.key==='End'){event.preventDefault();resizePanel(panelMax)}}}></button>{/if}

  <header class="ad-head">
    {#if detail}
      <button type="button" class="ad-icon-btn" onclick={back} title={$t("Back")} aria-label={$t("Back")}>
        <ArrowLeft size={15} />
      </button>
      <h3 class="ad-title">{verbOf(detail)}</h3>
    {:else}
      <h3 class="ad-title">
        {$t(embedded?"chat.projectActivity":"Task panel")}
        {#if stats.running > 0}<span class="ad-live" title={$t("Running")}></span>{/if}
      </h3>
    {/if}
    <div class="ad-head-actions">
      {#if !detail && events.length > 0}
        <button type="button" class="ad-icon-btn" onclick={() => void clearAll()} title={$t("Clear")} aria-label={$t("Clear")}>
          <Trash2 size={14} />
        </button>
      {/if}
      {#if !embedded}<button
        type="button"
        class="ad-icon-btn"
        onclick={onToggleExpanded}
        title={expanded ? $t("Collapse") : $t("Expand")}
        aria-label={expanded ? $t("Collapse") : $t("Expand")}
      >
        {#if expanded}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
      </button>
      <button type="button" class="ad-icon-btn" onclick={onClose} title={$t("Collapse panel")} aria-label={$t("Collapse panel")}>
        <ChevronsRight size={16} />
      </button>{/if}
    </div>
  </header>

  {#if detail}
    <!-- ───────────── Detail ───────────── -->
    {@const DetailIcon = KIND_ICON[detail.kind]}
    <div class="ad-body">
      <div class="ad-detail-head">
        <span class="ad-ico ad-k-{detail.kind}">
          <DetailIcon size={14} />
        </span>
        <div class="ad-detail-title">
          <p class="tx-mono ad-break">{detail.title || detail.tool}</p>
          <p class="ad-muted">
            {detail.tool} · {formatClock(detail.startedMs)}
            {#if detail.durationMs !== undefined} · {formatDuration(detail.durationMs)}{/if}
          </p>
        </div>
        <span class="ad-status ad-status--{detail.status}">
          {#if detail.status === "running"}<LoaderCircle size={12} class="animate-spin" /> {$t("Running")}
          {:else if detail.status === "success"}<CircleCheck size={12} /> {$t("Succeeded")}
          {:else}<CircleAlert size={12} /> {$t("Failed")}{/if}
        </span>
      </div>

      {#if detail.dryRun}
        <p class="ad-note">{$t("Preview only — changes were not written.")}</p>
      {/if}
      {#if detail.error}
        <pre class="ad-error">{detail.error}</pre>
      {:else if detail.detail}
        <pre class="ad-detail-text">{detail.detail}</pre>
      {/if}

      <dl class="ad-facts">
        <div><dt>{$t("Request")}</dt><dd>{formatBytes(detail.requestBytes)}</dd></div>
        <div><dt>{$t("Response")}</dt><dd>{detail.status === "running" ? "…" : formatBytes(detail.responseBytes)}</dd></div>
        <div><dt>{$t("Duration")}</dt><dd>{detail.durationMs !== undefined ? formatDuration(detail.durationMs) : "…"}</dd></div>
      </dl>

      {#if detail.changes.length > 0}
        <section class="ad-section">
          <h4 class="ad-section-title">{$t("File changes")}</h4>
          <ul class="ad-changes">
            {#each detail.changes as change (change.path + change.operation)}
              {@const OpIcon = opIcon(change)}
              <li class="ad-change">
                <div class="ad-change-top">
                  <span class="ad-op ad-op--{change.operation}" title={change.operation}>{opLetter(change.operation)}</span>
                  <OpIcon size={13} />
                  <span class="tx-mono ad-change-path" title={change.path}>{change.path}</span>
                  <span class="ad-lines">
                    <span class="ad-plus">+{change.added}</span>
                    <span class="ad-minus">−{change.removed}</span>
                  </span>
                </div>
                {#if change.bytesBefore !== undefined && change.bytesAfter !== undefined}
                  <div class="ad-size">
                    <span class="ad-size-label">{$t("Before")}</span>
                    <svg class="ad-size-bar" viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
                      <rect class="ad-size-track" x="0" y="0" width="100" height="6" rx="3" />
                      <rect class="ad-size-before" x="0" y="0" width={(change.bytesBefore / maxChangeBytes) * 100} height="6" rx="3" />
                    </svg>
                    <span class="ad-size-val">{formatBytes(change.bytesBefore)}</span>
                    <span class="ad-size-label">{$t("After")}</span>
                    <svg class="ad-size-bar" viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
                      <rect class="ad-size-track" x="0" y="0" width="100" height="6" rx="3" />
                      <rect class="ad-size-after" x="0" y="0" width={(change.bytesAfter / maxChangeBytes) * 100} height="6" rx="3" />
                    </svg>
                    <span class="ad-size-val">
                      {formatBytes(change.bytesAfter)}
                      <em class:ad-plus={change.bytesAfter > change.bytesBefore} class:ad-minus={change.bytesAfter < change.bytesBefore}>
                        {formatDelta(change.bytesBefore, change.bytesAfter)}
                      </em>
                    </span>
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {:else if detail.paths.length > 0}
        <section class="ad-section">
          <h4 class="ad-section-title">{detail.kind === "read" ? $t("Files viewed") : $t("Targets")}</h4>
          <ul class="ad-paths">
            {#each detail.paths as path (path)}
              <li class="tx-mono" title={path}>{path}</li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if detail.hasDiff}
        <section class="ad-section">
          <div class="ad-diff-bar">
            <h4 class="ad-section-title">{$t("Code diff")}</h4>
            <div class="ad-seg" role="radiogroup" aria-label={$t("Code diff")}>
              <button type="button" role="radio" aria-checked={diffMode === "unified"} class:active={diffMode === "unified"} onclick={() => (diffMode = "unified")}>
                {$t("Unified")}
              </button>
              <button type="button" role="radio" aria-checked={diffMode === "split"} class:active={diffMode === "split"} onclick={() => (diffMode = "split")}>
                {$t("Side by side")}
              </button>
            </div>
          </div>
          {#if diffFiles.length > 1}
            <div class="ad-tabs" role="tablist">
              {#each diffFiles as file, index (file.path + index)}
                <button type="button" role="tab" aria-selected={index === diffFileIndex} class:active={index === diffFileIndex} title={file.path} onclick={() => (diffFileIndex = index)}>
                  {baseName(file.path)}
                  <span class="ad-plus">+{file.added}</span><span class="ad-minus">−{file.removed}</span>
                </button>
              {/each}
            </div>
          {/if}
          {#if detailLoading}
            <p class="ad-muted ad-pad"><LoaderCircle size={13} class="animate-spin" /> {$t("Loading…")}</p>
          {:else if activeDiff}
            {#if diffMode === "split"}
              <div class="ad-split-head"><span>{$t("Before")}</span><span>{$t("After")}</span></div>
            {/if}
            {#key `${detail.seq}:${diffFileIndex}:${diffMode}`}
              <DiffView file={activeDiff} mode={diffMode} />
            {/key}
            {#if detail.diffTruncated}
              <p class="ad-note">{$t("Diff truncated for display.")}</p>
            {/if}
          {:else}
            <p class="ad-muted ad-pad">{$t("No diff available.")}</p>
          {/if}
        </section>
      {/if}
    </div>
  {:else}
    <!-- ───────────── Overview ───────────── -->
    <div class="ad-body">
      <!-- Goal · overall progress · current step · checklist · agent report -->
      {#if plan}
        <PlanCard {plan} busy={stats.running > 0} {now} />
      {:else}
        <section class="ad-goal is-empty">
          <header class="ad-goal-head">
            <span class="ad-goal-label"><Target size={13} /> {$t("Task progress")}</span>
            <span class="ad-chip">{stats.total}</span>
          </header>
          <p class="ad-muted">{$t("No plan reported yet. The AI's goal, steps and progress appear here when it calls set_todos / update_plan / report_progress.")}</p>
        </section>
      {/if}

      <!-- Work in flight -->
      {#if running.length > 0}
        <section class="ad-working" aria-live="polite">
          <h4 class="ad-section-title"><span class="ad-live"></span>{$t("Working now")}</h4>
          <ul class="ad-working-list">
            {#each running.slice(0, 3) as event (event.seq)}
              {@const RunIcon = KIND_ICON[event.kind]}
              <li>
                <button type="button" class="ad-working-row" onclick={() => select(event.seq)}>
                  <span class="ad-ico ad-k-{event.kind}"><RunIcon size={12} /></span>
                  <span class="ad-verb">{verbOf(event)}</span>
                  <span class="tx-mono ad-target" title={event.title}>{primaryText(event)}</span>
                  <span class="ad-working-time"><LoaderCircle size={11} class="animate-spin" /> {formatDuration(now - event.startedMs)}</span>
                </button>
              </li>
            {/each}
          </ul>
          {#if running.length > 3}<p class="ad-muted">{`+${running.length - 3}`}</p>{/if}
        </section>
      {/if}

      <!-- Stats -->
      <section class="ad-stats">
        <div class="ad-stat ad-stat--ring">
          <svg viewBox="0 0 36 36" class="ad-ring" aria-hidden="true">
            <circle class="ad-ring-track" cx="18" cy="18" r="15" />
            <circle
              class="ad-ring-fill"
              class:is-bad={successRate !== null && successRate < 80}
              cx="18"
              cy="18"
              r="15"
              stroke-dasharray={`${ringDash} 94.25`}
            />
          </svg>
          <div>
            <p class="ad-stat-val">{successRate === null ? "–" : `${successRate}%`}</p>
            <p class="ad-stat-label">{$t("Success rate")}</p>
          </div>
        </div>
        <div class="ad-stat">
          <p class="ad-stat-val">{stats.total > 0 ? formatDuration(stats.avgDurationMs) : "–"}</p>
          <p class="ad-stat-label">{`${$t("Avg response")} · p95 ${stats.total > 0 ? formatDuration(stats.p95DurationMs) : "–"}`}</p>
        </div>
        <div class="ad-stat">
          <p class="ad-stat-val">{stats.total}{#if stats.running > 0}<small> +{stats.running}</small>{/if}</p>
          <p class="ad-stat-label">{`${$t("Calls")} · ${callsPerMinute.toFixed(1)}/min`}</p>
        </div>
        <div class="ad-stat">
          <p class="ad-stat-val">{formatBytes(stats.bytesIn + stats.bytesOut)}</p>
          <p class="ad-stat-label">↑{formatBytes(stats.bytesIn)} ↓{formatBytes(stats.bytesOut)}</p>
        </div>
      </section>

      <p class="ad-counters">
        <span><FileText size={12} /> {stats.reads}</span>
        <span><Search size={12} /> {stats.searches}</span>
        <span><FilePenLine size={12} /> {stats.edits}</span>
        <span><SquareTerminal size={12} /> {stats.execs}</span>
        <span class="ad-sep"></span>
        <span title={$t("Files changed")}>{$t("Files changed")} {stats.filesChanged}</span>
        <span class="ad-plus">+{stats.linesAdded}</span>
        <span class="ad-minus">−{stats.linesRemoved}</span>
      </p>

      <!-- Views -->
      <div class="ad-viewbar" role="tablist" aria-label={$t("Task panel")}>
        <button type="button" role="tab" aria-selected={view === "timeline"} class:active={view === "timeline"} onclick={() => (view = "timeline")}>
          {$t("Timeline")}
        </button>
        <button type="button" role="tab" aria-selected={view === "groups"} class:active={view === "groups"} onclick={() => (view = "groups")}>
          {$t("By category")}
        </button>
        <button type="button" role="tab" aria-selected={view === "files"} class:active={view === "files"} onclick={() => (view = "files")}>
          {$t("Files changed")}
          {#if files.length > 0}<span class="ad-viewbar-count">{files.length}</span>{/if}
        </button>
      </div>

      {#if loadError}
        <p class="ad-error">{loadError}</p>
      {/if}

      {#if view === "timeline"}
      <!-- Filters -->
        <div class="ad-filters" role="tablist">
          {#each [["all", "All"], ["read", "Reads"], ["edit", "Edits"], ["exec", "Commands"], ["error", "Errors"]] as [value, label] (value)}
          <button type="button" role="tab" aria-selected={filter === value} class:active={filter === value} onclick={() => (filter = value as Filter)}>
            {$t(label as MessageKey)}
            {#if value === "error" && stats.errors > 0}<span class="ad-badge">{stats.errors}</span>{/if}
          </button>
        {/each}
        </div>

      <!-- Timeline -->
      {#if filtered.length === 0}
        <div class="ad-empty">
          <ListChecks size={22} />
          <p>{events.length === 0 ? $t("Waiting for AI activity…") : $t("Nothing matches this filter.")}</p>
          {#if events.length === 0}
            <p class="ad-muted">{$t("Every file the AI reads, searches or edits shows up here live.")}</p>
          {/if}
        </div>
      {:else}
        <ul class="ad-timeline">
          {#each filtered as event (event.seq)}
            {@const totals = changeTotals(event)}
            {@const RowIcon = KIND_ICON[event.kind]}
            <li>
              <button
                type="button"
                class="ad-row"
                class:is-running={event.status === "running"}
                class:is-error={event.status === "error"}
                onclick={() => select(event.seq)}
              >
                <span class="ad-ico ad-k-{event.kind}">
                  {#if event.status === "running"}
                    <LoaderCircle size={13} class="animate-spin" />
                  {:else}
                    <RowIcon size={13} />
                  {/if}
                </span>
                <span class="ad-row-main">
                  <span class="ad-row-top">
                    <span class="ad-verb">{verbOf(event)}</span>
                    <span class="ad-target tx-mono" title={event.title}>{primaryText(event)}</span>
                  </span>
                  {#if secondaryText(event) || event.changes.length > 0}
                    <span class="ad-row-sub">
                      {#if event.changes.length > 0}
                        <span class="ad-plus">+{totals.added}</span>
                        <span class="ad-minus">−{totals.removed}</span>
                        {#if totals.sized}
                          <span>{formatBytes(totals.before)} → {formatBytes(totals.after)}</span>
                        {/if}
                        {#if event.dryRun}<span class="ad-tag">{$t("Preview")}</span>{/if}
                      {/if}
                      <span class="ad-row-sub-text" class:ad-minus={event.status === "error"}>{secondaryText(event)}</span>
                    </span>
                  {/if}
                </span>
                <span class="ad-row-meta">
                  <span>{event.status === "running" ? formatDuration(now - event.startedMs) : formatDuration(event.durationMs)}</span>
                  <span class="ad-muted">{formatClock(event.startedMs)}</span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      {:else if view === "groups"}
        <!-- Tool activity grouped by category -->
        {#if groups.length === 0}
          <div class="ad-empty">
            <ListChecks size={22} />
            <p>{$t("Waiting for AI activity…")}</p>
          </div>
        {:else}
          <div class="ad-groups">
            {#each groups as group (group.category)}
              {@const meta = CATEGORY_META[group.category]}
              {@const open = !collapsedGroups.includes(group.category)}
              <section class="ad-group" class:is-open={open}>
                <button type="button" class="ad-group-head" aria-expanded={open} onclick={() => toggleGroup(group.category)}>
                  <span class="ad-group-emoji" aria-hidden="true">{meta.emoji}</span>
                  <span class="ad-group-name">{$t(meta.label)}</span>
                  <span class="ad-group-count">{group.total}</span>
                  {#if group.running > 0}<span class="ad-live" title={$t("Running")}></span>{/if}
                  {#if group.errors > 0}<span class="ad-badge" title={$t("Errors")}>{group.errors}</span>{/if}
                  <span class="ad-group-time">{formatDuration(group.durationMs)}</span>
                  <ChevronDown size={14} class="ad-group-chevron" />
                </button>
                {#if open}
                  <ul class="ad-group-list">
                    {#each group.events as event (event.seq)}
                      <li>
                        <button
                          type="button"
                          class="ad-mini-row"
                          class:is-error={event.status === "error"}
                          class:is-running={event.status === "running"}
                          onclick={() => select(event.seq)}
                        >
                          <span class="ad-state ad-state--{event.status}" title={event.status === "running" ? $t("Running") : event.status === "success" ? $t("Succeeded") : $t("Failed")}></span>
                          <span class="ad-verb">{verbOf(event)}</span>
                          <span class="tx-mono ad-target" title={event.title}>{primaryText(event)}</span>
                          <span class="ad-mini-meta">{event.status === "running" ? formatDuration(now - event.startedMs) : formatDuration(event.durationMs)}</span>
                        </button>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </section>
            {/each}
          </div>
        {/if}
      {:else}
        <!-- Files changed in this session -->
        {#if files.length === 0}
          <div class="ad-empty">
            <FilePenLine size={22} />
            <p>{$t("No files changed yet.")}</p>
          </div>
        {:else}
          <p class="ad-files-sum">
            <span>{$t("Files changed")} {files.length}</span>
            <span class="ad-plus">+{stats.linesAdded}</span>
            <span class="ad-minus">−{stats.linesRemoved}</span>
          </p>
          <ul class="ad-files">
            {#each files as file (file.path)}
              <li>
                <button type="button" class="ad-file-row" title={file.path} onclick={() => select(file.lastSeq)}>
                  <span class="ad-op ad-op--{file.operation}">{opLetter(file.operation)}</span>
                  <span class="ad-file-main">
                    <span class="tx-mono ad-file-name">{baseName(file.path)}</span>
                    <span class="ad-file-dir">{dirName(file.path)}</span>
                  </span>
                  {#if file.edits > 1}<span class="ad-tag">{`×${file.edits}`}</span>{/if}
                  <span class="ad-lines">
                    <span class="ad-plus">+{file.added}</span>
                    <span class="ad-minus">−{file.removed}</span>
                  </span>
                  <span class="ad-mini-meta">{formatClock(file.lastMs)}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </div>
  {/if}
</aside>

<style>
.ad-drawer.resizable{position:relative;align-self:stretch;margin:14px 14px 14px 8px;border:1px solid var(--color-border);border-radius:18px;box-shadow:0 16px 42px #0004;max-width:calc(100% - 28px)}
.task-resize-handle{position:absolute;inset:12px auto 12px 0;width:7px;z-index:5;cursor:col-resize;touch-action:none;border-radius:8px;background:transparent}.task-resize-handle:hover,.task-resize-handle:focus-visible,.resizing .task-resize-handle{background:#6096ec88;outline:none}.resizing{user-select:none}
@media(max-width:1040px){.ad-drawer.resizable{position:absolute;top:0;right:0;bottom:0;z-index:35;background:color-mix(in srgb,var(--card-bg) 90%,transparent);backdrop-filter:blur(24px)}}
.ad-drawer.embedded{position:relative;inset:auto;width:100%;min-width:0;max-width:none;height:100%;flex:1;margin:0;border:0;border-radius:0;box-shadow:none;transform:none;z-index:auto}
</style>
