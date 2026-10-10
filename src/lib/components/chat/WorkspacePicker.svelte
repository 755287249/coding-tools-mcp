<script lang="ts">
 import {tick} from 'svelte';
 import Folder from '@lucide/svelte/icons/folder';
 import Plus from '@lucide/svelte/icons/plus';
 import Check from '@lucide/svelte/icons/check';
 import {t} from '$lib/i18n';
 import {anchoredChoices} from '$lib/chat/anchored-choices';
 import type {WorkspaceProfile} from '$lib/types';
 let {workspaces,value=$bindable(''),onCreate}:{workspaces:WorkspaceProfile[];value?:string;onCreate?:()=>void}=$props();
 let open=$state(false),query=$state(''),anchor=$state<HTMLButtonElement>(),search=$state<HTMLInputElement>();
 const selected=$derived(workspaces.find(w=>w.id===value));
 const matches=$derived(workspaces.filter(w=>w.name.toLocaleLowerCase().includes(query.toLocaleLowerCase())));
 async function toggle(){open=!open;query='';if(open){await tick();search?.focus()}}
 function choose(id:string){value=id;open=false;anchor?.focus()}
 function keys(e:KeyboardEvent){if(!['ArrowDown','ArrowUp','Home','End'].includes(e.key))return;const buttons=[...(e.currentTarget as HTMLElement).querySelectorAll<HTMLButtonElement>('button')];if(!buttons.length)return;e.preventDefault();const at=buttons.indexOf(document.activeElement as HTMLButtonElement);buttons[e.key==='Home'?0:e.key==='End'?buttons.length-1:(at+(e.key==='ArrowDown'?1:-1)+buttons.length)%buttons.length]?.focus()}
</script>
<button class="workspace-choice" type="button" bind:this={anchor} onclick={toggle} aria-label={$t('chat.selectWorkspace')} aria-expanded={open} aria-haspopup="dialog"><Folder size={14}/><span>{selected?.name??$t('chat.selectWorkspace')}</span><span>⌄</span></button>
{#if open}<div class="workspace-menu" popover="auto" role="dialog" tabindex="-1" aria-label={$t('chat.selectWorkspace')} use:anchoredChoices={()=>anchor} ontoggle={e=>{if(e.newState==='closed')open=false}} onkeydown={keys}>
<input bind:this={search} bind:value={query} placeholder={$t('project.search')} aria-label={$t('project.search')}/>
{#if onCreate}<button type="button" onclick={()=>{open=false;onCreate?.()}}><Plus size={15}/>{$t('project.create')}</button>{/if}
<div class="workspace-list">{#each matches as workspace(workspace.id)}<button type="button" onclick={()=>choose(workspace.id)} aria-pressed={value===workspace.id}><Folder size={15}/><span>{workspace.name}</span>{#if value===workspace.id}<Check size={15}/>{/if}</button>{/each}</div>
</div>{/if}
<style>
.workspace-choice{display:flex;align-items:center;gap:6px;min-width:0;max-width:240px;padding:5px 8px;border-radius:8px;color:var(--color-text);background:var(--surface-hover);font-size:11px;cursor:pointer}.workspace-choice span:first-of-type{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.workspace-choice :global(svg){flex:none}.workspace-menu{position:fixed;inset:auto;margin:0;padding:8px;border:1px solid var(--color-border);border-radius:14px;background:var(--surface-2);color:var(--color-text);box-shadow:0 10px 32px #0006;overflow:auto}.workspace-menu input{width:100%;min-width:0;background:transparent;border:0;border-bottom:1px solid var(--color-border);padding:8px;font-size:13px;color:inherit;margin-bottom:5px}.workspace-menu button{display:flex;align-items:center;gap:8px;width:100%;padding:8px;border-radius:8px;text-align:left;font-size:13px;cursor:pointer}.workspace-menu button span{flex:1;overflow-wrap:anywhere}.workspace-menu button:hover,.workspace-menu button[aria-pressed=true]{background:var(--surface-hover)}button:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}.workspace-list{border-top:1px solid var(--color-border);margin-top:4px;padding-top:4px}
</style>
