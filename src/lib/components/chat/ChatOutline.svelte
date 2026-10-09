<script lang="ts">
  import type { ChatMessage } from '$lib/api/chat';
  import { messageOutline } from '$lib/chat/navigation';
  import { t } from '$lib/i18n';
  let { messages, activeId, onSelect }: { messages: ChatMessage[]; activeId: string; onSelect: (id: string) => void } = $props();
  const items = $derived(messageOutline(messages));
  let preview = $state<{ id: string; title: string; text: string; top: number; left: number } | null>(null);
  function showPreview(event: MouseEvent | FocusEvent, item: (typeof items)[number]) {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    preview = { id: item.id, title: item.title, text: item.preview, top: Math.max(8, Math.min(rect.top - 24, window.innerHeight - 160)), left: rect.right + 8 };
  }
</script>

{#if items.length}
  <nav class="chat-outline" aria-label={$t('chat.97')}>
    {#each items as item, i (item.id)}
      <button type="button" class:active={activeId === item.id} aria-current={activeId === item.id ? 'location' : undefined} aria-label={`${i + 1}. ${item.title}`} onmouseenter={event => showPreview(event, item)} onmouseleave={() => preview = null} onfocus={event => showPreview(event, item)} onblur={() => preview = null} onclick={() => { preview = null; onSelect(item.id); }}><span></span></button>
    {/each}
  </nav>
  {#if preview}<div class="outline-preview" role="tooltip" style:top={`${preview.top}px`} style:left={`${preview.left}px`}><strong>{preview.title}</strong>{#if preview.text}<p>{preview.text}</p>{/if}</div>{/if}
{/if}

<style>
  .chat-outline{position:absolute;left:3px;top:50%;transform:translateY(-50%);max-height:85%;overflow-y:auto;scrollbar-width:none;display:flex;flex-direction:column;align-items:center;width:20px;padding:5px 0;gap:1px}
  button{display:grid;place-items:center;flex:none;width:20px;height:10px;cursor:pointer;border-radius:4px}button span{height:2px;width:10px;background:var(--color-text-muted);opacity:.5;transition:width .15s,opacity .15s}button.active span,button:hover span,button:focus-visible span{width:16px;opacity:1;background:var(--color-text)}button:focus-visible{outline:1px solid var(--color-text-muted);outline-offset:-1px}
  .outline-preview{position:fixed;z-index:60;pointer-events:none;width:min(320px,calc(100vw - 90px));max-height:145px;overflow:hidden;border:1px solid var(--color-border);border-radius:8px;padding:11px 13px;background:var(--card-bg);box-shadow:0 6px 24px #0004;font-size:11px;line-height:1.6;overflow-wrap:anywhere}.outline-preview strong{font-weight:550}.outline-preview p{margin:5px 0 0;color:var(--color-text-muted);display:-webkit-box;line-clamp:3;-webkit-line-clamp:3;-webkit-box-orient:vertical;overflow:hidden}
  @media(prefers-reduced-motion:reduce){button span{transition:none}}
</style>
