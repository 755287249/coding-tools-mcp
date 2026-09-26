<script lang="ts">
  import { alert as message } from "$lib/api/native";
  import CopyButton from "$lib/components/CopyButton.svelte";
  import SecretInput from "$lib/components/SecretInput.svelte";
  import { getSecret, regenerateSecret, getSharedSecret, regenerateSharedSecret } from "$lib/api/secrets";
  import type { ActionsAuthDraft } from "$lib/types";
  import { t } from "$lib/i18n";

  export const ACTIONS_AUTH_OPTIONS = [
    { value: "api_key", label: "API Key / Bearer" },
    { value: "none", label: "No authentication" },
    { value: "oauth", label: "OAuth" },
  ] as const;

  export type { ActionsAuthDraft } from "$lib/types";

  interface Props {
    workspaceId: string;
    authType: string;
    oauthClientId: string;
    oauthScopes: string;
    openapiUrl: string;
    privacyUrl: string;
    oauthAuthorizeUrl: string;
    oauthTokenUrl: string;
    useSharedSecrets?: boolean;
    onSave: (draft: ActionsAuthDraft) => void | Promise<void>;
  }

  let {
    workspaceId,
    authType,
    oauthClientId,
    oauthScopes,
    openapiUrl,
    privacyUrl,
    oauthAuthorizeUrl,
    oauthTokenUrl,
    useSharedSecrets = false,
    onSave,
  }: Props = $props();

  let draftAuthType = $state("api_key");
  let draftOauthClientId = $state("");
  let draftOauthScopes = $state("");
  let draftUseShared = $state(false);
  let apiKey = $state("");
  let loadedApiKey = $state("");
  let oauthClientSecret = $state("");
  let loadedOauthClientSecret = $state("");
  let oauthPassword = $state("");
  let loadedOauthPassword = $state("");
  let oauthTokenSecret = $state("");
  let loadedOauthTokenSecret = $state("");
  let loadingKey = $state(true);
  let loadingOAuthSecret = $state(true);
  let loadingOAuthPassword = $state(true);
  let loadingOAuthTokenSecret = $state(true);
  let regenerating = $state(false);
  let regeneratingOAuthSecret = $state(false);
  let regeneratingOAuthPassword = $state(false);
  let regeneratingOAuthTokenSecret = $state(false);
  let saving = $state(false);
  let secretsLoadSeq = 0;
  let suppressSecretsReload = $state(false);

  const secretsDirty = $derived(
    apiKey !== loadedApiKey ||
      oauthClientSecret !== loadedOauthClientSecret ||
      oauthPassword !== loadedOauthPassword ||
      oauthTokenSecret !== loadedOauthTokenSecret,
  );

  const dirty = $derived(
    draftAuthType !== authType ||
      draftOauthClientId !== oauthClientId ||
      draftOauthScopes !== oauthScopes ||
      draftUseShared !== useSharedSecrets ||
      secretsDirty,
  );
  const showApiKey = $derived(draftAuthType === "api_key");
  const showOAuth = $derived(draftAuthType === "oauth");

  $effect(() => {
    draftAuthType = authType;
    draftOauthClientId = oauthClientId;
    draftOauthScopes = oauthScopes;
    draftUseShared = useSharedSecrets;
  });

  $effect(() => {
    if (suppressSecretsReload) return;
    workspaceId;
    draftUseShared;
    void loadSecrets();
  });

  async function loadSecrets() {
    const seq = ++secretsLoadSeq;
    loadingKey = true;
    loadingOAuthSecret = true;
    loadingOAuthPassword = true;
    loadingOAuthTokenSecret = true;
    try {
      const [key, secret, password, tokenSecret] = await Promise.all([
        draftUseShared
          ? getSharedSecret("actions_api_key")
          : getSecret(workspaceId, "actions_api_key"),
        draftUseShared
          ? getSharedSecret("actions_oauth_client_secret")
          : getSecret(workspaceId, "actions_oauth_client_secret"),
        draftUseShared
          ? getSharedSecret("actions_oauth_password")
          : getSecret(workspaceId, "actions_oauth_password"),
        draftUseShared
          ? getSharedSecret("actions_oauth_token_secret")
          : getSecret(workspaceId, "actions_oauth_token_secret"),
      ]);
      if (seq !== secretsLoadSeq) return;
      apiKey = key ?? "";
      loadedApiKey = key ?? "";
      oauthClientSecret = secret ?? "";
      loadedOauthClientSecret = secret ?? "";
      oauthPassword = password ?? "";
      loadedOauthPassword = password ?? "";
      oauthTokenSecret = tokenSecret ?? "";
      loadedOauthTokenSecret = tokenSecret ?? "";
    } finally {
      if (seq !== secretsLoadSeq) return;
      loadingKey = false;
      loadingOAuthSecret = false;
      loadingOAuthPassword = false;
      loadingOAuthTokenSecret = false;
    }
  }

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    suppressSecretsReload = true;
    try {
      await onSave({
        authType: draftAuthType,
        oauthClientId: draftOauthClientId.trim(),
        oauthScopes: draftOauthScopes.trim(),
        useSharedSecrets: draftUseShared,
      });
      loadedApiKey = apiKey;
      loadedOauthClientSecret = oauthClientSecret;
      loadedOauthPassword = oauthPassword;
      loadedOauthTokenSecret = oauthTokenSecret;
    } finally {
      suppressSecretsReload = false;
      saving = false;
    }
  }

  async function regenerate() {
    if (regenerating) return;
    regenerating = true;
    try {
      apiKey = draftUseShared
        ? await regenerateSharedSecret("actions_api_key")
        : await regenerateSecret(workspaceId, "actions_api_key");
      loadedApiKey = apiKey;
    } catch (error) {
      await message(String(error), { title: $t("Regeneration failed"), kind: "error" });
    } finally {
      regenerating = false;
    }
  }

  async function regenerateOAuthSecret() {
    if (regeneratingOAuthSecret) return;
    regeneratingOAuthSecret = true;
    try {
      oauthClientSecret = draftUseShared
        ? await regenerateSharedSecret("actions_oauth_client_secret")
        : await regenerateSecret(workspaceId, "actions_oauth_client_secret");
      loadedOauthClientSecret = oauthClientSecret;
    } catch (error) {
      await message(String(error), { title: $t("Regeneration failed"), kind: "error" });
    } finally {
      regeneratingOAuthSecret = false;
    }
  }

  async function regenerateOAuthPassword() {
    if (regeneratingOAuthPassword) return;
    regeneratingOAuthPassword = true;
    try {
      oauthPassword = draftUseShared
        ? await regenerateSharedSecret("actions_oauth_password")
        : await regenerateSecret(workspaceId, "actions_oauth_password");
      loadedOauthPassword = oauthPassword;
    } catch (error) {
      await message(String(error), { title: $t("Regeneration failed"), kind: "error" });
    } finally {
      regeneratingOAuthPassword = false;
    }
  }

  async function regenerateOAuthTokenSecret() {
    if (regeneratingOAuthTokenSecret) return;
    regeneratingOAuthTokenSecret = true;
    try {
      oauthTokenSecret = draftUseShared
        ? await regenerateSharedSecret("actions_oauth_token_secret")
        : await regenerateSecret(workspaceId, "actions_oauth_token_secret");
      loadedOauthTokenSecret = oauthTokenSecret;
    } catch (error) {
      await message(String(error), { title: $t("Regeneration failed"), kind: "error" });
    } finally {
      regeneratingOAuthTokenSecret = false;
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
    {$t("Use the GPT configuration card above to copy OpenAPI URLs and secrets. Change authentication and secrets here.")}
  </p>

  <label class="grid gap-1">
    <span class="text-xs text-[var(--color-text-muted)]">{$t("Authentication method")}</span>
    <select
      class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
      bind:value={draftAuthType}
    >
      {#each ACTIONS_AUTH_OPTIONS as option}
        <option value={option.value}>{option.value === "none" ? $t("No authentication") : option.label}</option>
      {/each}
    </select>
  </label>

  <label class="flex items-center gap-2">
    <input
      type="checkbox"
      class="h-4 w-4"
      bind:checked={draftUseShared}
    />
    <span class="text-xs text-[var(--color-text-muted)]">{$t("Use global shared secrets (managed under Settings → Shared secrets)")}</span>
  </label>

  {#if showApiKey}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("API Key (Bearer)")}</span>
      <SecretInput
        value={loadingKey ? $t("Loading…") : apiKey}
        readonly
        disabled={loadingKey}
        showCopy={!!apiKey}
        onRegenerate={() => void regenerate()}
        regenerating={regenerating}
      />
    </label>
    <p class="text-xs text-[var(--color-text-muted)]">
      {$t("In GPT Actions authentication, choose API Key → Bearer and use this value as the key.")}
    </p>
  {:else if showOAuth}
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth Client ID (enter in GPT)")}</span>
      <div class="flex gap-2">
        <input
          type="text"
          class="min-w-0 flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm"
          bind:value={draftOauthClientId}
        />
        {#if draftOauthClientId}
          <CopyButton value={draftOauthClientId} />
        {/if}
      </div>
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth Client Secret (enter in GPT)")}</span>
      <SecretInput
        value={loadingOAuthSecret ? $t("Loading…") : oauthClientSecret}
        readonly
        disabled={loadingOAuthSecret}
        showCopy={!!oauthClientSecret}
        onRegenerate={() => void regenerateOAuthSecret()}
        regenerating={regeneratingOAuthSecret}
      />
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth Password (server validation)")}</span>
      <SecretInput
        value={loadingOAuthPassword ? $t("Loading…") : oauthPassword}
        readonly
        disabled={loadingOAuthPassword}
        showCopy={!!oauthPassword}
        onRegenerate={() => void regenerateOAuthPassword()}
        regenerating={regeneratingOAuthPassword}
      />
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("OAuth Token Secret (JWT signing)")}</span>
      <SecretInput
        value={loadingOAuthTokenSecret ? $t("Loading…") : oauthTokenSecret}
        readonly
        disabled={loadingOAuthTokenSecret}
        showCopy={!!oauthTokenSecret}
        onRegenerate={() => void regenerateOAuthTokenSecret()}
        regenerating={regeneratingOAuthTokenSecret}
      />
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Authorization URL (enter in GPT)")}</span>
      <div class="flex gap-2">
        <input
          type="text"
          readonly
          class="min-w-0 flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs"
          value={oauthAuthorizeUrl}
        />
        {#if oauthAuthorizeUrl}
          <CopyButton value={oauthAuthorizeUrl} />
        {/if}
      </div>
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Token URL (enter in GPT)")}</span>
      <div class="flex gap-2">
        <input
          type="text"
          readonly
          class="min-w-0 flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs"
          value={oauthTokenUrl}
        />
        {#if oauthTokenUrl}
          <CopyButton value={oauthTokenUrl} />
        {/if}
      </div>
    </label>
    <label class="grid gap-1">
      <span class="text-xs text-[var(--color-text-muted)]">{$t("Scope (enter in GPT, space separated)")}</span>
      <input
        type="text"
        class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
        placeholder={$t("For example: coding-tools")}
        bind:value={draftOauthScopes}
      />
    </label>
    <p class="text-xs text-[var(--color-text-muted)]">
      {$t("The GPT editor generates the Callback URL automatically; no configuration is needed here. Keep the default token exchange method.")}
    </p>
  {:else}
    <p class="text-xs text-[var(--color-text-muted)]">
      {$t("Requests are not authenticated. Select None in GPT. Use this only for local debugging; public endpoints should use API Key or OAuth.")}
    </p>
  {/if}

  <div class="flex justify-end pt-1">
    <button
      type="submit"
      class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-[var(--primary-contrast)] transition-opacity hover:opacity-90 disabled:opacity-50"
      disabled={saving || !dirty}
    >
      {saving ? $t("Saving…") : $t("Save configuration")}
    </button>
  </div>
</form>
