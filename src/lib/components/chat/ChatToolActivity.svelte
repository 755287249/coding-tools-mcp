<script lang="ts">
  import { t } from '$lib/i18n';
  import type { ChatMessage } from '$lib/api/chat';
  import { summarizeToolActivity } from '$lib/chat/tool-activity';
  import ChatMarkdown from './ChatMarkdown.svelte';
  import ChatStatusIcon from './ChatStatusIcon.svelte';
  import ChatAttachment from './ChatAttachment.svelte';
  let { messages, settled, workspaceId, folderId, chatId }: { messages: ChatMessage[]; settled: boolean; workspaceId: string; folderId: string; chatId: string } = $props();
  const summary = $derived(summarizeToolActivity(messages, settled));
  const running = $derived(summary.running > 0);
  const state = $derived(running ? 'running' : summary.failed ? 'failed' : summary.unresolved ? 'interrupted' : 'completed');
  let expanded = $state(false);
  let previousRunning: boolean | null = null;
  $effect(() => {
    if (previousRunning !== running) { expanded = running; previousRunning = running; }
  });
</script>
<details class="tool-activity" bind:open={expanded} data-running={running}>
  <summary>
    <ChatStatusIcon {state} label={running ? $t('chat.55') : summary.failed ? $t('chat.58') : summary.unresolved ? $t('chat.95') : $t('chat.56')}/>
    <span>{$t('chat.92')} {summary.calls}</span>
    {#if running}<span class="working">{$t('chat.55')} {summary.running}</span>{/if}
    {#if summary.failed}<span class="failed">{$t('chat.58')} {summary.failed}</span>{/if}
    {#if summary.unresolved}<span class="working">{$t('chat.95')} {summary.unresolved}</span>{/if}
    {#if !running && !summary.failed && !summary.unresolved}<span class="done">{$t('chat.56')}</span>{/if}
  </summary>
  <div class="tool-reports">
    {#each messages as message (message.id)}
      {#if message.tool_event}
        <details class="tool-report">
          <summary><span>{message.tool_event.name}</span><ChatStatusIcon state={message.tool_event.status === 'running' ? 'queued' : message.tool_event.status} label={message.tool_event.status === 'running' ? $t('chat.93') : message.tool_event.status === 'failed' ? $t('chat.58') : $t('chat.56')}/><time>{new Date(message.created_at).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'})}</time></summary>
          <ChatMarkdown text={message.text} {workspaceId} {folderId} {chatId}/>
          {#if message.tool_event.input}<h4>{$t('chat.76')}</h4><pre>{message.tool_event.input}</pre>{/if}
          {#if message.tool_event.output}<h4>{$t('chat.77')}</h4><pre>{message.tool_event.output}</pre>{/if}
          {#if message.tool_event.output_truncated}<p>{$t('chat.78')}</p>{/if}
          {#each message.attachments ?? [] as file (workspaceId + ':' + folderId + ':' + chatId + ':' + file.id)}<ChatAttachment {workspaceId} {folderId} {chatId} {file}/>{/each}
        </details>
      {/if}
    {/each}
  </div>
</details>
<style>
.tool-activity{font-size:12px}.tool-activity>summary{cursor:pointer;display:flex;align-items:center;gap:8px;flex-wrap:wrap;list-style:none}.tool-activity>summary::before{content:'›';font-size:17px;line-height:1;transition:transform .15s}.tool-activity[open]>summary::before{transform:rotate(90deg)}.working,.done,time{font-size:10px;color:var(--color-text-muted)}.failed{color:var(--danger);font-size:10px}.tool-reports{border-left:1px solid var(--color-border);margin:10px 0 0 6px;padding-left:12px}.tool-report{padding:7px 0}.tool-report summary{cursor:pointer;display:flex;align-items:center;gap:8px;color:var(--color-text-muted)}.tool-report summary span{overflow-wrap:anywhere}.tool-report time{margin-left:auto;white-space:nowrap}pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:340px;overflow:auto;font:11px/1.6 monospace;margin:8px 0}h4{font-size:11px;font-weight:600;margin-top:12px}.tool-report p{color:var(--color-text-muted)}
</style>
