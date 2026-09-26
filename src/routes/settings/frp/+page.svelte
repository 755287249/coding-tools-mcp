<script lang="ts">
  import { onMount } from "svelte";
  import { alert as message } from "$lib/api/native";
  import {
    deleteFrpProfile,
    listFrpProfiles,
    saveFrpProfile,
    type FrpProfileDto,
  } from "$lib/api/settings";
  import SecretInput from "$lib/components/SecretInput.svelte";
  import { t } from "$lib/i18n";

  let profiles = $state<FrpProfileDto[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let editingId = $state<string | null>(null);
  let name = $state("");
  let server = $state("");
  let serverPort = $state(443);
  let token = $state("");

  async function refresh() {
    loading = true;
    try {
      profiles = await listFrpProfiles();
    } finally {
      loading = false;
    }
  }

  function resetForm() {
    editingId = null;
    name = "";
    server = "";
    serverPort = 443;
    token = "";
  }

  function editProfile(profile: FrpProfileDto) {
    editingId = profile.id;
    name = profile.name;
    server = profile.server;
    serverPort = profile.serverPort;
    token = "";
  }

  async function save() {
    if (!name.trim() || !server.trim()) {
      await message($t("Please enter a configuration name and server hostname."), {
        title: $t("Cannot save"),
        kind: "warning",
      });
      return;
    }
    saving = true;
    try {
      await saveFrpProfile(
        {
          id: editingId ?? "",
          name: name.trim(),
          server: server.trim(),
          serverPort,
        },
        token.trim() || undefined,
      );
      resetForm();
      await refresh();
    } catch (error) {
      await message(String(error), { title: $t("Failed to save"), kind: "error" });
    } finally {
      saving = false;
    }
  }

  async function removeProfile(profile: FrpProfileDto) {
    try {
      await deleteFrpProfile(profile.id);
      if (editingId === profile.id) {
        resetForm();
      }
      await refresh();
    } catch (error) {
      await message(String(error), { title: $t("Deletion failed"), kind: "error" });
    }
  }

  onMount(refresh);
</script>

<section class="page-scroll">
  <header class="page-header">
    <p class="page-kicker">{$t("Global settings")}</p>
    <h2 class="page-title">{$t("FRP configuration")}</h2>
    <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-muted)]">
      {$t(
        "Configure FRP servers, ports, and tokens here. Workspaces select a profile and provide a subdomain; saving a changed subdomain updates frpc and restarts the tunnel.",
      )}
    </p>
  </header>

  <div class="page-body flex flex-col gap-6">
    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">{editingId ? $t("Edit configuration") : $t("Create configuration")}</h3>
      <form
        class="mt-4 grid gap-3"
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Name")}</span>
          <input
            type="text"
            class="tx-input"
            placeholder={$t("Company FRP")}
            bind:value={name}
          />
        </label>
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Server hostname")}</span>
          <input
            type="text"
            class="tx-input tx-mono"
            placeholder="frp.example.com"
            bind:value={server}
          />
        </label>
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Port")}</span>
          <input
            type="number"
            min="1"
            max="65535"
            class="tx-input"
            bind:value={serverPort}
          />
        </label>
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">
            Token {editingId ? `(${$t("Leave empty to keep the current value")})` : ""}
          </span>
          <SecretInput
            bind:value={token}
            placeholder="frp auth token"
            showCopy={false}
          />
        </label>
        <div class="flex gap-2 pt-1">
          <button
            type="submit"
            class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-[var(--primary-contrast)] disabled:opacity-50"
            disabled={saving}
          >
            {saving ? $t("Saving…") : editingId ? $t("Update") : $t("Add")}
          </button>
          {#if editingId}
            <button
              type="button"
              class="tx-btn-ghost"
              onclick={resetForm}
            >
              {$t("Cancel")}
            </button>
          {/if}
        </div>
      </form>
    </div>

    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">{$t("Saved configurations")}</h3>
      {#if loading}
        <p class="mt-4 text-sm text-[var(--color-text-muted)]">{$t("Loading…")}</p>
      {:else if profiles.length === 0}
        <p class="mt-4 text-sm text-[var(--color-text-muted)]">{$t("No FRP configurations.")}</p>
      {:else}
        <ul class="mt-4 space-y-2">
          {#each profiles as profile (profile.id)}
            <li
              class="tx-panel flex items-center justify-between gap-3 px-3 py-2"
            >
              <div class="min-w-0">
                <p class="truncate text-sm font-medium">{profile.name}</p>
                <p class="truncate font-mono text-xs text-[var(--color-text-muted)]">
                  {profile.server}:{profile.serverPort}
                  · Token {profile.hasToken ? $t("Configured") : $t("Not configured")}
                </p>
              </div>
              <div class="flex shrink-0 gap-2">
                <button
                  type="button"
                  class="text-xs text-[var(--color-accent)] hover:underline"
                  onclick={() => editProfile(profile)}
                >
                  {$t("Edit")}
                </button>
                <button
                  type="button"
                  class="text-xs text-red-400 hover:underline"
                  onclick={() => removeProfile(profile)}
                >
                  {$t("Delete")}
                </button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
</section>
