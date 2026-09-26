<script lang="ts">
  import Droplet from "@lucide/svelte/icons/droplet";
  import DropletOff from "@lucide/svelte/icons/droplet-off";
  import { t } from "$lib/i18n";
  import { GLASS_MAX, GLASS_MIN, glassBlur, glassOpacity } from "$lib/stores/glass";
</script>

<div class="gc-pill">
  <button
    type="button"
    class="gc-toggle"
    class:is-off={!$glassBlur}
    title={$t("Frosted glass")}
    aria-label={$t("Frosted glass")}
    aria-pressed={$glassBlur}
    onclick={() => glassBlur.update((value) => !value)}
  >
    {#if $glassBlur}<Droplet size={13} />{:else}<DropletOff size={13} />{/if}
  </button>
  <label class="gc-range" title={$t("Window opacity")}>
    <span class="sr-only">{$t("Window opacity")}</span>
    <input type="range" min={GLASS_MIN} max={GLASS_MAX} step="1" bind:value={$glassOpacity} />
    <span class="gc-value">{$glassOpacity}%</span>
  </label>
</div>
