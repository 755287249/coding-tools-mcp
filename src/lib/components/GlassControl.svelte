<script lang="ts">
  import Droplet from "@lucide/svelte/icons/droplet";
  import { t } from "$lib/i18n";
  import { GLASS_MAX, GLASS_MIN, glassOpacity } from "$lib/stores/glass";
</script>

<label class="gc-pill" title={$t("Window opacity")}>
  <Droplet size={13} aria-hidden="true" />
  <span class="sr-only">{$t("Window opacity")}</span>
  <input type="range" min={GLASS_MIN} max={GLASS_MAX} step="1" bind:value={$glassOpacity} />
  <span class="gc-value">{$glassOpacity}%</span>
</label>
