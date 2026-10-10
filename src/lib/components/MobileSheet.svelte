<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import { mobileViewport } from '$lib/chat/mobile-viewport';
  import { t } from '$lib/i18n';
  import X from '@lucide/svelte/icons/x';
  let { title, edge='bottom', onClose, children }:{title:string;edge?:'bottom'|'left';onClose:()=>void;children:Snippet}=$props();
  let dialog:HTMLDialogElement;
  onMount(()=>{
    const opener=document.activeElement;
    dialog.showModal();
    return ()=>{dialog.close();queueMicrotask(()=>{if(opener instanceof HTMLElement&&opener.isConnected)opener.focus({preventScroll:true})})};
  });
</script>
<svelte:window onresize={()=>{if(innerWidth>700)onClose()}}/>
<dialog use:mobileViewport class="mobile-sheet" class:drawer={edge==='left'} bind:this={dialog} aria-label={title} onclose={onClose} oncancel={event=>{event.preventDefault();onClose()}} onclick={event=>{if(event.target===dialog){const r=dialog.getBoundingClientRect();if(event.clientX<r.left||event.clientX>r.right||event.clientY<r.top||event.clientY>r.bottom)onClose()}}}>
  <header><strong>{title}</strong><button type="button" aria-label={$t('Close')} onclick={onClose}><X size={20}/></button></header>
  {@render children()}
</dialog>
<style>
.mobile-sheet{position:fixed;inset:auto 0 max(0px,calc(100dvh - var(--mobile-viewport-height,100dvh) - var(--mobile-viewport-top,0px)));margin:0;width:100%;max-width:100%;max-height:calc(var(--mobile-viewport-height,100dvh) * .85);overflow:auto;border:1px solid var(--color-border);border-radius:26px 26px 0 0;padding:0 0 max(10px,env(safe-area-inset-bottom));background:var(--surface-2);color:var(--color-text);box-shadow:0 -10px 45px #0003}.mobile-sheet::backdrop{background:#0006}.mobile-sheet header{display:flex;align-items:center;justify-content:space-between;padding:15px 22px 8px;flex:none}.mobile-sheet header strong{font-size:15px;font-weight:600}.mobile-sheet header button{display:grid;place-items:center;width:36px;height:36px;border-radius:50%;background:var(--surface-hover);cursor:pointer}.mobile-sheet header button:focus-visible{outline:2px solid var(--primary)}.mobile-sheet.drawer{inset:var(--mobile-viewport-top,0px) auto auto 0;width:min(330px,86vw);height:var(--mobile-viewport-height,100dvh);max-height:var(--mobile-viewport-height,100dvh);border:0;border-right:1px solid var(--color-border);border-radius:0;background:var(--surface-2);padding-top:env(safe-area-inset-top)}.drawer[open]{display:flex;flex-direction:column}.drawer header{padding:20px 22px 14px}.drawer header strong{font-size:20px;letter-spacing:-.5px}
</style>
