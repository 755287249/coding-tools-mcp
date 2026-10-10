<script lang="ts">
  import {onMount} from 'svelte';
  import BrowserSharing from './BrowserSharing.svelte';
  import X from '@lucide/svelte/icons/x';
  import {t} from '$lib/i18n';
  let {onClose}:{onClose:()=>void}=$props();
  let dialog:HTMLDialogElement;
  onMount(()=>{const opener=document.activeElement;dialog.showModal();return()=>{dialog.close();if(opener instanceof HTMLElement&&opener.isConnected)opener.focus()}});
</script>
<dialog bind:this={dialog} aria-label={$t('sharing.title')} oncancel={e=>{e.preventDefault();onClose()}}><button class="close-share" aria-label={$t('Close')} onclick={onClose}><X size={20}/></button><BrowserSharing/></dialog>
<style>dialog{position:fixed;margin:auto;width:min(680px,calc(100vw - 28px));max-height:88dvh;overflow:auto;border:1px solid var(--color-border);border-radius:18px;background:var(--surface-2);color:var(--color-text);padding:10px}dialog::backdrop{background:#0007}.close-share{position:absolute;right:15px;top:15px;z-index:2;cursor:pointer;padding:6px;border-radius:7px;background:var(--surface-2)}button:focus-visible{outline:2px solid var(--primary)}</style>
