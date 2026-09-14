<script lang="ts">
  import { alert as message } from "$lib/api/native";
  import SecretInput from "$lib/components/SecretInput.svelte";
  import {
    getWorkspaceSecret,
    regenerateWorkspaceSecret,
    getSharedSecret,
    setSharedSecret,
    regenerateSharedSecret,
    type WorkspaceSecretKey,
    type SharedSecretKey,
  } from "$lib/api/secrets";
  import { getBackend, readSecretIfAvailable, workspaceAuthSecretKeys } from "$lib/backend";
  import type { AuthConfig } from "$lib/types";
  import { t, translate, type MessageKey } from "$lib/i18n";

  export interface SaveAuthOptions {
    skipRuntimeRestart?: boolean;
  }

  interface Props {
    workspaceId: string;
    auth: AuthConfig;
    onSaveProfile: (auth: AuthConfig, options?: SaveAuthOptions) => void | Promise<void>;
  }

  let { workspaceId, auth, onSaveProfile }: Props = $props();
  const capabilities = getBackend().capabilities;
  const SECONDS_PER_DAY = 24 * 60 * 60;
  const DEFAULT_OAUTH_TOKEN_TTL_DAYS = 7;
  const MAX_OAUTH_TOKEN_TTL_DAYS = 30;
  const AUTH_OPTIONS = $derived(
    capabilities.staticBearerAuth
      ? [
          { value: "oauth", label: "OAuth" },
          { value: "bearer", label: "Bearer Token" },
          { value: "noauth", label: "No authentication" },
        ]
      : [{ value: "oauth", label: "OAuth" }],
  );

  let draft = $state<AuthConfig>({ type: "oauth", oauth_client_id: "", use_shared_secrets: false });
  let draftOauthTokenTtlDays = $state(DEFAULT_OAUTH_TOKEN_TTL_DAYS);
  let saving = $state(false);
  let secrets = $state<Partial<Record<WorkspaceSecretKey, string>>>({});
  let loadedSecrets = $state<Partial<Record<WorkspaceSecretKey, string>>>({});
  let loadedSharedOauthClientId = $state("");
  let regenerating = $state<WorkspaceSecretKey | null>(null);
  let secretsLoadSeq = 0;
  let suppressSecretsReload = $state(false);

  const secretsDirty = $derived(
    (Object.keys(secrets) as WorkspaceSecretKey[]).some(
      (k) => secrets[k] !== loadedSecrets[k],
    ),
  );

  const dirty = $derived(
    draft.type !== auth.type ||
      (draft.use_shared_secrets
        ? draft.oauth_client_id !== loadedSharedOauthClientId
        : draft.oauth_client_id !== auth.oauth_client_id) ||
      draft.use_shared_secrets !== !!auth.use_shared_secrets ||
      (draft.type === "oauth" &&
        draftOauthTokenTtlDays * SECONDS_PER_DAY !==
          (auth.oauth_token_ttl_seconds ?? DEFAULT_OAUTH_TOKEN_TTL_DAYS * SECONDS_PER_DAY)) ||
      secretsDirty,
  );

  const showOAuth = $derived(draft.type === "oauth");
  const showBearer = $derived(draft.type === "bearer");

  $effect(() => {
    const type = capabilities.staticBearerAuth ? auth.type : "oauth";
    draft = {
      type,
      oauth_client_id: auth.oauth_client_id,
      use_shared_secrets: capabilities.sharedSecretStore && !!auth.use_shared_secrets,
    };
    draftOauthTokenTtlDays = Math.min(
      MAX_OAUTH_TOKEN_TTL_DAYS,
      Math.max(
        1,
        Math.round(
          (auth.oauth_token_ttl_seconds ?? DEFAULT_OAUTH_TOKEN_TTL_DAYS * SECONDS_PER_DAY) /
            SECONDS_PER_DAY,
        ),
      ),
    );
  });

  $effect(() => {
    if (suppressSecretsReload) return;
    const id = workspaceId;
    const authType = draft.type;
    const useShared = draft.use_shared_secrets ?? false;
    void loadSecrets(id, authType, useShared);
  });

  async function loadSecrets(id: string, authType: string, useShared: boolean) {
    const seq = ++secretsLoadSeq;
    const sharedClientId =
      authType === "oauth" && useShared && capabilities.sharedSecretStore
        ? await getSharedSecret("oauth_client_id")
        : null;
    const keys = workspaceAuthSecretKeys(authType, capabilities);
    if (keys.length === 0) {
      if (seq !== secretsLoadSeq) return;
      secrets = {};
      loadedSecrets = {};
      return;
    }
    const loaded = await Promise.all(
      keys.map(async (key) => {
        const value = await readSecretIfAvailable(() =>
          useShared && capabilities.sharedSecretStore
            ? getSharedSecret(key as SharedSecretKey)
            : getWorkspaceSecret(id, key),
        );
        return [key, value] as const;
      }),
    );
    if (seq !== secretsLoadSeq) return;
    if (authType === "oauth" && useShared) {
      draft = { ...draft, oauth_client_id: sharedClientId ?? "" };
      loadedSharedOauthClientId = sharedClientId ?? "";
    } else {
      loadedSharedOauthClientId = "";
    }
    secrets = Object.fromEntries(loaded);
    loadedSecrets = Object.fromEntries(loaded);
  }

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    suppressSecretsReload = true;
    try {
      let sharedSecretChanged = false;
      const clientId =
        draft.type === "oauth" && draft.use_shared_secrets
          ? draft.oauth_client_id.trim()
          : "";
      if (draft.type === "oauth" && draft.use_shared_secrets) {
        if (!clientId) throw new Error(translate("OAuth Client ID cannot be empty"));
        sharedSecretChanged = clientId !== loadedSharedOauthClientId;
      }
      if (
        draft.type === "oauth" &&
        (!Number.isInteger(draftOauthTokenTtlDays) ||
          draftOauthTokenTtlDays < 1 ||
          draftOauthTokenTtlDays > MAX_OAUTH_TOKEN_TTL_DAYS)
      ) {
        throw new Error(translate("OAuth token lifetime must be between 1 and 30 days"));
      }
      // Persist the shared-secret flag first. If the secret value changes, the backend
      // owns the single runtime restart; the page must not race it with a second restart.
      await onSaveProfile(
        {
          ...draft,
          oauth_token_ttl_seconds:
            draft.type === "oauth"
              ? draftOauthTokenTtlDays * SECONDS_PER_DAY
              : draft.oauth_token_ttl_seconds,
        },
        { skipRuntimeRestart: sharedSecretChanged },
      );
      if (sharedSecretChanged) {
        await setSharedSecret("oauth_client_id", clientId);
        loadedSharedOauthClientId = clientId;
      }
      // Auth save only persists profile fields; secrets are already stored by regenerate.
      loadedSecrets = { ...secrets };
    } catch (error) {
      await message(String(error), { title: $t("Failed to save"), kind: "error" });
    } finally {
      suppressSecretsReload = false;
      saving = false;
    }
  }

  async function regenerate(key: WorkspaceSecretKey) {
    if (regenerating) return;
    regenerating = key;
    try {
      const value = draft.use_shared_secrets
        ? await regenerateSharedSecret(key as SharedSecretKey)
        : await regenerateWorkspaceSecret(workspaceId, key);
      secrets = { ...secrets, [key]: value };
      // Regeneration persists immediately and the backend owns the debounced restart.
      // Treat the returned value as saved so the profile Save button cannot restart again.
      loadedSecrets = { ...loadedSecrets, [key]: value };
    } catch (error) {
      await message(String(error), { title: $t("Regeneration failed"), kind: "error" });
    } finally {
      regenerating = null;
    }
  }
</script>

<form
  class="grid gap-3"
  onsubmit={(event) => {
    event.preventDefault();
    void save();
  }}
>
  <p class="text-xs text-[var(--color-text-muted)]">
    {$t("Use the GPT configuration card above to copy Client IDs and secrets. Change the authentication type and regenerate secrets here.")}
  </p>

  <label class="grid gap-1">
    <span class="text-xs text-[var(--color-text-muted)]">{$t("Authentication type")}</span>
    <select
      class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
      bind:value={draft.type}
    >
      {#each AUTH_OPTIONS as option}
        <option value={option.value}>{option.value === "noauth" ? $t(option.label as MessageKey) : option.label}</option>
      {/each}
    </select>
  </label>

  {#if capabilities.sharedSecretStore}
    <label class="flex items-center gap-2">
      <input
        type="checkbox"
        class="h-4 w-4"
        bind:checked={draft.use_shared_secrets}
      />
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Use global shared secrets (managed under Settings → Shared secrets)")}</span>
    </label>
  {/if}

  {#if showOAuth}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth client ID")}</span>
      <input
        type="text"
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
        bind:value={draft.oauth_client_id}
        readonly={draft.use_shared_secrets}
      />
    </label>

    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth access token lifetime (days)")}</span>
      <input
        type="number"
        min="1"
        max={MAX_OAUTH_TOKEN_TTL_DAYS}
        step="1"
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
        bind:value={draftOauthTokenTtlDays}
      />
      <span class="text-xs text-[var(--color-text-muted)]">
        {$t("Applies to newly issued access tokens. Existing tokens keep their original expiry.")}
      </span>
    </label>

    {#if capabilities.staticBearerAuth}
      <div class="grid gap-1">
        <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth client secret")}</span>
        <SecretInput
          value={secrets.oauth_client_secret ?? ""}
          placeholder={$t("Loading…")}
          readonly
          onRegenerate={() => void regenerate("oauth_client_secret")}
          regenerating={regenerating === "oauth_client_secret"}
        />
      </div>
    {/if}

    <div class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Authorization password")}</span>
      <SecretInput
        value={secrets.oauth_password ?? ""}
        placeholder={$t("Enter this password when ChatGPT authorizes for the first time")}
        readonly
        onRegenerate={() => void regenerate("oauth_password")}
        regenerating={regenerating === "oauth_password"}
      />
    </div>
  {/if}

  {#if showBearer}
    <div class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">Bearer Token</span>
      <SecretInput
        value={secrets.bearer_token ?? ""}
        placeholder={$t("Loading…")}
        readonly
        onRegenerate={() => void regenerate("bearer_token")}
        regenerating={regenerating === "bearer_token"}
      />
    </div>
  {/if}

  <div class="flex justify-end pt-1">
    <button
      type="submit"
      class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
      disabled={saving || !dirty}
    >
      {saving ? $t("Saving…") : $t("Save configuration")}
    </button>
  </div>
</form>
