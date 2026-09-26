<script lang="ts">
  import { onMount } from "svelte";
  import { alert as message } from "$lib/api/native";
  import type { DownloadConfig, SoftwareStatus } from "$lib/api/software";
  import {
    listSoftware,
    installSoftware,
    uninstallSoftware,
    getDownloadConfig,
    setDownloadConfig,
  } from "$lib/api/software";
  import { t } from "$lib/i18n";

  let software = $state<SoftwareStatus[]>([]);
  let loading = $state(true);
  let installing = $state<string | null>(null);
  let uninstalling = $state<string | null>(null);

  let downloadConfig = $state<DownloadConfig>({
    githubMirror: "",
    proxyMode: "system",
    proxyUrl: "",
  });
  let configChanged = $state(false);

  const tunnelSoftware = $derived(software.filter((item) => (item.group ?? "tunnel") === "tunnel"));
  const sandboxSoftware = $derived(software.filter((item) => item.group === "sandbox"));

  async function refresh() {
    loading = true;
    try {
      software = await listSoftware();
      downloadConfig = await getDownloadConfig();
      configChanged = false;
    } finally {
      loading = false;
    }
  }

  async function install(kind: string) {
    installing = kind;
    try {
      const status = await installSoftware(kind);
      await refresh();
      if (status.nextSteps?.trim()) {
        await message(status.nextSteps, { title: $t("Next steps"), kind: "info" });
      }
    } catch (e) {
      await message(String(e), { title: $t("Installation failed"), kind: "error" });
    } finally {
      installing = null;
    }
  }

  async function uninstall(kind: string) {
    uninstalling = kind;
    try {
      await uninstallSoftware(kind);
      await refresh();
    } catch (e) {
      await message(String(e), { title: $t("Uninstallation failed"), kind: "error" });
    } finally {
      uninstalling = null;
    }
  }

  async function saveConfig() {
    try {
      await setDownloadConfig(downloadConfig);
      configChanged = false;
      await message($t("Download settings saved."), { title: $t("Saved"), kind: "info" });
    } catch (e) {
      await message(String(e), { title: $t("Failed to save"), kind: "error" });
    }
  }

  onMount(refresh);
</script>

<section class="page-scroll">
  <header class="page-header">
    <p class="page-kicker">{$t("Global settings")}</p>
    <h2 class="page-title">{$t("Software management")}</h2>
    <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-muted)]">
      {$t(
        "Install tunnel clients and sandbox CLIs. Cache-managed tunnel binaries can be uninstalled here; sandbox tools use the official package manager.",
      )}
    </p>
  </header>

  <div class="page-body flex flex-col gap-6">
    <!-- Binary status -->
    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">{$t("Status")}</h3>
      {#if loading}
        <p class="mt-4 text-sm text-[var(--color-text-muted)]">{$t("Loading…")}</p>
      {:else if software.length === 0}
        <p class="mt-4 text-sm text-[var(--color-text-muted)]">{$t("No information available.")}</p>
      {:else}
        {@render softwareGroup($t("Tunnel clients"), tunnelSoftware)}
        {@render softwareGroup($t("Sandbox tools"), sandboxSoftware)}
      {/if}
    </div>

    <!-- Download config -->
    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">{$t("Download settings")}</h3>
      <form
        class="mt-4 grid gap-3"
        onsubmit={(e) => { e.preventDefault(); void saveConfig(); }}
      >
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("GitHub mirror")}</span>
          <input
            type="text"
            class="tx-input tx-mono"
            placeholder="https://your-trusted-mirror.example"
            bind:value={downloadConfig.githubMirror}
            oninput={() => (configChanged = true)}
          />
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Leave empty to use the official GitHub release directly. A configured mirror is fallback only and downloaded binaries are still verified.")}</span>
        </label>
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Proxy mode")}</span>
          <select
            class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
            bind:value={downloadConfig.proxyMode}
            onchange={() => (configChanged = true)}
          >
            <option value="system">{$t("System proxy (default)")}</option>
            <option value="none">{$t("No proxy")}</option>
            <option value="manual">{$t("Manual proxy URL")}</option>
          </select>
        </label>
        {#if downloadConfig.proxyMode === "manual"}
          <label class="grid gap-1">
            <span class="text-xs text-[var(--color-text-muted)]">{$t("Proxy URL")}</span>
            <input
              type="text"
              class="tx-input tx-mono"
              placeholder="http://127.0.0.1:7890"
              bind:value={downloadConfig.proxyUrl}
              oninput={() => (configChanged = true)}
            />
          </label>
        {/if}
        <div class="flex justify-end pt-1">
          <button
            type="submit"
            class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-[var(--primary-contrast)] disabled:opacity-50"
            disabled={!configChanged}
          >
            {$t("Save settings")}
          </button>
        </div>
      </form>
    </div>
  </div>
</section>

{#snippet softwareGroup(title: string, items: SoftwareStatus[])}
  {#if items.length > 0}
    <div class="mt-4">
      <p class="text-xs font-medium text-[var(--color-text-muted)]">{title}</p>
      <ul class="mt-2 space-y-2">
        {#each items as s (s.kind)}
          <li class="tx-panel flex items-center justify-between gap-3 px-3 py-2">
            <div class="min-w-0">
              <p class="text-sm font-medium">{s.name}</p>
              <p class="font-mono text-xs text-[var(--color-text-muted)]">
                {s.installed ? s.path : $t("Not installed")}
                · {s.managed ? $t("Managed") : $t("System installation")}
                {#if s.version}{` · v${s.version}`}{/if}
              </p>
              {#if s.installed && s.outdated}
                <p class="mt-1 text-xs leading-5 text-amber-400">{$t("This version is outdated. Update it before starting a tunnel.")}</p>
              {/if}
              {#if s.hint}
                <p class="mt-1 text-xs leading-5 text-[var(--color-text-muted)]">{s.hint}</p>
              {/if}
              {#if s.installed && s.nextSteps && s.nextSteps !== s.hint}
                <p class="mt-1 whitespace-pre-line text-xs leading-5 text-[var(--color-text-muted)]">{s.nextSteps}</p>
              {/if}
            </div>
            <div class="flex shrink-0 gap-2">
              {#if s.installed}
                {#if s.managed && s.outdated}
                  <button
                    type="button"
                    class="text-xs text-[var(--color-accent)] hover:underline disabled:opacity-50"
                    disabled={installing === s.kind}
                    onclick={() => install(s.kind)}
                  >
                    {installing === s.kind ? $t("Updating…") : $t("Update")}
                  </button>
                {/if}
                {#if s.managed}
                  <button
                    type="button"
                    class="text-xs text-red-400 hover:underline disabled:opacity-50"
                    disabled={uninstalling === s.kind}
                    onclick={() => uninstall(s.kind)}
                  >
                    {uninstalling === s.kind ? $t("Uninstalling…") : $t("Uninstall")}
                  </button>
                {:else}
                  <span class="text-xs text-[var(--color-text-muted)]">{$t("System installation")}</span>
                {/if}
              {:else if s.installable !== false}
                <button
                  type="button"
                  class="text-xs text-[var(--color-accent)] hover:underline disabled:opacity-50"
                  disabled={installing === s.kind}
                  onclick={() => install(s.kind)}
                >
                  {installing === s.kind ? $t("Installing…") : $t("Install")}
                </button>
              {:else}
                <span class="text-xs text-[var(--color-text-muted)]">{$t("Not installed")}</span>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
{/snippet}
