<script lang="ts">
  import Moon from "@lucide/svelte/icons/moon";
  import Sun from "@lucide/svelte/icons/sun";
  import Monitor from "@lucide/svelte/icons/monitor";
  import { onMount } from "svelte";
  import { t } from "$lib/i18n";
  import MenuSelect from "./MenuSelect.svelte";

  type Theme = 'system' | 'light' | 'dark';
  let preference = $state<Theme>('system');
  onMount(() => {
    const sync = () => { preference = (document.documentElement.dataset.themePreference as Theme) ?? 'system'; };
    sync();
    window.addEventListener('ctmcp-theme-change', sync);
    return () => window.removeEventListener('ctmcp-theme-change', sync);
  });
  const options = $derived([
    { value: 'system' as Theme, label: $t('Follow system'), icon: Monitor },
    { value: 'light' as Theme, label: $t('Light theme'), icon: Sun },
    { value: 'dark' as Theme, label: $t('Dark theme'), icon: Moon },
  ]);
</script>

<MenuSelect value={preference} {options} label={$t('Switch theme')} onChange={value => window.dispatchEvent(new CustomEvent('ctmcp-theme-preference', { detail: value }))} />
