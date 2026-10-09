<script lang="ts">
  import { t } from '$lib/i18n';
  import { USER_MESSAGE_STATUS_KEYS, type chatUserState } from '$lib/chat/status';
  import ChatStatusIcon from './ChatStatusIcon.svelte';
  let { state }: {state: ReturnType<typeof chatUserState>} = $props();
</script>
{#if state}
  <div class="receipt" role="group" aria-label={$t('chat.84')}>
    <ChatStatusIcon state={state.read ? 'read' : 'unread'} label={$t(state.read ? 'chat.79' : 'chat.80')}/>
    <ChatStatusIcon state={state.status} label={$t(USER_MESSAGE_STATUS_KEYS[state.status])}/>
  </div>
{/if}
<style>
  .receipt{margin-top:8px;display:flex;justify-content:flex-end;gap:6px;color:var(--color-text-muted)}
</style>
