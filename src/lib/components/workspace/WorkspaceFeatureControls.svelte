<script lang="ts">
  import Tabs from "$lib/components/Tabs.svelte";
  import { getBackend, type ExtensionInventoryPayload, type ExtensionKind, type SkillInventoryPayload } from "$lib/backend";
  import { t } from "$lib/i18n";

  type FeatureTab = "skills" | "hooks" | "mcp";

  interface Props {
    workspaceId: string;
    initialTab?: FeatureTab;
  }

  let { workspaceId, initialTab = "skills" }: Props = $props();

  const backend = getBackend().workspaceFeatures;
  let skills = $state<SkillInventoryPayload | null>(null);
  let extensions = $state<ExtensionInventoryPayload | null>(null);
  let loading = $state(true);
  let error = $state("");
  let busy = $state(new Set<string>());
  let activeTab = $state<FeatureTab>("skills");
  $effect(() => { activeTab = initialTab; });
  let loadGeneration = 0;

  const featureTabs = $derived([
    { value: "skills", label: $t("Skills") },
    { value: "hooks", label: $t("Hooks") },
    { value: "mcp", label: "MCP" },
  ]);

  const diagnostics = $derived([
    ...(skills?.diagnostics ?? []),
    ...(extensions?.diagnostics ?? []),
  ]);

  function isBusy(key: string): boolean {
    return busy.has(key);
  }

  function setBusy(key: string, value: boolean) {
    const next = new Set(busy);
    if (value) next.add(key);
    else next.delete(key);
    busy = next;
  }

  async function refresh(id = workspaceId) {
    const generation = ++loadGeneration;
    loading = true;
    error = "";
    try {
      const [nextSkills, nextExtensions] = await Promise.all([
        backend.skills(id),
        backend.extensions(id),
      ]);
      if (generation !== loadGeneration || id !== workspaceId) return;
      skills = nextSkills;
      extensions = nextExtensions;
    } catch (cause) {
      if (generation !== loadGeneration || id !== workspaceId) return;
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      if (generation === loadGeneration && id === workspaceId) loading = false;
    }
  }

  async function toggleSkillsActive(active: boolean) {
    if (!skills || isBusy("skills:master")) return;
    setBusy("skills:master", true);
    try {
      await backend.setSkillsActive(workspaceId, active);
      await refresh();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      setBusy("skills:master", false);
    }
  }

  async function toggleSkill(key: string, enabled: boolean) {
    const busyKey = `skill:${key}`;
    if (isBusy(busyKey)) return;
    setBusy(busyKey, true);
    try {
      await backend.setSkillEnabled(workspaceId, key, enabled);
      await refresh();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      setBusy(busyKey, false);
    }
  }

  async function toggleExtensionActive(kind: ExtensionKind, active: boolean) {
    const busyKey = `${kind}:master`;
    if (isBusy(busyKey)) return;
    setBusy(busyKey, true);
    try {
      await backend.setExtensionActive(workspaceId, kind, active);
      await refresh();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      setBusy(busyKey, false);
    }
  }

  async function toggleExtension(kind: ExtensionKind, key: string, enabled: boolean) {
    const busyKey = `${kind}:${key}`;
    if (isBusy(busyKey)) return;
    setBusy(busyKey, true);
    try {
      await backend.setExtensionEnabled(workspaceId, kind, key, enabled);
      await refresh();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      setBusy(busyKey, false);
    }
  }

  $effect(() => {
    const id = workspaceId;
    void refresh(id);
    return () => {
      loadGeneration += 1;
    };
  });
</script>

<div class="grid gap-4">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div>
      <p class="tx-section-label">{$t("Workspace feature controls")}</p>
      <h3 class="mt-1 text-lg font-semibold">{$t("Skills / Hooks / MCP")}</h3>
      <p class="mt-1 text-sm text-[var(--color-text-muted)]">
        {$t("Manage discovered Skills, hooks, and MCP servers from Codex and Claude configuration.")}
      </p>
    </div>
    <button type="button" class="tx-btn-ghost" disabled={loading} onclick={() => void refresh()}>
      {$t("Refresh")}
    </button>
  </div>

  <Tabs
    items={featureTabs}
    value={activeTab}
    idPrefix="workspace-feature-tabs"
    ariaLabel={$t("Workspace feature controls")}
    onchange={(value) => (activeTab = value as FeatureTab)}
  />

  {#if error}
    <div class="tx-card border-[var(--color-error)] p-4 text-sm text-[var(--color-error)]">{error}</div>
  {/if}

  <div
    class="tx-tabpanel mt-4"
    role="tabpanel"
    id={`workspace-feature-tabs-panel-${activeTab}`}
    aria-labelledby={`workspace-feature-tabs-tab-${activeTab}`}
    tabindex="0"
  >
    {#if loading && !skills && !extensions}
      <div class="tx-card p-5 text-sm text-[var(--color-text-muted)]">{$t("Working…")}</div>
    {:else}
      {#if activeTab === "skills"}
        <section class="tx-card overflow-hidden">
          <div class="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--color-border)] p-5">
            <div>
              <h4 class="font-semibold">{$t("Skills")}</h4>
              <p class="mt-1 text-xs text-[var(--color-text-muted)]">
                {skills?.skills.length ?? 0} {$t("Skills")}
              </p>
            </div>
            <label class="flex cursor-pointer items-center gap-2 text-sm font-medium">
              <span>{$t("Enable all skills")}</span>
              <input
                type="checkbox"
                class="h-4 w-4 accent-[var(--color-accent)]"
                checked={skills?.active ?? false}
                disabled={!skills || isBusy("skills:master")}
                onchange={(event) => void toggleSkillsActive(event.currentTarget.checked)}
              />
            </label>
          </div>
          {#if skills?.skills.length}
            <div class="divide-y divide-[var(--color-border)]">
              {#each skills.skills as skill (skill.key)}
                <div class="flex items-start justify-between gap-4 p-4">
                  <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-2">
                      <span class="font-medium">{skill.name}</span>
                      <span class="tx-status-pill cursor-default">{skill.source}</span>
                      <span class="tx-status-pill cursor-default">{skill.scope}</span>
                      {#if skill.folderName}<span class="text-xs text-[var(--color-text-muted)]">{skill.folderName}</span>{/if}
                    </div>
                    {#if skill.description}
                      <p class="mt-1 text-sm text-[var(--color-text-secondary)]">{skill.description}</p>
                    {/if}
                    <p class="mt-1 break-all text-xs text-[var(--color-text-muted)]">{skill.relativePath}</p>
                  </div>
                  <input
                    type="checkbox"
                    class="mt-1 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
                    aria-label={`${skill.name} ${$t("Skills")}`}
                    checked={skill.enabled}
                    disabled={!skills.active || isBusy(`skill:${skill.key}`)}
                    onchange={(event) => void toggleSkill(skill.key, event.currentTarget.checked)}
                  />
                </div>
              {/each}
            </div>
          {:else}
            <p class="p-5 text-sm text-[var(--color-text-muted)]">{$t("No skills discovered.")}</p>
          {/if}
        </section>
      {:else if activeTab === "hooks"}
        <section class="tx-card overflow-hidden">
          <div class="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--color-border)] p-5">
            <div>
              <h4 class="font-semibold">{$t("Hooks")}</h4>
              <p class="mt-1 text-xs text-[var(--color-text-muted)]">
                {extensions?.hooks.length ?? 0} {$t("Hooks")}
              </p>
            </div>
            <label class="flex cursor-pointer items-center gap-2 text-sm font-medium">
              <span>{$t("Enable all hooks")}</span>
              <input
                type="checkbox"
                class="h-4 w-4 accent-[var(--color-accent)]"
                checked={extensions?.hooksActive ?? false}
                disabled={!extensions || isBusy("hook:master")}
                onchange={(event) => void toggleExtensionActive("hook", event.currentTarget.checked)}
              />
            </label>
          </div>
          {#if extensions?.hooks.length}
            <div class="divide-y divide-[var(--color-border)]">
              {#each extensions.hooks as hook (hook.key)}
                <div class="flex items-start justify-between gap-4 p-4">
                  <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-2">
                      <span class="font-medium">{hook.event}</span>
                      <span class="tx-status-pill cursor-default">{hook.provider}</span>
                      <span class="tx-status-pill cursor-default">{hook.scope}</span>
                      {#if !hook.supported}<span class="text-xs text-[var(--color-warning)]">{$t("Unsupported")}</span>{/if}
                      {#if !hook.sourceEnabled}<span class="text-xs text-[var(--color-text-muted)]">{$t("Source disabled")}</span>{/if}
                    </div>
                    {#if hook.matcher}<p class="mt-1 text-sm text-[var(--color-text-secondary)]">{hook.matcher}</p>{/if}
                    <p class="mt-1 break-all text-xs text-[var(--color-text-muted)]">{hook.sourcePath}</p>
                  </div>
                  <input
                    type="checkbox"
                    class="mt-1 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
                    aria-label={`${hook.event} ${$t("Hooks")}`}
                    checked={hook.enabled}
                    disabled={!extensions.hooksActive || !hook.supported || !hook.sourceEnabled || isBusy(`hook:${hook.key}`)}
                    onchange={(event) => void toggleExtension("hook", hook.key, event.currentTarget.checked)}
                  />
                </div>
              {/each}
            </div>
          {:else}
            <p class="p-5 text-sm text-[var(--color-text-muted)]">{$t("No hooks discovered.")}</p>
          {/if}
        </section>
      {:else if activeTab === "mcp"}
        <section class="tx-card overflow-hidden">
          <div class="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--color-border)] p-5">
            <div>
              <h4 class="font-semibold">{$t("MCP servers")}</h4>
              <p class="mt-1 text-xs text-[var(--color-text-muted)]">
                {extensions?.mcpServers.length ?? 0} {$t("MCP servers")}
              </p>
            </div>
            <label class="flex cursor-pointer items-center gap-2 text-sm font-medium">
              <span>{$t("Enable all MCP servers")}</span>
              <input
                type="checkbox"
                class="h-4 w-4 accent-[var(--color-accent)]"
                checked={extensions?.mcpActive ?? false}
                disabled={!extensions || isBusy("mcp:master")}
                onchange={(event) => void toggleExtensionActive("mcp", event.currentTarget.checked)}
              />
            </label>
          </div>
          {#if extensions?.mcpServers.length}
            <div class="divide-y divide-[var(--color-border)]">
              {#each extensions.mcpServers as server (server.key)}
                <div class="flex items-start justify-between gap-4 p-4">
                  <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-2">
                      <span class="font-medium">{server.name}</span>
                      <span class="tx-status-pill cursor-default">{server.provider}</span>
                      <span class="tx-status-pill cursor-default">{server.scope}</span>
                      <span class="tx-status-pill cursor-default">{server.transport}</span>
                      {#if server.connected}<span class="text-xs text-[var(--color-success)]">{$t("Connected")}</span>{/if}
                      {#if !server.supported}<span class="text-xs text-[var(--color-warning)]">{$t("Unsupported")}</span>{/if}
                      {#if !server.sourceEnabled}<span class="text-xs text-[var(--color-text-muted)]">{$t("Source disabled")}</span>{/if}
                    </div>
                    <p class="mt-1 text-sm text-[var(--color-text-secondary)]">
                      {$t("{count} tools", { count: server.toolCount })}
                    </p>
                    {#if server.error}<p class="mt-1 text-sm text-[var(--color-error)]">{server.error}</p>{/if}
                    <p class="mt-1 break-all text-xs text-[var(--color-text-muted)]">{server.sourcePath}</p>
                  </div>
                  <input
                    type="checkbox"
                    class="mt-1 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
                    aria-label={`${server.name} MCP`}
                    checked={server.enabled}
                    disabled={!extensions.mcpActive || !server.supported || !server.sourceEnabled || isBusy(`mcp:${server.key}`)}
                    onchange={(event) => void toggleExtension("mcp", server.key, event.currentTarget.checked)}
                  />
                </div>
              {/each}
            </div>
          {:else}
            <p class="p-5 text-sm text-[var(--color-text-muted)]">{$t("No MCP servers discovered.")}</p>
          {/if}
        </section>
      {/if}

      {#if diagnostics.length}
        <section class="tx-card p-5">
          <h4 class="font-semibold">{$t("Diagnostics")}</h4>
          <div class="mt-3 grid gap-2">
            {#each diagnostics as diagnostic}
              <div class="rounded-lg border border-[var(--color-border)] p-3 text-sm">
                <p class="font-medium">{diagnostic.code}</p>
                <p class="mt-1 text-[var(--color-text-secondary)]">{diagnostic.message}</p>
              </div>
            {/each}
          </div>
        </section>
      {/if}
    {/if}
  </div>
</div>
