<script lang="ts">
  import { untrack, onDestroy } from 'svelte';
  import Download from '@lucide/svelte/icons/download';
  import ImageIcon from '@lucide/svelte/icons/image';
  import { t } from '$lib/i18n';
  import CodePreview from './CodePreview.svelte';
  import ImagePreview from './ImagePreview.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  import { isTextFile, MAX_PREVIEW_BYTES } from '$lib/chat/preview';
  import { readChatFile, MAX_BROWSER_PREVIEW_BYTES } from '$lib/chat/attachment-transfer';
  import { localChat, type ChatFile } from '$lib/api/chat';
  let { workspaceId, folderId, chatId, file }: { workspaceId: string; folderId: string; chatId: string; file: ChatFile } = $props();
  const isImage=$derived(file.mime.startsWith('image/'));
  let openRequest=$state(0);
  let alive=true;
  let preview = $state('');
  let code = $state<string | null>(null);
  let error = $state('');
  let busy = $state(false);
  let localOnly = $state(false);
  let pathCopied = $state(false);
  async function copyPath() { try { await navigator.clipboard.writeText(file.path); pathCopied = true; } catch { error = $t('chat.42'); } }
  onDestroy(()=>{alive=false;if(preview)URL.revokeObjectURL(preview)});
  let autoPreviewKey = '';
  $effect(() => {
    const key = `${workspaceId}:${folderId}:${chatId}:${file.id}`;
    if (isImage && autoPreviewKey !== key) {
      autoPreviewKey = key;
      untrack(() => { void open(false); });
    }
  });
  async function open(download: boolean, show=false) {
    if (busy) return;
    busy = true; error = '';
    try {
      if (file.size > MAX_BROWSER_PREVIEW_BYTES) { localOnly = true; return; }
      const bytes = await readChatFile(args=>localChat(workspaceId,folderId,args),chatId,file);
      if(!alive)return;
      if (!download && isImage) { if (preview) URL.revokeObjectURL(preview); preview = URL.createObjectURL(new Blob([bytes as BlobPart],{type:file.mime})); if(show)openRequest++; }
      else if (!download && isTextFile(file.name)) {
        if (bytes.length > MAX_PREVIEW_BYTES) throw new Error($t('chat.65'));
        code = new TextDecoder('utf-8', {fatal:true}).decode(bytes);
      } else {
        const url = URL.createObjectURL(new Blob([bytes as BlobPart], {type:'application/octet-stream'}));
        const a = document.createElement('a'); a.href = url; a.download = file.name; a.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
      }
    } catch(e) { error = String(e); } finally { busy = false; }
  }
  function viewImage(){if(preview)openRequest++;else void open(false,true)}
</script>
{#if isImage}
<div class="image-attachment">
  <button type="button" class="image-thumb" title={`${file.label??file.name} · ${$t('chat.66')}`} aria-label={`${file.label??file.name} · ${$t('chat.66')}`} aria-busy={busy} disabled={busy||localOnly} onclick={viewImage}>
    {#if preview}<img src={preview} alt={file.label??file.name}/>{:else}<ImageIcon size={25}/><span>{file.label??file.name}</span><small>{busy?$t('Loading…'):error?$t('Try again'):$t('chat.66')}</small>{/if}
  </button>
  {#if preview}<span class="image-label">{file.label??file.name}</span><button type="button" class="image-download" aria-label={`${$t('chat.52')}: ${file.label??file.name}`} title={$t('chat.52')} disabled={busy} onclick={()=>open(true)}><Download size={13}/></button><ImagePreview src={preview} name={file.label??file.name} {openRequest} thumbnail={false} {workspaceId} {folderId} {chatId} path={file.path}/>{/if}
  {#if localOnly}<div class="image-local"><LocalPathLink {workspaceId} {folderId} {chatId} path={file.path} label={$t('chat.showInFolder')}/></div>{/if}
  {#if error}<span class="image-error" role="alert" title={error}>{$t('Try again')}</span>{/if}
</div>
{:else}
<div class="attachment">
  <button class="name" onclick={() => open(false)} disabled={busy} title={file.path}>📎 {file.label ?? file.name} <small>{Math.ceil(file.size / 1024)} {$t('chat.59')}</small></button>
  <button onclick={() => open(true)} disabled={busy}>{$t('chat.52')}</button>
  <div class="local-path"><code><LocalPathLink {workspaceId} {folderId} {chatId} path={file.path}/></code><button onclick={copyPath}>{$t(pathCopied?'chat.25':'chat.120')}</button></div>
  {#if localOnly}<p class="local-note">{$t('chat.121')}</p>{/if}
  {#if preview}<ImagePreview src={preview} name={file.label ?? file.name} {workspaceId} {folderId} {chatId} path={file.path}/>{/if}{#if code !== null}<CodePreview {code} language={file.name}/>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</div>
{/if}
<style>
.attachment{margin:8px 0;padding:9px 12px;border:1px solid var(--color-border);border-radius:9px;display:flex;gap:12px;flex-wrap:wrap;align-items:center;font-size:11px}.name{flex:1;text-align:left;overflow-wrap:anywhere}small{color:var(--color-text-muted);white-space:nowrap}button{cursor:pointer}button:disabled{opacity:.5}p{color:var(--danger);width:100%;overflow-wrap:anywhere}
.local-path{display:flex;gap:8px;width:100%;align-items:center;color:var(--color-text-muted)}.local-path code{flex:1;overflow-wrap:anywhere;font-size:10px}.local-note{color:var(--color-text-muted)}

.image-attachment{position:relative;width:112px;height:112px;flex:none;color:var(--color-text)}.image-thumb{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:6px;width:100%;height:100%;padding:0;border:1px solid var(--color-border);border-radius:18px;overflow:hidden;background:var(--surface-2);cursor:zoom-in}.image-thumb img{display:block;width:100%;height:100%;object-fit:cover}.image-thumb span{font-size:11px;max-width:90%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.image-thumb small{font-size:10px}.image-label{position:absolute;bottom:5px;left:5px;max-width:calc(100% - 10px);padding:2px 5px;border-radius:5px;background:#0009;color:white;font-size:9px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;pointer-events:none}.image-download{position:absolute;right:5px;top:5px;display:grid;place-items:center;width:25px;height:25px;border-radius:50%;background:#0009;color:white;opacity:0}.image-attachment:hover .image-download,.image-attachment:focus-within .image-download{opacity:1}.image-thumb:focus-visible,.image-download:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}.image-local{position:absolute;bottom:8px;left:5px;right:5px;font-size:10px;text-align:center}.image-error{position:absolute;bottom:5px;left:5px;right:5px;text-align:center;font-size:9px;color:var(--danger);pointer-events:none}.image-thumb:disabled{cursor:default}
@media(max-width:700px){.image-attachment{width:96px;height:96px}.image-thumb{border-radius:16px}.image-download{opacity:1}}
@media(hover:none){.image-download{opacity:1}}
</style>
