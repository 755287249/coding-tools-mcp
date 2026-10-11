<script lang="ts">
  import { get } from 'svelte/store';
  import { blockWorkspace, restoreWorkspace } from '$lib/stores/workspace-lifecycle';
  import { sessions, sessionOf, messageOf } from '$lib/stores/sessions';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl } from '$lib/app-path';
  import { deleteWorkspace } from '$lib/api/workspaces';
  import { workspaces, mcpRuntimeStates, actionsRuntimeStates } from '$lib/stores/app';
  import { showToast } from '$lib/stores/toast';
  import { t } from '$lib/i18n';

  let { workspace, onClose }: { workspace: {id:string;name:string}; onClose: () => void } = $props();
  let dialog: HTMLDialogElement;
  let busy = $state(false), error = $state('');
  onMount(() => { dialog.showModal(); });
  async function remove() {
    if (busy) return;
    const id = workspace.id;
    busy = true; error = '';
    if (sessionOf(get(sessions), id).working) {
      error = $t('workspace.waitOperation'); busy = false; return;
    }
    blockWorkspace(id);
    try { await deleteWorkspace(id); }
    catch (cause) {
      restoreWorkspace(id); error = messageOf(cause); busy = false;
      showToast(error, {kind:'error'}); return;
    }
    sessions.update(items => { const next = {...items}; delete next[id]; return next; });
    workspaces.update(items => items.filter(item => item.id !== id));
    for (const states of [mcpRuntimeStates, actionsRuntimeStates]) states.update(items => {
      const next = {...items}; delete next[id]; return next;
    });
    const active = $page.params.id === id;
    dialog.close();
    showToast($t('workspace.removed'), {kind:'success'});
    if (active) { try { await goto(appUrl('/')); } catch (cause) { showToast(String(cause), {kind:'error'}); } }
  }
</script>

<dialog bind:this={dialog} aria-label={$t('workspace.remove')} onclose={onClose}>
  <h2>{$t('workspace.remove')}</h2>
  <p>{$t('workspace.removeConfirm', {name:workspace.name})}</p>
  <p class="hint">{$t('workspace.removeHint')}</p>
  {#if busy}<p role="status">{$t('workspace.removingProgress')}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button onclick={() => dialog.close()}>{$t('Cancel')}</button>
    <button class="danger" disabled={busy} onclick={remove}>{busy ? $t('Working…') : $t('workspace.remove')}</button>
  </div>
</dialog>

<style>
  dialog{margin:auto;width:min(440px,calc(100vw - 32px));max-height:85dvh;overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-1);color:var(--color-text);box-shadow:var(--dialog-shadow)}
  dialog::backdrop{background:var(--backdrop)}h2{font-size:17px;font-weight:600}p{margin:16px 0;line-height:1.7;overflow-wrap:anywhere;font-size:13px}.hint{color:var(--color-text-secondary)}.error{color:var(--danger)}
  .actions{display:flex;justify-content:flex-end;gap:10px;margin-top:24px}button{min-height:40px;padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer}.danger{background:var(--danger);color:white}button:disabled{opacity:.5;cursor:wait}button:focus-visible{outline:2px solid var(--primary);outline-offset:3px}
</style>
