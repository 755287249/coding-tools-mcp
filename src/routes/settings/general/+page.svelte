<script lang="ts">
  import { onMount } from "svelte";
  import { alert as message } from "$lib/api/native";
  import { getProxy, setProxy, type ProxyConfigDto } from "$lib/api/settings";
  import { t } from "$lib/i18n";

  let proxy = $state<ProxyConfigDto>({ mode: "none", url: "", tunnelProtocol: "http2" });
  let changed = $state(false);
  let saving = $state(false);

  async function refresh() {
    try {
      const loaded = await getProxy();
      proxy = { ...loaded, tunnelProtocol: loaded.tunnelProtocol || "http2" };
      changed = false;
    } catch (e) {
      await message(String(e), { title: $t("Failed to load"), kind: "error" });
    }
  }

  async function save() {
    saving = true;
    try {
      await setProxy(proxy);
      changed = false;
      await message($t("Proxy settings saved."), { title: $t("Saved"), kind: "info" });
    } catch (e) {
      await message(String(e), { title: $t("Failed to save"), kind: "error" });
    } finally {
      saving = false;
    }
  }

  function handleChange() {
    changed = true;
  }

  onMount(refresh);
</script>

<section class="page-scroll">
  <header class="page-header">
    <p class="page-kicker">{$t("Global settings")}</p>
    <h2 class="page-title">{$t("General")}</h2>
    <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-muted)]">
      {$t(
        "Configure the global network proxy. It applies to Cloudflare tunnel connections but not software downloads.",
      )}
    </p>
  </header>

  <div class="page-body flex flex-col gap-6">
    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">{$t("Network proxy")}</h3>
      <form
        class="mt-4 grid gap-3"
        onsubmit={(e) => { e.preventDefault(); void save(); }}
      >
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Proxy mode")}</span>
          <select
            class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
            bind:value={proxy.mode}
            onchange={handleChange}
          >
            <option value="none">{$t("No proxy")}</option>
            <option value="system">{$t("System proxy")}</option>
            <option value="manual">{$t("Manual proxy URL")}</option>
          </select>
        </label>

        {#if proxy.mode === "manual"}
          <label class="grid gap-1">
            <span class="text-xs text-[var(--color-text-muted)]">{$t("Proxy URL")}</span>
            <input
              type="text"
              class="tx-input tx-mono"
              placeholder="http://127.0.0.1:7890"
              bind:value={proxy.url}
              oninput={handleChange}
            />
            <span class="text-xs text-[var(--color-text-muted)]">
              {$t("Supports HTTP, HTTPS, and SOCKS proxies, such as http://127.0.0.1:7890")}
            </span>
          </label>
        {/if}

        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">{$t("Cloudflare tunnel protocol")}</span>
          <select
            class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
            bind:value={proxy.tunnelProtocol}
            onchange={handleChange}
          >
            <option value="http2">{$t("HTTP/2 over TCP (recommended, works with VPN / TUN mode)")}</option>
            <option value="auto">{$t("Automatic (QUIC first, then HTTP/2)")}</option>
            <option value="quic">{$t("QUIC over UDP only")}</option>
          </select>
          <span class="text-xs text-[var(--color-text-muted)]">
            {$t("Clash, FlClash and similar tools in TUN (virtual adapter) mode often break QUIC. HTTP/2 keeps the tunnel working with them turned on.")}
          </span>
        </label>

        <div class="flex justify-end pt-1">
          <button
            type="submit"
            class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-[var(--primary-contrast)] disabled:opacity-50"
            disabled={!changed || saving}
          >
            {saving ? $t("Saving…") : $t("Save settings")}
          </button>
        </div>
      </form>
    </div>
  </div>
</section>
