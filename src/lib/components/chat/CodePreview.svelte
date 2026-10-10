<script lang="ts">
  import { t } from '$lib/i18n';
  import { copyText } from '$lib/browser-tools';
  import { isDesktopWindow } from '$lib/stores/glass';
  import { appUrl } from '$lib/app-path';
  import { buildPreviewDocument, previewLanguage, MAX_PREVIEW_BYTES } from '$lib/chat/preview';
  let { code, language = '' }: { code: string; language?: string } = $props();
  let mode = $state<'code'|'run'>('code');
  let document = $state('');
  let runner = $state('');
  let revision = $state(0);
  let frame = $state<HTMLIFrameElement>();
  let copied = $state<string | null>(null);
  let copying = $state(false);
  let copyError = $state(false);
  $effect(() => { code; copied = null; copyError = false; });
  async function copyCode() {
    const value = code;
    copying = true; copyError = false;
    try { await copyText(value); if (code === value) copied = value; }
    catch { if (code === value) { copied = null; copyError = true; } }
    finally { copying = false; }
  }
  const kind = $derived(previewLanguage(language));
  const canRun = $derived(kind && new TextEncoder().encode(code).length <= MAX_PREVIEW_BYTES);
  function run() {
    if (!kind || !canRun) return;
    document = buildPreviewDocument(code, kind);
    runner = !isDesktopWindow() ? appUrl('/chat-preview.html') : /Windows|Android/.test(navigator.userAgent) ? 'http://chatpreview.localhost/index.html' : 'chatpreview://localhost/index.html';
    revision++; mode = 'run';
  }
  function ready(event: MessageEvent) {
    if (frame && event.source === frame.contentWindow && event.data?.type === 'ctmcp-preview-ready' && mode === 'run') frame.contentWindow?.postMessage({type:'ctmcp-preview', document}, '*');
  }
</script>
<svelte:window onmessage={ready}/>
<div class="code-preview">
  <div class="toolbar"><span>{language || $t('chat.60')}</span><div><button class:active={mode === 'code'} onclick={() => mode = 'code'}>{$t('chat.60')}</button>{#if kind}<button class:active={mode === 'run'} onclick={run} disabled={!canRun}>{mode === 'run' ? $t('chat.62') : $t('chat.61')}</button>{/if}<button type="button" onclick={copyCode} disabled={copying} aria-label={$t('chat.copyCode')}><span aria-live="polite">{copied === code ? $t('Copied') : $t('Copy')}</span></button></div></div>
  {#if copyError}<p class="hint copy-error" role="alert">{$t('chat.copyCodeFailed')}</p>{/if}
  {#if mode === 'run'}<p class="hint">{$t('chat.63')}</p>{#key revision}<iframe bind:this={frame} src={runner} sandbox="allow-scripts" referrerpolicy="no-referrer" title={$t('chat.64')}></iframe>{/key}
  {:else}<pre><code>{code}</code></pre>{#if kind && !canRun}<p class="hint">{$t('chat.65')}</p>{/if}{/if}
</div>
<style>
.code-preview{width:100%;margin:10px 0;border:1px solid var(--color-border);border-radius:10px;overflow:hidden;background:var(--color-bg)}.toolbar{display:flex;flex-wrap:wrap;justify-content:space-between;align-items:center;padding:8px 12px;gap:8px;font-size:11px;border-bottom:1px solid var(--color-border)}.toolbar>span{min-width:0;flex:1;font-family:monospace;overflow-wrap:anywhere}.toolbar>div{display:flex;gap:8px;flex-shrink:0}button{padding:4px 9px;border-radius:5px;cursor:pointer}button.active{background:var(--card-bg);color:var(--primary)}button:disabled{opacity:.4;cursor:default}pre{max-height:450px;overflow:auto;padding:14px;font:12px/1.7 monospace;margin:0;white-space:pre;tab-size:2}iframe{display:block;width:100%;height:400px;border:0;background:white}.hint{font-size:10px;color:var(--color-text-muted);padding:7px 12px;margin:0}
.copy-error{color:var(--danger)}button:focus-visible{outline:2px solid var(--primary);outline-offset:2px}</style>
