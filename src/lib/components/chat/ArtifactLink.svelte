<script lang="ts">
  import {localChat} from '$lib/api/chat';
  import {t} from '$lib/i18n';
  import ImagePreview from './ImagePreview.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  let {workspaceId,folderId,chatId,path,label}: {workspaceId:string;folderId:string;chatId:string;path:string;label:string}=$props();
  let src=$state(''), error=$state(''), busy=$state(false), request=$state(0);
  async function open(){if(busy)return;busy=true;error='';try{
    const result=await localChat(workspaceId,folderId,{action:'read_artifact',chat_id:chatId,source_path:path});
    if(!result.data_base64||!['image/png','image/jpeg','image/gif','image/webp'].includes(result.mime??''))throw new Error($t('chat.57'));
    src=`data:${result.mime};base64,${result.data_base64}`;request++;
  }catch(e){error=String(e)}finally{busy=false}}
</script>
<button class="artifact-link" title={$t('chat.66')} onclick={open} disabled={busy}>{label}</button>
<LocalPathLink {workspaceId} {folderId} {chatId} {path} label={$t('chat.showInFolder')}/>
{#if error}<span class="artifact-error" role="alert">{error}</span>{/if}
{#if src}<ImagePreview {workspaceId} {folderId} {chatId} {path} {src} name={path.split('/').at(-1)??path} openRequest={request} thumbnail={false}/>{/if}
<style>.artifact-link{display:inline;color:var(--chat-link);text-decoration:underline;text-underline-offset:3px;cursor:zoom-in;text-align:left;overflow-wrap:anywhere}.artifact-link:disabled{opacity:.5}.artifact-error{display:block;color:var(--danger);font-size:11px}</style>
