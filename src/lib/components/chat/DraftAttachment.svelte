<script lang="ts">
  import { untrack } from 'svelte';
  import type { PreviewImage } from '$lib/chat/image-gallery';
  import { t } from '$lib/i18n';
  import { localChat, type ChatFile } from '$lib/api/chat';
  import { readChatFile, MAX_BROWSER_PREVIEW_BYTES } from '$lib/chat/attachment-transfer';
  import ImagePreview from './ImagePreview.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  let {file,workspaceId,folderId,chatId,images,disabled=false,onRemove}:{images:PreviewImage[];file:ChatFile;workspaceId:string;folderId:string;chatId:string;disabled?:boolean;onRemove:()=>void}=$props();
  let src=$state(''), error=$state(''), openRequest=$state(0);
  $effect(()=>{
    const f=file,ws=workspaceId,folder=folderId,chat=chatId; let alive=true,url='';
    src='';error='';
    if(f.mime.startsWith('image/')&&f.size<=MAX_BROWSER_PREVIEW_BYTES)untrack(()=>{void readChatFile(args=>localChat(ws,folder,args),chat,f,[ws,folder]).then(bytes=>{if(alive){url=URL.createObjectURL(new Blob([bytes as BlobPart],{type:f.mime}));src=url;}}).catch(e=>{if(alive)error=String(e)})});
    return()=>{alive=false;if(url)URL.revokeObjectURL(url)};
  });
</script>
<div class="draft-attachment">
  <button type="button" class="preview" disabled={!src} onclick={()=>openRequest++} title={file.name}>{#if src}<img {src} alt={file.label ?? file.name}/>{:else}<span>{file.mime.startsWith('image/')?'▧':'▤'}</span>{/if}</button>
  <span class="label" title={file.name}>{file.label ?? file.name}</span>
  <button type="button" class="remove" aria-label={`${$t('chat.53')}: ${file.label ?? file.name}`} disabled={disabled} onclick={onRemove}>×</button>
  {#if src}<ImagePreview {images} {src} name={file.label ?? file.name} {openRequest} thumbnail={false} {workspaceId} {folderId} {chatId} path={file.path}/>{/if}
  {#if error || file.size>MAX_BROWSER_PREVIEW_BYTES}<LocalPathLink {workspaceId} {folderId} {chatId} path={file.path} label={$t('chat.showInFolder')}/>{/if}
</div>
<style>
.draft-attachment{position:relative;flex:none;width:96px;min-height:104px;display:flex;flex-direction:column;gap:5px;font-size:11px;text-align:center}.preview{display:grid;place-items:center;height:76px;border:1px solid #ffffff25;border-radius:9px;background:#202020;overflow:hidden;cursor:zoom-in}.preview img{width:100%;height:100%;object-fit:cover}.preview span{font-size:26px;color:#bbb}.preview:disabled{cursor:default}.label{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#ddd}.remove{position:absolute;right:3px;top:3px;display:grid;place-items:center;width:20px;height:20px;border-radius:50%;background:#171717;color:#fff;box-shadow:0 1px 4px #0008;font-size:16px;cursor:pointer}.remove:disabled{opacity:.5}.remove:focus-visible,.preview:focus-visible{outline:2px solid #7bacff}
</style>
