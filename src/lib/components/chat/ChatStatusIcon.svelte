<script lang="ts">
  import Circle from '@lucide/svelte/icons/circle';
  import CheckCheck from '@lucide/svelte/icons/check-check';
  import Check from '@lucide/svelte/icons/check';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleHelp from '@lucide/svelte/icons/circle-help';
  import CircleX from '@lucide/svelte/icons/circle-x';
  import Clock from '@lucide/svelte/icons/clock';
  import LoaderCircle from '@lucide/svelte/icons/loader-circle';
  import Square from '@lucide/svelte/icons/square';
  import Pause from '@lucide/svelte/icons/pause';
  const icons = {
    unread: Circle, read: CheckCheck, queued: Clock,
    processing: LoaderCircle, supplementing: LoaderCircle, running: LoaderCircle,
    awaiting_user: CircleHelp, answered: Check, replied: CircleCheck,
    complete: CircleCheck, completed: CircleCheck, failed: CircleX,
    closed: Square, interrupted: Pause,
  };
  let { state, label }: {state: keyof typeof icons; label: string} = $props();
  const Icon = $derived(icons[state]);
  const spinning = $derived(state === 'processing' || state === 'supplementing' || state === 'running');
</script>
<span class="status-icon" class:spinning class:attention={state === 'awaiting_user'} class:failed={state === 'failed'} class:done={state === 'complete' || state === 'completed' || state === 'replied' || state === 'read' || state === 'answered'} role="img" aria-label={label} title={label}>
  <Icon size={14} aria-hidden="true"/>
</span>
<style>
  .status-icon{display:inline-flex;align-items:center;justify-content:center;flex:none;vertical-align:middle;color:var(--color-text-muted)}
  .done{color:var(--success,#65a97e)}.attention{color:var(--primary)}.failed{color:var(--danger)}
  .spinning{color:var(--primary);animation:chat-status-spin 1.4s linear infinite}
  @keyframes chat-status-spin{to{transform:rotate(360deg)}}
  @media(prefers-reduced-motion:reduce){.spinning{animation:none}}
</style>
