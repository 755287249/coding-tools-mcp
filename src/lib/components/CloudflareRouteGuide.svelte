<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ClipboardList from "@lucide/svelte/icons/clipboard-list";
  import Copy from "@lucide/svelte/icons/copy";
  import Link from "@lucide/svelte/icons/link";
  import Route from "@lucide/svelte/icons/route";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import {
    CLOUDFLARE_TUNNELS_URL,
    guessZone,
    hostnameSplits,
    routeHostname,
    routeServiceUrl,
    splitForZone,
  } from "$lib/connect/cloudflare-route";
  import { t } from "$lib/i18n";

  interface Props {
    /** Domain as typed or saved (`mcp.example.com` or `https://mcp.example.com`). */
    hostname: string;
    /** Local port of the service the tunnel publishes. */
    port: number;
    bindAddress?: string;
    /** Start collapsed (used where the guide is secondary). */
    collapsed?: boolean;
  }

  let { hostname, port, bindAddress, collapsed = false }: Props = $props();

  const ZONE_STORAGE_PREFIX = "ctm.cloudflare-zone:";

  let open = $state(true);
  let zoneOverride = $state<string | null>(null);
  let copied = $state<string | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  $effect.pre(() => {
    open = !collapsed;
  });

  const host = $derived(routeHostname(hostname));
  const splits = $derived(host ? hostnameSplits(host) : []);
  const storedZone = $derived.by(() => {
    if (!host) return null;
    try {
      return localStorage.getItem(ZONE_STORAGE_PREFIX + host);
    } catch {
      return null;
    }
  });
  const zone = $derived.by(() => {
    if (!host) return "";
    const preferred = zoneOverride ?? storedZone;
    return preferred && splits.some((split) => split.zone === preferred) ? preferred : guessZone(host);
  });
  const split = $derived(host ? splitForZone(host, zone) : { subdomain: "", zone: "" });
  const serviceUrl = $derived(routeServiceUrl(port, bindAddress));
  const multiLevel = $derived(split.subdomain.includes("."));

  function chooseZone(value: string) {
    zoneOverride = value;
    if (!host) return;
    try {
      localStorage.setItem(ZONE_STORAGE_PREFIX + host, value);
    } catch {
      // Storage unavailable: the choice still applies for this session.
    }
  }

  async function writeClipboard(value: string) {
    try {
      await navigator.clipboard.writeText(value);
    } catch {
      const area = document.createElement("textarea");
      area.value = value;
      area.setAttribute("readonly", "");
      area.className = "ax-offscreen";
      document.body.appendChild(area);
      area.select();
      document.execCommand("copy");
      area.remove();
    }
  }

  async function copy(key: string, value: string) {
    await writeClipboard(value);
    copied = key;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = null), 1600);
  }

  function summary(): string {
    const empty = $t("(leave empty)");
    return [
      $t("Cloudflare › Networking › Tunnels › your tunnel › Routes › Add route › Published application"),
      `${$t("Subdomain field")}: ${split.subdomain || empty}`,
      `${$t("Domain field")}: ${split.zone}`,
      `${$t("Path field")}: ${empty}`,
      `${$t("Service URL field")}: ${serviceUrl}`,
      `${$t("Full hostname")}: ${host}`,
    ].join("\n");
  }
</script>

<div class="cf-guide" class:is-open={open}>
  <button type="button" class="cf-guide-head" aria-expanded={open} onclick={() => (open = !open)}>
    <Route size={14} />
    <span class="cf-guide-title">{$t("Cloudflare route settings")}</span>
    {#if host && !open}
      <span class="cf-guide-peek tx-mono">{serviceUrl}</span>
    {/if}
    <ChevronDown size={14} class={open ? "cf-guide-chevron is-open" : "cf-guide-chevron"} />
  </button>

  {#if open}
    <div class="cf-guide-body">
      <p class="cf-guide-intro">
        {$t("In the Cloudflare dashboard open Networking › Tunnels › your tunnel › Routes › Add route › Published application, then paste these values:")}
      </p>

      {#if host}
        <div class="cf-fields">
          <div class="cf-field">
            <span class="cf-label">{$t("Subdomain field")}</span>
            <div class="cf-value-wrap">
              {#if split.subdomain}
                <button type="button" class="cf-value tx-mono" title={$t("Copy")} onclick={() => void copy("sub", split.subdomain)}>
                  <span class="truncate">{split.subdomain}</span>
                  {#if copied === "sub"}<Check size={12} />{:else}<Copy size={12} />{/if}
                </button>
              {:else}
                <span class="cf-empty">{$t("(leave empty)")}</span>
              {/if}
            </div>
          </div>

          <div class="cf-field">
            <span class="cf-label">{$t("Domain field")}</span>
            <div class="cf-value-wrap">
              <button type="button" class="cf-value tx-mono" title={$t("Copy")} onclick={() => void copy("zone", split.zone)}>
                <span class="truncate">{split.zone}</span>
                {#if copied === "zone"}<Check size={12} />{:else}<Copy size={12} />{/if}
              </button>
              <span class="cf-hint">{$t("Pick it in the drop-down")}</span>
            </div>
          </div>
          {#if splits.length > 1}
            <div class="cf-zone-pick" role="radiogroup" aria-label={$t("Which part is your Cloudflare domain?")}>
              <span class="cf-zone-q">{$t("Which part is your Cloudflare domain?")}</span>
              {#each splits as option (option.zone)}
                <button
                  type="button"
                  role="radio"
                  aria-checked={option.zone === split.zone}
                  class="cf-zone-chip tx-mono"
                  class:active={option.zone === split.zone}
                  onclick={() => chooseZone(option.zone)}
                >
                  {option.zone}
                </button>
              {/each}
            </div>
          {/if}

          <div class="cf-field">
            <span class="cf-label">{$t("Path field")}</span>
            <div class="cf-value-wrap">
              <span class="cf-empty">{$t("(leave empty)")}</span>
              <span class="cf-hint">{$t("Don't enter /mcp — sign-in and discovery URLs must reach the app too.")}</span>
            </div>
          </div>

          <div class="cf-field is-key">
            <span class="cf-label">{$t("Service URL field")}</span>
            <div class="cf-value-wrap">
              <button type="button" class="cf-value tx-mono" title={$t("Copy")} onclick={() => void copy("service", serviceUrl)}>
                <span class="truncate">{serviceUrl}</span>
                {#if copied === "service"}<Check size={12} />{:else}<Copy size={12} />{/if}
              </button>
              <span class="cf-hint">{$t("The port must match this workspace's local port ({port}).", { port })}</span>
            </div>
          </div>
        </div>

        <p class="cf-check">
          {$t("Cloudflare should show the full hostname:")}
          <span class="tx-mono">{host}</span>
        </p>
        {#if multiLevel}
          <p class="cf-warn"><TriangleAlert size={12} /> {$t("Multi-level subdomains need Cloudflare's Advanced Certificate Manager.")}</p>
        {/if}
        <ul class="cf-notes">
          <li>{$t("Leave the additional application settings at their defaults.")}</li>
          <li>{$t("If Cloudflare says a DNS record already exists, delete the A / AAAA / CNAME record with the same name under DNS first.")}</li>
          <li>{$t("The domain saved here must be the same hostname, otherwise browser sign-in is rejected.")}</li>
        </ul>

        <div class="cf-actions">
          <button type="button" class="tx-btn-ghost cf-action" onclick={() => void copy("all", summary())}>
            {#if copied === "all"}<Check size={13} />{:else}<ClipboardList size={13} />{/if}
            {copied === "all" ? $t("Copied!") : $t("Copy all")}
          </button>
          <button type="button" class="tx-btn-ghost cf-action" onclick={() => void copy("link", CLOUDFLARE_TUNNELS_URL)}>
            {#if copied === "link"}<Check size={13} />{:else}<Link size={13} />{/if}
            {copied === "link" ? $t("Copied!") : $t("Copy dashboard link")}
          </button>
        </div>
      {:else}
        <p class="cf-hint">{$t("Enter the domain above to generate the values to paste.")}</p>
      {/if}
    </div>
  {/if}
</div>
