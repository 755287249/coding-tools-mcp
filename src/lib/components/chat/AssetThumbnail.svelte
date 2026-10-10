<script lang="ts">
  import { t } from '$lib/i18n';
  import { localChat, type ChatFile } from '$lib/api/chat';
  import { readChatFile } from '$lib/chat/attachment-transfer';
  import { assetPreview } from '$lib/chat/asset-preview';
  import ImagePreview from './ImagePreview.svelte';
  import File from '@lucide/svelte/icons/file';
  import FileText from '@lucide/svelte/icons/file-text';
  import Image from '@lucide/svelte/icons/image';
  let { workspaceId,folderId,chatId,file,registered,onOpen }: {workspaceId:string;folderId:string;chatId:string;file:ChatFile;registered:boolean;onOpen:()=>void}=$props();
  let host=$state<HTMLDivElement>();
  let src=$state(''), excerpt=$state(''), failed=$state(false), busy=$state(false), openRequest=$state(0);
  const kind=$derived(assetPreview(file,registered));
  const extension=$derived(file.name.split('.').at(-1)?.toUpperCase()??'');
  $effect(()=>{
    const target=host, ws=workspaceId, folder=folderId, chat=chatId, attachment=file, known=registered, preview=kind;
    if(!target)return;
    let cancelled=false, started=false, objectUrl='';
    src='';excerpt='';failed=false;busy=false;openRequest=0;
    async function load() {
      if(started||!preview)return;started=true;busy=true;
      try {
        const request=(args:Parameters<typeof localChat>[2])=>{if(cancelled)throw Error('Preview cancelled');return localChat(ws,folder,args)};
        if(known){
          const bytes=await readChatFile(request,chat,attachment);
          if(cancelled)return;
          if(preview.kind==='image'){objectUrl=URL.createObjectURL(new Blob([bytes as BlobPart],{type:preview.mime}));src=objectUrl;}
          else excerpt=new TextDecoder('utf-8',{fatal:true}).decode(bytes).slice(0,1600);
        }else{
          const result=await request({action:'read_artifact',chat_id:chat,source_path:attachment.path});
          if(cancelled)return;
          if(!result.data_base64||!['image/png','image/jpeg','image/gif','image/webp'].includes(result.mime??''))throw Error('Preview unavailable');
          src=`data:${result.mime};base64,${result.data_base64}`;
        }
      }catch{if(!cancelled)failed=true}finally{if(!cancelled)busy=false}
    }
    const observer=new IntersectionObserver(entries=>{if(entries.some(entry=>entry.isIntersecting)){observer.disconnect();void load()}},{rootMargin:'100px'});
    observer.observe(target);
    return()=>{cancelled=true;observer.disconnect();if(objectUrl)URL.revokeObjectURL(objectUrl)};
  });
</script>
<div class="asset-visual" bind:this={host}>
  <button type="button" class:text-preview={!!excerpt} disabled={!src&&!registered} title={$t('Preview')} aria-label={`${$t('Preview')}: ${file.name}`} onclick={()=>{if(src)openRequest++;else onOpen()}}>
    {#if src}<img {src} alt={file.name} loading="lazy"/>{:else if excerpt}<pre>{excerpt}</pre>{:else}<span class="file-placeholder">{#if kind?.kind==='image'}<Image size={38}/>{:else if kind?.kind==='text'}<FileText size={38}/>{:else}<File size={38}/>{/if}<strong>{extension}</strong>{#if busy}<small>{$t('Loading…')}</small>{:else if failed}<small>{$t('Preview')}</small>{/if}</span>{/if}
  </button>
  {#if src}<ImagePreview {src} name={file.name} {workspaceId} {folderId} {chatId} path={file.path} {openRequest} thumbnail={false}/>{/if}
</div>
<style>
.asset-visual{height:180px;background:color-mix(in srgb,var(--color-text) 4%,var(--card-bg));border-bottom:1px solid var(--color-border)}button{display:block;width:100%;height:100%;overflow:hidden;text-align:left;cursor:pointer}button:focus-visible{outline:2px solid var(--primary);outline-offset:-3px}img{width:100%;height:100%;object-fit:contain;padding:8px}.file-placeholder{display:flex;align-items:center;justify-content:center;flex-direction:column;gap:9px;height:100%;color:var(--color-text-muted)}.file-placeholder strong{font-size:11px;letter-spacing:1px}.file-placeholder small{font-size:10px}pre{font:11px/1.6 ui-monospace,monospace;white-space:pre-wrap;overflow-wrap:anywhere;padding:16px;color:var(--color-text-muted);height:100%;overflow:hidden;mask-image:linear-gradient(#000 65%,transparent)}.text-preview{background:var(--card-bg)}
</style>
