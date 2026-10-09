<script lang="ts">
  import { t } from '$lib/i18n';
  import CodePreview from './CodePreview.svelte';
  import ImagePreview from './ImagePreview.svelte';
  import { isTextFile, MAX_PREVIEW_BYTES } from '$lib/chat/preview';
  import { localChat, type ChatFile } from '$lib/api/chat';
  let { workspaceId, folderId, chatId, file }: { workspaceId: string; folderId: string; chatId: string; file: ChatFile } = $props();
  let preview = $state('');
  let code = $state<string | null>(null);
  let error = $state('');
  let busy = $state(false);
  async function open(download: boolean) {
    if (busy) return;
    busy = true; error = '';
    try {
      const result = await localChat(workspaceId, folderId, { action: 'read_attachment', chat_id: chatId, upload_id: file.id });
      if (!result.data_base64) throw new Error($t('chat.57'));
      const bytes = Uint8Array.from(atob(result.data_base64), c => c.charCodeAt(0));
      if (!download && ['image/png','image/jpeg','image/gif','image/webp'].includes(file.mime)) preview = `data:${file.mime};base64,${result.data_base64}`;
      else if (!download && isTextFile(file.name)) {
        if (bytes.length > MAX_PREVIEW_BYTES) throw new Error($t('chat.65'));
        code = new TextDecoder('utf-8', {fatal:true}).decode(bytes);
      } else {
        const url = URL.createObjectURL(new Blob([bytes], {type:'application/octet-stream'}));
        const a = document.createElement('a'); a.href = url; a.download = file.name; a.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
      }
    } catch(e) { error = String(e); } finally { busy = false; }
  }
</script>
<div class="attachment">
  <button class="name" onclick={() => open(false)} disabled={busy} title={file.path}>📎 {file.name} <small>{Math.ceil(file.size / 1024)} {$t('chat.59')}</small></button>
  <button onclick={() => open(true)} disabled={busy}>{$t('chat.52')}</button>
  {#if preview}<ImagePreview src={preview} name={file.name}/>{/if}{#if code !== null}<CodePreview {code} language={file.name}/>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</div>
<style>
.attachment{margin:8px 0;padding:9px 12px;border:1px solid var(--color-border);border-radius:9px;display:flex;gap:12px;flex-wrap:wrap;align-items:center;font-size:11px}.name{flex:1;text-align:left;overflow-wrap:anywhere}small{color:var(--color-text-muted);white-space:nowrap}button{cursor:pointer}button:disabled{opacity:.5}p{color:var(--danger);width:100%;overflow-wrap:anywhere}
</style>
