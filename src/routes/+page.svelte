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
  {#key selected?.id ?? ''}
    <ChatPanel externalNavigation startNew workspaceId={selected?.id??''} folders={selected?workspaceFolders(selected):[]} activeFolderId={selected?.active_folder_id} {endpoint} auth={selected?.auth??{type:'none',oauth_client_id:''}} onNavigate={(folder,chat)=>{if(selected)void goto(appUrl(chatLocation(selected.id,folder,chat)),{noScroll:true})}}>
      {#snippet workspacePicker()}
        <label class="workspace-choice"><Folder size={14}/><select required aria-label={$t('chat.selectWorkspace')} bind:value={projectId}><option value="" disabled>{$t('chat.selectWorkspace')}</option>{#each $workspaces as workspace (workspace.id)}<option value={workspace.id}>{workspace.name}</option>{/each}</select></label>
        {#if canCreate && !selected}<button class="create-workspace" type="button" onclick={()=>connectionActions.newConnection?.()}><Plus size={14}/>{$t('Choose a project folder')}</button>{/if}
      {/snippet}
    </ChatPanel>
  {/key}
</div>
<style>
.chat-home{position:relative;display:flex;flex:1;flex-direction:column;min-height:0;background:var(--card-bg)}.chat-home :global(.chat-shell){flex:1;min-height:0}.workspace-choice,.create-workspace{display:flex;align-items:center;gap:6px;min-width:0;color:var(--color-text-muted);font-size:11px}.workspace-choice select{min-width:0;max-width:200px;background:var(--card-bg);color:var(--color-text);border:1px solid var(--color-border);border-radius:7px;padding:5px 7px;font-size:11px}.create-workspace{cursor:pointer}.workspace-choice select:focus-visible,.create-workspace:focus-visible{outline:2px solid var(--primary);outline-offset:2px}
</style>
