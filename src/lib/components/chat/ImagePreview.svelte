<script lang="ts">
  import { t } from '$lib/i18n';
  let { src, name, openRequest = 0, thumbnail = true }: {src: string; name: string; openRequest?:number; thumbnail?:boolean} = $props();
  let viewer = $state<HTMLDialogElement>();
  $effect(()=>{if(openRequest>0&&viewer){zoom=100;if(!viewer.open)viewer.showModal()}});
  let zoom = $state(100);
</script>
{#if thumbnail}<button class="thumbnail" onclick={() => { zoom = 100; viewer?.showModal(); }} title={$t('chat.66')}><img {src} alt={name}/></button>{/if}
<dialog bind:this={viewer} aria-label={name}>
  <header><strong>{name}</strong><button onclick={() => viewer?.close()}>{$t('chat.67')}</button></header>
  <div class="controls"><label>{$t('chat.68')} <input type="range" min="25" max="400" step="25" bind:value={zoom}/> {zoom}%</label><button onclick={() => zoom = 100}>{$t('chat.69')}</button></div>
  <div class="canvas"><img {src} alt={name} style:width={`${zoom}%`}/></div>
</dialog>
<style>
.thumbnail{width:100%;text-align:left;cursor:zoom-in}.thumbnail img{max-width:100%;max-height:300px;object-fit:contain;border-radius:7px}dialog{width:min(1000px,90vw);height:85dvh;max-width:95vw;max-height:95dvh;border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg);color:var(--color-text);padding:0}dialog::backdrop{background:#0009}header,.controls{padding:12px 18px;display:flex;align-items:center;justify-content:space-between;gap:12px;border-bottom:1px solid var(--color-border);font-size:12px}strong{overflow-wrap:anywhere}button{cursor:pointer}label{display:flex;align-items:center;gap:10px}.canvas{overflow:auto;height:calc(100% - 100px);background:repeating-conic-gradient(#8882 0% 25%,#8881 0% 50%) 50%/20px 20px;display:block}.canvas img{display:block;height:auto;max-width:none;margin:auto}
</style>
