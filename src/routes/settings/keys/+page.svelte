<script lang="ts">
  import { onMount } from "svelte";
  import { alert as message } from "$lib/api/native";
  import SecretInput from "$lib/components/SecretInput.svelte";
  import {
    getSharedSecret,
    setSharedSecret,
    regenerateSharedSecret,
    type SharedSecretKey,
  } from "$lib/api/secrets";
  import { t, type MessageKey } from "$lib/i18n";

  const MCP_KEYS: { key: SharedSecretKey; label: MessageKey }[] = [
    { key: "oauth_client_id", label: "MCP OAuth Client ID" },
    { key: "bearer_token", label: "MCP Bearer Token" },
    { key: "oauth_client_secret", label: "MCP OAuth client secret" },
    { key: "oauth_password", label: "MCP authorization password" },
    { key: "oauth_token_secret", label: "MCP Token Secret" },
  ];

  const ACTIONS_KEYS: { key: SharedSecretKey; label: MessageKey }[] = [
    { key: "actions_api_key", label: "Actions API Key" },
    { key: "actions_oauth_client_secret", label: "Actions OAuth client secret" },
    { key: "actions_oauth_password", label: "Actions authorization password" },
    { key: "actions_oauth_token_secret", label: "Actions Token Secret" },
  ];

  const ALL_KEYS = [...MCP_KEYS, ...ACTIONS_KEYS];

  let secrets = $state<Record<string, string>>({});
  let originals = $state<Record<string, string>>({});
  let loading = $state(true);
  let saving = $state(false);
  let regenerating = $state<string | null>(null);

  const dirty = $derived(ALL_KEYS.some(({ key }) => secrets[key] !== undefined && secrets[key] !== originals[key]));

  async function loadAll() {
    loading = true;
    try {
      const results = await Promise.all(
        ALL_KEYS.map(async ({ key }) => {
          let value = "";
          try {
            value = (await getSharedSecret(key)) ?? "";
          } catch {
            // Individual key load failure — show empty for this key
            // rather than failing the entire page.
          }
          return [key, value] as const;
        }),
      );
      for (const [key, value] of results) {
        secrets[key] = value;
        originals[key] = value;
      }
    } finally {
      loading = false;
    }
  }

  async function regenerate(key: SharedSecretKey) {
    if (regenerating) return;
    regenerating = key;
    try {
      const value = await regenerateSharedSecret(key);
      secrets[key] = value;
      // Keep originals stale so the save button lights up,
      // giving the user visible confirmation before we navigate away.
      // saveAll will write the same value (idempotent) and update originals.
    } catch (e) {
      await message(String(e), { title: $t("Regeneration failed"), kind: "error" });
    } finally {
      regenerating = null;
    }
  }

  async function saveAll() {
    saving = true;
    try {
      for (const { key } of ALL_KEYS) {
        if (secrets[key] !== undefined && secrets[key] !== originals[key]) {
          await setSharedSecret(key, secrets[key]);
          originals[key] = secrets[key];
        }
      }
    } catch (e) {
      await message(String(e), { title: $t("Failed to save"), kind: "error" });
    } finally {
      saving = false;
    }
  }

  onMount(loadAll);
</script>

<section class="page-scroll">
  <header class="page-header">
    <p class="page-kicker">{$t("Global settings")}</p>
    <h2 class="page-title">{$t("Shared secrets")}</h2>
    <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-muted)]">
      {$t(
        "Manage shared authentication secrets here. Workspaces can use these shared values or their own. After a secret changes, the corresponding running services restart automatically.",
      )}
    </p>
  </header>

  <div class="page-body flex flex-col gap-6">
    <div class="flex flex-col gap-6">
      <!-- MCP keys -->
      <div class="tx-card p-4">
        <h3 class="text-sm font-semibold">{$t("MCP authentication secrets")}</h3>
        {#if loading}
          <p class="mt-4 text-sm text-[var(--color-text-muted)]">{$t("Loading…")}</p>
        {:else}
          <div class="mt-4 grid gap-4">
            {#each MCP_KEYS as { key, label }}
              <div class="grid gap-1">
                <span class="text-xs text-[var(--color-text-muted)]">{$t(label)}</span>
                <SecretInput
                  bind:value={secrets[key]}
                  disabled={loading}
                  onRegenerate={() => regenerate(key)}
                  regenerating={regenerating === key}
                />
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Actions keys -->
      <div class="tx-card p-4">
        <h3 class="text-sm font-semibold">{$t("Actions authentication secrets")}</h3>
        {#if loading}
          <p class="mt-4 text-sm text-[var(--color-text-muted)]">{$t("Loading…")}</p>
        {:else}
          <div class="mt-4 grid gap-4">
            {#each ACTIONS_KEYS as { key, label }}
              <div class="grid gap-1">
                <span class="text-xs text-[var(--color-text-muted)]">{$t(label)}</span>
                <SecretInput
                  bind:value={secrets[key]}
                  disabled={loading}
                  onRegenerate={() => regenerate(key)}
                  regenerating={regenerating === key}
                />
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>

    <div class="flex justify-end">
      <button
        type="button"
        class="rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-[var(--primary-contrast)] transition-opacity hover:opacity-90 disabled:opacity-50"
        disabled={!dirty || saving}
        onclick={() => saveAll()}
      >
        {saving ? $t("Saving…") : $t("Save changes")}
      </button>
    </div>
  </div>
</section>
