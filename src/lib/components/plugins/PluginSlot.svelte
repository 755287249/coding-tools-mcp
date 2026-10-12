<script lang="ts">
  import { onMount } from 'svelte';
  import { pluginSlot } from '$lib/plugins/runtime';
  let { id, workspaceId, folderId }: { id: string; workspaceId: string; folderId: string } = $props();
  let element: HTMLDivElement;
  function update(){ if(element) pluginSlot.set({ id, workspaceId, folderId, element, rect: element.getBoundingClientRect() }); }
  onMount(()=>{ const observer=new ResizeObserver(update);observer.observe(element);addEventListener('resize',update);addEventListener('scroll',update,true);update();return()=>{observer.disconnect();removeEventListener('resize',update);removeEventListener('scroll',update,true);pluginSlot.update(slot=>slot?.element===element?null:slot)}; });
  $effect(()=>{id;workspaceId;folderId;queueMicrotask(update)});
</script>
<div class="plugin-slot" bind:this={element} data-plugin={id}></div>
<style>.plugin-slot{position:relative;width:100%;height:min(620px,calc(100vh - 230px));min-height:420px;border-radius:12px;overflow:hidden;background:var(--card-bg)}</style>
