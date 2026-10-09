<script lang="ts">
  import {localChat} from '$lib/api/chat';
  import {t} from '$lib/i18n';
  let {workspaceId,folderId,chatId,path,label}: {workspaceId:string;folderId:string;chatId:string;path:string;label?:string}=$props();
  let busy=$state(false),error=$state('');
  async function reveal(){if(busy)return;busy=true;error='';try{await localChat(workspaceId,folderId,{action:'reveal_path',chat_id:chatId,source_path:path});}catch(e){error=String(e)}finally{busy=false}}
</script>
<button type="button" class="local-path-link" title={`${$t('chat.showInFolder')}: ${path}`} onclick={reveal} disabled={busy}>{label??path}</button>
{#if error}<span class="path-error" role="alert">{error}</span>{/if}
<style>.local-path-link{display:inline;color:#81b4ff;text-decoration:underline;text-underline-offset:3px;text-align:left;cursor:pointer;overflow-wrap:anywhere}.local-path-link:disabled{opacity:.5}.path-error{display:block;color:var(--danger);font-size:11px;overflow-wrap:anywhere}</style>
