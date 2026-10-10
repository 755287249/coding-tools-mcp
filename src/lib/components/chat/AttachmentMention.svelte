<script lang="ts">
  import { onDestroy } from 'svelte';
  import { localChat, type ChatFile } from '$lib/api/chat';
  import { readChatFile, MAX_BROWSER_PREVIEW_BYTES } from '$lib/chat/attachment-transfer';
  import ImagePreview from './ImagePreview.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  let {file,workspaceId,folderId,chatId}:{file:ChatFile;workspaceId:string;folderId:string;chatId:string}=$props();
  let src=$state(''),busy=$state(false),failed=$state(false),openRequest=$state(0);
  let alive=true, generation=0, scope='';
  $effect(()=>{const key=JSON.stringify([workspaceId,folderId,chatId,file.id,file.sha256]);if(key!==scope){scope=key;generation++;if(src)URL.revokeObjectURL(src);src='';failed=false;busy=false;}});
  onDestroy(()=>{alive=false;if(src)URL.revokeObjectURL(src)});
  async function open(){
    if(busy)return;busy=true;const gen=generation;
    try{if(!src){const bytes=await readChatFile(args=>localChat(workspaceId,folderId,args),chatId,file,[workspaceId,folderId]);if(!alive||gen!==generation)return;src=URL.createObjectURL(new Blob([bytes as BlobPart],{type:file.mime}));}openRequest++;}
    catch{if(gen===generation)failed=true}finally{if(gen===generation)busy=false}
  }
</script>
{#if file.mime.startsWith('image/') && file.size<=MAX_BROWSER_PREVIEW_BYTES && !failed}<button type="button" class="attachment-mention" disabled={busy} onclick={open} title={file.name}>@{file.label}</button>{#if src}<ImagePreview {src} name={file.label ?? file.name} {openRequest} thumbnail={false} {workspaceId} {folderId} {chatId} path={file.path}/>{/if}
{:else}<LocalPathLink {workspaceId} {folderId} {chatId} path={file.path} label={'@'+file.label}/>{/if}
<style>.attachment-mention{display:inline;padding:0 3px;border-radius:3px;color:#79b5ff;background:#397ddd22;font:inherit;cursor:pointer}.attachment-mention:hover{text-decoration:underline}.attachment-mention:focus-visible{outline:1px solid #79b5ff}</style>
