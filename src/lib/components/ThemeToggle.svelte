<script lang="ts">
  import Moon from "@lucide/svelte/icons/moon";
  import Sun from "@lucide/svelte/icons/sun";
  import { onMount } from "svelte";
  import { t } from "$lib/i18n";

  // The modern dark theme is the default; light is opt-in and remembered.
  let dark = $state(true);

  onMount(() => {
    let stored: string | null = null;
    try {
      stored = localStorage.getItem("theme");
    } catch {
      stored = null;
    }
    dark = stored !== "light";
    apply(false);
  });

  function apply(persist = true) {
    const theme = dark ? "dark" : "light";
    document.documentElement.setAttribute("data-theme", theme);
    document.documentElement.classList.toggle("dark", dark);
    if (!persist) return;
    try {
      localStorage.setItem("theme", theme);
    } catch {
      // Ignore storage failures; the theme still applies for this session.
    }
  }

  function toggle() {
    dark = !dark;
    apply();
  }
</script>

<button
  type="button"
  class="tx-icon-btn tx-icon-btn--bordered"
  onclick={toggle}
  aria-label={$t("Switch theme")}
  title={$t(dark ? "Light theme" : "Dark theme")}
>
  {#if dark}
    <Sun size={15} />
  {:else}
    <Moon size={15} />
  {/if}
</button>
