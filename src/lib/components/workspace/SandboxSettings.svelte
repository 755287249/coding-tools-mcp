<script lang="ts">
  import { onMount } from "svelte";
  import { listSandboxBackends } from "$lib/api/workspaces";
  import { t } from "$lib/i18n";
  import type {
    SandboxBackendDescriptor,
    SandboxConfig,
    SandboxPathAccess,
    SandboxPathGrant,
  } from "$lib/types";

  interface Props {
    config: SandboxConfig;
    locked: boolean;
    onSave: (config: SandboxConfig) => void | Promise<void>;
    hasWslFolder?: boolean;
    projectIsolation?:boolean;
  }

  let { config, locked, onSave, hasWslFolder = false, projectIsolation=false }: Props = $props();
  let enabled = $state(false);
  let backend = $state("appcontainer");
  let readOnlyPaths = $state("");
  let writablePaths = $state("");
  let options = $state<Record<string, string>>({});
  let backends = $state<SandboxBackendDescriptor[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let error = $state("");

  const selected = $derived(backends.find((item) => item.id === backend));
  const wslUnsupported = $derived(Boolean(hasWslFolder && selected && !selected.supportsWsl));
  const pendingExternalPaths = $derived(buildExternalPaths(readOnlyPaths, writablePaths));
  const dirty = $derived(
    enabled !== config.enabled ||
      backend !== config.backend ||
      JSON.stringify(pendingExternalPaths) !== JSON.stringify(config.external_paths ?? []) ||
      JSON.stringify(options) !== JSON.stringify(config.options ?? {}),
  );

  $effect(() => {
    enabled = projectIsolation || config.enabled;
    backend = config.backend;
    readOnlyPaths = pathsForAccess(config.external_paths ?? [], "read_only");
    writablePaths = pathsForAccess(config.external_paths ?? [], "modify");
    options = { ...(config.options ?? {}) };
  });

  function pathsForAccess(paths: SandboxPathGrant[], access: SandboxPathAccess) {
    return paths
      .filter((item) => item.access === access)
      .map((item) => item.path)
      .join("\n");
  }

  function parsePathLines(value: string) {
    return value
      .split(/\r?\n/)
      .map((item) => item.trim())
      .filter(Boolean);
  }

  function buildExternalPaths(readOnly: string, writable: string): SandboxPathGrant[] {
    const paths = new Map<string, SandboxPathAccess>();
    for (const path of parsePathLines(readOnly)) paths.set(path, "read_only");
    for (const path of parsePathLines(writable)) paths.set(path, "modify");
    return Array.from(paths, ([path, access]) => ({ path, access }));
  }

  onMount(() => {
    void loadBackends();
  });

  async function loadBackends() {
    loading = true;
    error = "";
    try {
      backends = await listSandboxBackends();
      if (!backends.some((item) => item.id === backend) && backends[0]) {
        backend = backends[0].id;
      }
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    error = "";
    try {
      await onSave({ enabled:projectIsolation||enabled, backend, external_paths:projectIsolation?[]:pendingExternalPaths, options });
    } catch (cause) {
      error = String(cause);
    } finally {
      saving = false;
    }
  }
</script>

<div class="grid gap-4">
  <div class="flex items-start justify-between gap-5">
    <div>
      <p class="text-sm font-semibold text-[var(--color-text)]">{$t("Command sandbox")}</p>
      <p class="mt-1 text-xs leading-5 text-[var(--color-text-muted)]">
        {$t("When enabled, command execution must use the selected OS sandbox. It never silently falls back to policy-only execution.")}
      </p>
    </div>
    <span class="tx-badge" class:tx-badge-success={enabled}>{$t(enabled ? "Enabled" : "Disabled")}</span>
  </div>

  <label class="flex items-start gap-3 rounded-lg border border-[var(--color-border)] p-3 text-sm">
    <input class="mt-0.5 h-4 w-4" type="checkbox" bind:checked={enabled} disabled={locked||projectIsolation} />
    <span class="grid gap-1">
      <span class="font-medium text-[var(--color-text)]">{$t("Enable command sandbox")}</span>
      <span class="text-xs leading-5 text-[var(--color-text-muted)]">
        {#if projectIsolation}{$t('connection.sandboxHelp')}{:else}{$t("Sandbox changes normally apply to new commands immediately. Turning the sandbox off automatically starts or restarts the MCP service so the unsandboxed runtime is active.")}{/if}
      </span>
    </span>
  </label>

  <label class="grid gap-2 text-sm">
    <span class="font-medium text-[var(--color-text)]">{$t("Sandbox backend")}</span>
    <select
      class="tx-input"
      bind:value={backend}
      disabled={locked || loading || backends.length === 0}
    >
      {#each backends as item (item.id)}
        <option value={item.id}>{item.label}</option>
      {/each}
    </select>
  </label>

  {#if !projectIsolation}<div class="grid gap-3">
    <div>
      <p class="text-sm font-medium text-[var(--color-text)]">{$t("External filesystem access")}</p>
      <p class="mt-1 text-xs leading-5 text-[var(--color-text-muted)]">
        {$t("One absolute path per line. These paths are added to the OS sandbox authorization for this workspace.")}
      </p>
    </div>
    <label class="grid gap-2 text-sm">
      <span class="font-medium text-[var(--color-text)]">{$t("Read-only external paths")}</span>
      <textarea class="tx-input min-h-24" bind:value={readOnlyPaths} disabled={locked} rows="3"></textarea>
    </label>
    <label class="grid gap-2 text-sm">
      <span class="font-medium text-[var(--color-text)]">{$t("Writable external paths")}</span>
      <textarea class="tx-input min-h-24" bind:value={writablePaths} disabled={locked} rows="3"></textarea>
    </label>
  </div>

  {/if}

  {#if selected}
    <div class="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-soft)] p-3 text-xs leading-5 text-[var(--color-text-muted)]">
      <div class="flex flex-wrap items-center gap-2">
        <span class="font-semibold text-[var(--color-text)]">{selected.label}</span>
        {#if selected.experimental}
          <span class="tx-badge">{$t("Experimental")}</span>
        {/if}
        {#if selected.enforcementReady}
          <span class="tx-badge">{$t("Enforcement ready")}</span>
        {:else}
          <span class="tx-badge">{$t("Research only")}</span>
        {/if}
      </div>
      <p class="mt-1">{selected.description}</p>
      {#if selected.options.length > 0}
        <div class="mt-3 grid gap-3">
          {#each selected.options as option (option.id)}
            <label class="grid gap-1.5 text-sm">
              <span class="font-medium text-[var(--color-text)]">{option.label}</span>
              <input
                class="tx-input"
                type="text"
                value={options[option.id] ?? option.defaultValue}
                placeholder={option.placeholder}
                disabled={locked}
                oninput={(event) => {
                  options = { ...options, [option.id]: event.currentTarget.value };
                }}
              />
              <span class="text-xs text-[var(--color-text-muted)]">{option.description}</span>
            </label>
          {/each}
        </div>
      {/if}
      {#if !selected.hostSupported}
        <p class="mt-2 font-medium text-[var(--color-danger)]">{$t("This backend is not supported on the current host.")}</p>
      {:else if wslUnsupported}
        <p class="mt-2 font-medium text-[var(--color-warning)]">
          {$t("This backend cannot isolate WSL folders. Use Docker, Podman, Docker Sandboxes, or WSL Containers for those folders.")}
        </p>
      {:else if enabled && !selected.enforcementReady}
        <p class="mt-2 font-medium text-[var(--color-warning)]">
          {$t("This backend can be configured for research, but commands will be blocked until its production executor is ready.")}
        </p>
      {/if}
    </div>
  {:else if !loading}
    <p class="text-xs text-[var(--color-danger)]">{$t("No sandbox backend is available.")}</p>
  {/if}

  {#if error}
    <p class="text-xs text-[var(--color-danger)]">{error}</p>
  {/if}

  <div class="flex justify-end">
    <button class="tx-btn-primary" type="button" disabled={!dirty || saving || loading || locked} onclick={save}>
      {saving ? $t("Working…") : $t("Save sandbox settings")}
    </button>
  </div>
</div>
