<script lang="ts">
  import { localChat, type ChatFile } from '$lib/api/chat';
  import { goto } from '$app/navigation';
  import { appUrl } from '$lib/app-path';
  import { chatLocation } from '$lib/chat/location';
  import { t } from '$lib/i18n';
  import AssetThumbnail from './AssetThumbnail.svelte';
  import ChatAttachment from './ChatAttachment.svelte';
  import ArtifactLink from './ArtifactLink.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  let {workspaceId, folderId, focusSearchRequest=0}: {workspaceId:string;folderId:string;focusSearchRequest?:number}=$props();
  let searchInput=$state<HTMLInputElement>();
  $effect(()=>{if(focusSearchRequest>0)searchInput?.focus();});
  type Asset={file:ChatFile;chatId:string;title:string;preview:boolean};
  let assets=$state<Asset[]>([]), error=$state(''), loading=$state(false), query=$state(''), category=$state('all'), expanded=$state('');
  const categories=['all','image','document','code','other'] as const;
  function classify(file:ChatFile) { if(file.mime.startsWith('image/')||/\.(png|jpe?g|gif|webp|svg|avif|bmp)$/i.test(file.path))return 'image'; if(/\.(md|txt|pdf|docx?|csv|xlsx?)$/i.test(file.path))return 'document'; if(/\.(svelte|[cm]?[jt]sx?|rs|py|html|css|json|ya?ml|sh)$/i.test(file.path))return 'code';return 'other'; }
  const filtered=$derived(assets.filter(a=>(category==='all'||classify(a.file)===category)&&`${a.file.name} ${a.file.path} ${a.title}`.toLowerCase().includes(query.toLowerCase())));
  $effect(()=>{
    const ws=workspaceId, folder=folderId;let cancelled=false; assets=[];error='';loading=true;expanded='';
    void (async()=>{try{
      const result=await localChat(ws,folder,{action:'list'}); const found=new Map<string,Asset>();
      for(const summary of result.sessions??[]) {
        if(cancelled)return;
        const {session}=await localChat(ws,folder,{action:'read',chat_id:summary.id});
        for(const message of session?.messages??[]) {
          for(const file of message.attachments??[])found.set(file.path,{file,chatId:summary.id,title:summary.title,preview:true});
          for(const match of message.text.matchAll(/mcp-assistant\/artifacts\/[^\s<>"`\])]+/g)) {
            const path=match[0].replace(/[.,;，。；]+$/,'');
            if(!found.has(path))found.set(path,{file:{id:path,name:path.split('/').at(-1)??path,path,mime:'',size:0,sha256:''},chatId:summary.id,title:summary.title,preview:false});
          }
        }
        if(!cancelled)assets=[...found.values()];
      }
    }catch(e){if(!cancelled)error=String(e)}finally{if(!cancelled)loading=false}})();
    return()=>{cancelled=true};
  });
  async function copy(path:string){try{await navigator.clipboard.writeText(path)}catch(e){error=String(e)}}
</script>
<div class="asset-library">
  <input class="tx-input" bind:this={searchInput} bind:value={query} aria-label={$t('shell.findAssets')} placeholder={$t('shell.findAssets')}/>
  <div class="filters">{#each categories as key}<button class:active={category===key} onclick={()=>category=key}>{$t(`shell.${key}`)}</button>{/each}</div>
  {#if error}<p role="alert">{error}</p>{/if}{#if loading}<p role="status">{$t('Working…')}</p>{/if}
  <div class="assets">{#each filtered as asset (workspaceId+folderId+asset.file.path)}<article><AssetThumbnail {workspaceId} {folderId} chatId={asset.chatId} file={asset.file} registered={asset.preview} onOpen={()=>{if(asset.preview)expanded=expanded===asset.file.path?'':asset.file.path}}/><div class="asset-info"><strong title={asset.file.name}>{asset.file.name}</strong><code>{#if !asset.preview&&classify(asset.file)==='image'}<ArtifactLink {workspaceId} {folderId} chatId={asset.chatId} path={asset.file.path} label={asset.file.path}/>{:else}<LocalPathLink {workspaceId} {folderId} chatId={asset.chatId} path={asset.file.path}/>{/if}</code><small>{asset.title}</small></div><div class="actions"><button onclick={()=>copy(asset.file.path)}>{$t('chat.120')}</button><button onclick={()=>goto(appUrl(chatLocation(workspaceId,folderId,asset.chatId)))}>{$t('shell.openChat')}</button>{#if asset.preview}<button onclick={()=>expanded=expanded===asset.file.path?'':asset.file.path}>{$t('Preview')}</button>{/if}</div>{#if expanded===asset.file.path}<ChatAttachment {workspaceId} {folderId} chatId={asset.chatId} file={asset.file}/>{/if}</article>{:else}{#if !loading}<p>{$t('shell.noAssets')}</p>{/if}{/each}</div>
</div>
<style>
.asset-library{max-width:1440px;margin:auto}.filters,.actions{display:flex;gap:8px;flex-wrap:wrap;margin:15px 0}.filters button,.actions button{border:1px solid var(--color-border);border-radius:8px;padding:7px 12px;font-size:12px;cursor:pointer}.filters .active{background:#2563eb;color:#fff}.assets{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(240px,100%),1fr));gap:18px;align-items:start}article{min-width:0;overflow:hidden;border:1px solid var(--color-border);border-radius:14px;background:var(--card-bg)}.asset-info{display:flex;flex-direction:column;gap:8px;padding:14px 14px 0;min-width:0}.asset-info strong{font-size:14px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}code{display:block;font-size:10px;overflow-wrap:anywhere;color:var(--color-text-muted);max-height:32px;overflow:hidden}small{font-size:11px;color:var(--color-text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.actions{padding:0 14px;margin:14px 0;gap:6px}.actions button{font-size:11px;padding:6px 8px}p{font-size:13px;padding:12px 0}.asset-library>input{max-width:600px}button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}
</style>
