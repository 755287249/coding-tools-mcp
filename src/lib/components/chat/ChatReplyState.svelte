<script lang="ts">
  import { t } from '$lib/i18n';
  import type { chatReplyState } from '$lib/chat/status';
  import ChatStatusIcon from './ChatStatusIcon.svelte';
  let { state }: { state: ReturnType<typeof chatReplyState> } = $props();
</script>
{#if state}
  <div class="reply-state" class:awaiting={state === 'awaiting_user'}>
    <ChatStatusIcon {state} label={state === 'supplementing' ? $t('chat.72') : state === 'awaiting_user' ? $t('chat.73') : state === 'answered' ? $t('chat.75') : $t('chat.74')}/>
  </div>
{/if}
<style>
  .reply-state{margin-top:10px;font-size:11px;color:var(--color-text-muted)}
  .reply-state.awaiting{color:var(--primary);font-weight:600}
</style>
