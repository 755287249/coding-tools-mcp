<script lang="ts">
 import {onMount} from 'svelte';
 import AppUpdates from './AppUpdates.svelte';
 import X from '@lucide/svelte/icons/x';
 import {t} from '$lib/i18n';
 let {onClose}:{onClose:()=>void}=$props();
 let dialog:HTMLDialogElement;
 onMount(()=>{const opener=document.activeElement;dialog.showModal();return()=>{dialog.close();if(opener instanceof HTMLElement&&opener.isConnected)opener.focus({preventScroll:true})}});
</script>
<dialog bind:this={dialog} aria-label={$t('shell.about')} oncancel={e=>{e.preventDefault();onClose()}}><header><strong>{$t('shell.about')}</strong><button aria-label={$t('Close')} onclick={onClose}><X size={20}/></button></header><AppUpdates/></dialog>
<style>dialog{position:fixed;margin:auto;width:min(680px,calc(100vw - 28px));max-height:88dvh;overflow:auto;border:1px solid var(--color-border);border-radius:18px;background:var(--surface-2);color:var(--color-text);padding:20px}dialog::backdrop{background:#0007}header{display:flex;justify-content:space-between;align-items:center;margin-bottom:14px}header button{padding:6px;cursor:pointer;border-radius:7px}button:focus-visible{outline:2px solid var(--primary)}dialog :global(.page-scroll){overflow:visible}dialog :global(.page-header){padding:0 0 15px}dialog :global(.page-body){padding:0}dialog :global(.tx-card){padding:16px}</style>
