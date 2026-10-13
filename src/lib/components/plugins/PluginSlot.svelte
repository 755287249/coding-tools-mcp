<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { pluginSlot, pluginStatus } from '$lib/plugins/runtime';
  import { pluginPackages, installedAppPlugins, setAppPlugin } from '$lib/plugins/registry';
  let { id, workspaceId, folderId }: { id: string; workspaceId: string; folderId: string } = $props();
  let element: HTMLDivElement;
  const known=$derived($pluginPackages.some(item=>item.manifest.id===id));
  const added=$derived($installedAppPlugins.includes(id));
  const status=$derived($pluginStatus[id]);
  function update(){ if(element) pluginSlot.set({ id, workspaceId, folderId, element, rect: element.getBoundingClientRect() }); }
  onMount(()=>{ const observer=new ResizeObserver(update);observer.observe(element);addEventListener('resize',update);addEventListener('scroll',update,true);update();return()=>{observer.disconnect();removeEventListener('resize',update);removeEventListener('scroll',update,true);pluginSlot.update(slot=>slot?.element===element?null:slot)}; });
  $effect(()=>{id;workspaceId;folderId;queueMicrotask(update)});
</script>
<!-- The plugin iframe lives in PluginDock and is positioned over this box; the text below is only visible while it loads or fails. -->
<div class="plugin-slot" bind:this={element} data-plugin={id}>
  <div class="plugin-slot-state" role="status">
    {#if !known}<p>{$t('plugins.missing',{id})}</p>
    {:else if !added}<p>{$t('plugins.notAdded')}</p><button type="button" onclick={()=>setAppPlugin(id,true)}>{$t('plugins.addAndOpen')}</button>
    {:else if status?.state==='error'}<p class="error">{$t('plugins.loadFailed',{error:status.error??''})}</p>
    {:else}<p>{$t('plugins.loading')}</p>{/if}
  </div>
</div>
<style>.plugin-slot{position:relative;width:100%;height:min(620px,calc(100vh - 230px));min-height:420px;border-radius:12px;overflow:hidden;background:var(--card-bg)}.plugin-slot-state{position:absolute;inset:0;display:flex;flex-direction:column;gap:10px;align-items:center;justify-content:center;padding:24px;text-align:center;font-size:13px;color:var(--color-text-secondary)}.plugin-slot-state .error{color:var(--danger);overflow-wrap:anywhere}.plugin-slot-state button{padding:7px 14px;border-radius:18px;background:var(--color-text);color:var(--primary-contrast);cursor:pointer;font-size:12px}</style>
