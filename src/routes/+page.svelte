<script lang="ts">
  import { goto } from '$app/navigation';
  import { appUrl } from '$lib/app-path';
  import { t } from '$lib/i18n';
  import { workspaces } from '$lib/stores/app';
  import { connectionActions } from '$lib/stores/ui-mode';
  import { getRuntimeStatus } from '$lib/api/workspaces';
  import { workspaceFolders } from '$lib/types';
  import { chatLocation } from '$lib/chat/location';
  import { getBackend } from '$lib/backend';
  import ChatPanel from '$lib/components/chat/ChatPanel.svelte';
  import Folder from '@lucide/svelte/icons/folder';
  import Plus from '@lucide/svelte/icons/plus';
  let projectId=$state('');
  const selected=$derived($workspaces.find(workspace=>workspace.id===projectId));
  const canCreate=getBackend().capabilities.workspaceLifecycle;
  let endpoint=$state('');
  $effect(()=>{const id=selected?.id;endpoint='';let stopped=false;if(id)void getRuntimeStatus(id).then(runtime=>{if(!stopped)endpoint=runtime.publicEndpoint||runtime.localEndpoint}).catch(()=>{});return()=>{stopped=true}});
</script>
<div class="chat-home">
  {#if selected}
    <div class="project-picker"><Folder size={15}/><select aria-label={$t('chat.109')} bind:value={projectId}><option value="">{$t('chat.109')}</option>{#each $workspaces as workspace (workspace.id)}<option value={workspace.id}>{workspace.name}</option>{/each}</select></div>
    {#key selected.id}<ChatPanel externalNavigation startNew workspaceId={selected.id} folders={workspaceFolders(selected)} activeFolderId={selected.active_folder_id} {endpoint} auth={selected.auth} onNavigate={(folder,chat)=>void goto(appUrl(chatLocation(selected!.id,folder,chat)),{noScroll:true})}/>{/key}
  {:else}
    <div class="home-welcome"><div class="home-mark"><Folder size={32}/></div><h1>{$t('chat.102')}</h1><p>{$t('chat.110')}</p></div>
    <div class="home-projects"><label><Folder size={16}/><select aria-label={$t('chat.109')} bind:value={projectId}><option value="">{$t('chat.109')}</option>{#each $workspaces as workspace (workspace.id)}<option value={workspace.id}>{workspace.name}</option>{/each}</select></label>{#if canCreate}<button onclick={()=>connectionActions.newConnection?.()}><Plus size={15}/>{$t('Choose a project folder')}</button>{/if}</div>
  {/if}
</div>
<style>
.chat-home{position:relative;display:flex;flex:1;flex-direction:column;min-height:0;background:var(--card-bg)}.chat-home :global(.chat-shell){flex:1;min-height:0}.home-welcome{flex:1;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:17px;padding:30px}.home-mark{color:var(--color-text-muted)}h1{font-size:clamp(23px,3vw,32px);font-weight:550;letter-spacing:-.5px}p{font-size:12px;color:var(--color-text-muted);text-align:center}.home-projects{width:min(740px,calc(100% - 48px));margin:0 auto 30px;display:flex;flex-wrap:wrap;align-items:center;gap:12px;padding:22px;border:1px solid var(--color-border);border-radius:20px;background:var(--surface-hover)}label,.project-picker{display:flex;align-items:center;gap:8px;color:var(--color-text-muted)}select{max-width:100%;min-width:150px;border:0;background:var(--card-bg);padding:9px;border-radius:7px;color:var(--color-text);font-size:12px}button{display:flex;align-items:center;gap:7px;margin-left:auto;cursor:pointer;font-size:11px;color:var(--color-text-muted)}.project-picker{padding:10px 25px;border-bottom:1px solid var(--color-border);flex:none}button:focus-visible,select:focus-visible{outline:2px solid var(--primary);outline-offset:2px}@media(max-width:700px){.home-projects{padding:15px;width:calc(100% - 24px)}.project-picker{padding:8px 12px}}
</style>
