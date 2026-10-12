<script lang="ts">
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  /** Per-chat authorization mode: full / normal / strict (完全 / 一般 / 严格授权). */
  type Mode='full'|'normal'|'strict';
  let {mode='normal',disabled=false,onChange}:{mode?:Mode;disabled?:boolean;onChange:(mode:Mode)=>void}=$props();
  const MODES:Mode[]=['full','normal','strict'];
</script>
<label class="access-mode" class:full={mode==='full'} class:strict={mode==='strict'} title={$t(`chat.access.${mode}Hint` as MessageKey)}>
  <ShieldCheck size={14}/>
  <select aria-label={$t('chat.access.label')} value={mode} {disabled} onchange={e=>onChange(e.currentTarget.value as Mode)}>
    {#each MODES as item}<option value={item}>{$t(`chat.access.${item}` as MessageKey)}</option>{/each}
  </select>
</label>
<style>
.access-mode{display:inline-flex;align-items:center;gap:4px;height:29px;padding:0 4px 0 8px;border:1px solid var(--color-border);border-radius:999px;background:var(--surface-2);color:var(--color-text-muted);font-size:12px}
.access-mode.full{color:#d97706;border-color:color-mix(in srgb,#d97706 45%,var(--color-border))}
.access-mode.strict{color:#16a34a;border-color:color-mix(in srgb,#16a34a 45%,var(--color-border))}
select{border:0;background:transparent;color:inherit;font:inherit;padding:0 2px;cursor:pointer;outline:none}
select:disabled{cursor:default;opacity:.6}
.access-mode:focus-within{outline:2px solid var(--primary);outline-offset:1px}
</style>
