<script lang="ts">
  import { t } from '$lib/i18n';
  import { getBackend } from '$lib/backend';
  import { appUrl } from '$lib/app-path';
  import { buildPreviewDocument, previewLanguage, MAX_PREVIEW_BYTES } from '$lib/chat/preview';
  let { code, language = '' }: { code: string; language?: string } = $props();
  let mode = $state<'code'|'run'>('code');
  let document = $state('');
  let runner = $state('');
  let revision = $state(0);
  let frame = $state<HTMLIFrameElement>();
  const kind = $derived(previewLanguage(language));
  const canRun = $derived(kind && new TextEncoder().encode(code).length <= MAX_PREVIEW_BYTES);
  function run() {
    if (!kind || !canRun) return;
    document = buildPreviewDocument(code, kind);
    runner = getBackend().capabilities.host === 'node' ? appUrl('/chat-preview.html') : /Windows|Android/.test(navigator.userAgent) ? 'http://chatpreview.localhost/index.html' : 'chatpreview://localhost/index.html';
    revision++; mode = 'run';
  }
  function ready(event: MessageEvent) {
    if (frame && event.source === frame.contentWindow && event.data?.type === 'ctmcp-preview-ready' && mode === 'run') frame.contentWindow?.postMessage({type:'ctmcp-preview', document}, '*');
  }
</script>
<svelte:window onmessage={ready}/>
<div class="code-preview">
  <div class="toolbar"><span>{language || $t('chat.60')}</span><div><button class:active={mode === 'code'} onclick={() => mode = 'code'}>{$t('chat.60')}</button>{#if kind}<button class:active={mode === 'run'} onclick={run} disabled={!canRun}>{mode === 'run' ? $t('chat.62') : $t('chat.61')}</button>{/if}</div></div>
  {#if mode === 'run'}<p class="hint">{$t('chat.63')}</p>{#key revision}<iframe bind:this={frame} src={runner} sandbox="allow-scripts" referrerpolicy="no-referrer" title={$t('chat.64')}></iframe>{/key}
  {:else}<pre><code>{code}</code></pre>{#if kind && !canRun}<p class="hint">{$t('chat.65')}</p>{/if}{/if}
</div>
<style>
.code-preview{width:100%;margin:10px 0;border:1px solid var(--color-border);border-radius:10px;overflow:hidden;background:var(--color-bg)}.toolbar{display:flex;justify-content:space-between;align-items:center;padding:8px 12px;gap:8px;font-size:11px;border-bottom:1px solid var(--color-border)}.toolbar>span{font-family:monospace;overflow-wrap:anywhere}.toolbar>div{display:flex;gap:8px;flex-shrink:0}button{padding:4px 9px;border-radius:5px;cursor:pointer}button.active{background:var(--card-bg);color:var(--primary)}button:disabled{opacity:.4;cursor:default}pre{max-height:450px;overflow:auto;padding:14px;font:12px/1.7 monospace;margin:0;white-space:pre;tab-size:2}iframe{display:block;width:100%;height:400px;border:0;background:white}.hint{font-size:10px;color:var(--color-text-muted);padding:7px 12px;margin:0}
</style>
