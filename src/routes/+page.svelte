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
  import WorkspacePicker from '$lib/components/chat/WorkspacePicker.svelte';
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
        <WorkspacePicker workspaces={$workspaces} bind:value={projectId} onCreate={canCreate?()=>connectionActions.newConnection?.():undefined}/>
      {/snippet}
    </ChatPanel>
  {/key}
</div>
<style>.chat-home{position:relative;display:flex;flex:1;flex-direction:column;min-height:0;background:var(--card-bg)}.chat-home :global(.chat-shell){flex:1;min-height:0}</style>
