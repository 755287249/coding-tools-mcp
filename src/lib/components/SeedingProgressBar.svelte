<script lang="ts">
  import { t } from '$lib/i18n';
  import { pluginProgress, stopPluginJob } from '$lib/plugins/runtime';
  import Minus from '@lucide/svelte/icons/minus';
  import X from '@lucide/svelte/icons/x';
  import Sprout from '@lucide/svelte/icons/sprout';
  const job = $derived($pluginProgress);
  const percent = $derived(job && job.total ? Math.round(job.done / job.total * 100) : 0);
  const minimize = (value: boolean) => pluginProgress.update(current => current && { ...current, minimized: value });
</script>
{#if job}
  {#if job.minimized}
    <button type="button" class="seed-pill" class:error={job.status === 'error'} title={job.message} onclick={() => minimize(false)} aria-label={$t('rabbit.progress', { done: job.done, total: job.total })}>
      <Sprout size={13}/><span>{job.done}/{job.total}</span><i style:width={`${percent}%`}></i>
    </button>
  {:else}
    <div class="seed-bar" class:error={job.status === 'error'} role="status" aria-live="polite">
      <Sprout size={15}/>
      <div class="text"><strong>{$t('rabbit.progress', { done: job.done, total: job.total })}</strong><span>{job.status === 'done' ? $t('rabbit.done') : job.status === 'stopped' ? $t('rabbit.stopped') : job.message}{job.detail ? ` · ${job.detail}` : ''}</span></div>
      <div class="track"><i style:width={`${percent}%`}></i></div>
      {#if job.status === 'running'}<button type="button" class="stop" onclick={stopPluginJob}>{$t('rabbit.stop')}</button>{/if}
      <button type="button" class="icon" title={$t('rabbit.minimize')} aria-label={$t('rabbit.minimize')} onclick={() => minimize(true)}><Minus size={15}/></button>
      {#if job.status !== 'running'}<button type="button" class="icon" title={$t('Close')} aria-label={$t('Close')} onclick={() => pluginProgress.set(null)}><X size={15}/></button>{/if}
    </div>
  {/if}
{/if}
<style>
  .seed-bar{position:fixed;top:44px;left:50%;transform:translateX(-50%);z-index:900;display:flex;align-items:center;gap:10px;width:min(560px,calc(100vw - 24px));padding:9px 10px 9px 14px;border:1px solid var(--color-border);border-radius:14px;background:var(--card-bg);color:var(--color-text);box-shadow:0 10px 30px rgb(0 0 0 / .14);font-size:12.5px}
  .seed-bar > :global(svg){color:#16a34a;flex:none}
  .text{display:flex;flex-direction:column;min-width:0;flex:none;max-width:45%}.text span{font-size:11.5px;color:var(--color-text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .track{flex:1;height:6px;border-radius:99px;background:var(--surface-2,var(--color-surface));overflow:hidden}.track i,.seed-pill i{display:block;height:100%;background:#16a34a;transition:width .3s}
  .error .track i,.seed-pill.error i{background:var(--danger,#dc2626)}.seed-bar.error{border-color:color-mix(in srgb,var(--danger,#dc2626) 50%,var(--color-border))}
  .stop{padding:5px 10px;border:1px solid var(--color-border);border-radius:8px;background:transparent;color:var(--color-text);font-size:12px;cursor:pointer}
  .icon{display:grid;place-items:center;width:26px;height:26px;border:0;border-radius:7px;background:transparent;color:var(--color-text-muted);cursor:pointer}.icon:hover{background:var(--surface-hover)}
  .seed-pill{position:fixed;top:44px;right:16px;z-index:900;display:inline-flex;align-items:center;gap:6px;height:26px;padding:0 10px;border:1px solid var(--color-border);border-radius:99px;background:var(--card-bg);color:var(--color-text);font-size:11.5px;overflow:hidden;cursor:pointer;box-shadow:0 4px 14px rgb(0 0 0 / .1)}
  .seed-pill > :global(svg){color:#16a34a}.seed-pill i{position:absolute;left:0;bottom:0;height:2px}
</style>
