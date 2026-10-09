<script lang="ts">
  import { untrack, onDestroy } from 'svelte';
  import { t } from '$lib/i18n';
  import CodePreview from './CodePreview.svelte';
  import ImagePreview from './ImagePreview.svelte';
  import LocalPathLink from './LocalPathLink.svelte';
  import { isTextFile, MAX_PREVIEW_BYTES } from '$lib/chat/preview';
  import { readChatFile, MAX_BROWSER_PREVIEW_BYTES } from '$lib/chat/attachment-transfer';
  import { localChat, type ChatFile } from '$lib/api/chat';
  let { workspaceId, folderId, chatId, file }: { workspaceId: string; folderId: string; chatId: string; file: ChatFile } = $props();
  let preview = $state('');
  let code = $state<string | null>(null);
  let error = $state('');
  let busy = $state(false);
  let localOnly = $state(false);
  let pathCopied = $state(false);
  async function copyPath() { try { await navigator.clipboard.writeText(file.path); pathCopied = true; } catch { error = $t('chat.42'); } }
  onDestroy(()=>{if(preview)URL.revokeObjectURL(preview)});
  let autoPreviewKey = '';
  $effect(() => {
    const key = `${workspaceId}:${folderId}:${chatId}:${file.id}`;
    if (['image/png','image/jpeg','image/gif','image/webp'].includes(file.mime) && autoPreviewKey !== key) {
      autoPreviewKey = key;
      untrack(() => { void open(false); });
    }
  });
  async function open(download: boolean) {
    if (busy) return;
    busy = true; error = '';
    try {
      if (file.size > MAX_BROWSER_PREVIEW_BYTES) { localOnly = true; return; }
      const bytes = await readChatFile(args=>localChat(workspaceId,folderId,args),chatId,file);
      if (!download && ['image/png','image/jpeg','image/gif','image/webp'].includes(file.mime)) { if (preview) URL.revokeObjectURL(preview); preview = URL.createObjectURL(new Blob([bytes as BlobPart],{type:file.mime})); }
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
</script>
<div class="attachment">
  <button class="name" onclick={() => open(false)} disabled={busy} title={file.path}>📎 {file.label ?? file.name} <small>{Math.ceil(file.size / 1024)} {$t('chat.59')}</small></button>
  <button onclick={() => open(true)} disabled={busy}>{$t('chat.52')}</button>
  <div class="local-path"><code><LocalPathLink {workspaceId} {folderId} {chatId} path={file.path}/></code><button onclick={copyPath}>{$t(pathCopied?'chat.25':'chat.120')}</button></div>
  {#if localOnly}<p class="local-note">{$t('chat.121')}</p>{/if}
  {#if preview}<ImagePreview src={preview} name={file.label ?? file.name} {workspaceId} {folderId} {chatId} path={file.path}/>{/if}{#if code !== null}<CodePreview {code} language={file.name}/>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</div>
<style>
.attachment{margin:8px 0;padding:9px 12px;border:1px solid var(--color-border);border-radius:9px;display:flex;gap:12px;flex-wrap:wrap;align-items:center;font-size:11px}.name{flex:1;text-align:left;overflow-wrap:anywhere}small{color:var(--color-text-muted);white-space:nowrap}button{cursor:pointer}button:disabled{opacity:.5}p{color:var(--danger);width:100%;overflow-wrap:anywhere}
.local-path{display:flex;gap:8px;width:100%;align-items:center;color:var(--color-text-muted)}.local-path code{flex:1;overflow-wrap:anywhere;font-size:10px}.local-note{color:var(--color-text-muted)}
</style>
