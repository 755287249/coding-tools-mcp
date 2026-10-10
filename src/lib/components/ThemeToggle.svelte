<script lang="ts">
  import Moon from "@lucide/svelte/icons/moon";
  import Sun from "@lucide/svelte/icons/sun";
  import Monitor from "@lucide/svelte/icons/monitor";
  import { onMount } from "svelte";
  import { t } from "$lib/i18n";

  let preference = $state('system');
  onMount(() => {
    const sync = () => { preference = document.documentElement.dataset.themePreference ?? 'system'; };
    sync();
    window.addEventListener('ctmcp-theme-change', sync);
    return () => window.removeEventListener('ctmcp-theme-change', sync);
  });
</script>

<label class="theme-select" title={$t('Switch theme')}>
  {#if preference === 'system'}<Monitor size={15}/>{:else if preference === 'dark'}<Moon size={15}/>{:else}<Sun size={15}/>{/if}
  <select aria-label={$t('Switch theme')} value={preference} onchange={event => window.dispatchEvent(new CustomEvent('ctmcp-theme-preference', {detail: event.currentTarget.value}))}>
    <option value="system">{$t('Follow system')}</option>
    <option value="light">{$t('Light theme')}</option>
    <option value="dark">{$t('Dark theme')}</option>
  </select>
</label>

<style>
  .theme-select{display:flex;align-items:center;gap:6px;min-width:0;max-width:100%;padding:5px 7px;border:1px solid var(--color-border);border-radius:7px;color:var(--color-text);background:var(--surface-1)}
  select{min-width:0;max-width:140px;background:transparent;color:inherit;border:0;font-size:12px;cursor:pointer}
  select:focus-visible{outline:2px solid var(--primary);outline-offset:2px}
  :global(.sidebar-collapsed) .theme-select{position:relative;min-width:30px;min-height:30px;justify-content:center}
  :global(.sidebar-collapsed) select{position:absolute;inset:0;width:100%;height:100%;max-width:none;opacity:0}
  :global(.sidebar-collapsed) .theme-select:focus-within{outline:2px solid var(--primary);outline-offset:2px}
  @media(max-width:700px){.theme-select{min-height:40px}select{font-size:14px}}
</style>
