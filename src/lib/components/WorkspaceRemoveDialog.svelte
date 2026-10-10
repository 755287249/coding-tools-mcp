<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { appUrl } from '$lib/app-path';
  import { deleteWorkspace } from '$lib/api/workspaces';
  import { getBackend } from '$lib/backend';
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
    try { await deleteWorkspace(id); }
    catch (cause) { error = String(cause); busy = false; return; }
    workspaces.update(items => items.filter(item => item.id !== id));
    for (const states of [mcpRuntimeStates, actionsRuntimeStates]) states.update(items => {
      const next = {...items}; delete next[id]; return next;
    });
    const active = $page.params.id === id;
    dialog.close();
    if (getBackend().capabilities.agentRestart) showToast($t('workspace.removedRestart'), {kind:'success'});
    if (active) { try { await goto(appUrl('/')); } catch (cause) { showToast(String(cause), {kind:'error'}); } }
  }
</script>

<dialog bind:this={dialog} aria-label={$t('workspace.remove')} onclose={onClose} oncancel={event => {if(busy)event.preventDefault();}}>
  <h2>{$t('workspace.remove')}</h2>
  <p>{$t('workspace.removeConfirm', {name:workspace.name})}</p>
  <p class="hint">{$t('workspace.removeHint')}</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button disabled={busy} onclick={() => dialog.close()}>{$t('Cancel')}</button>
    <button class="danger" disabled={busy} onclick={remove}>{busy ? $t('Working…') : $t('workspace.remove')}</button>
  </div>
</dialog>

<style>
  dialog{margin:auto;width:min(440px,calc(100vw - 32px));max-height:85dvh;overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-1);color:var(--color-text);box-shadow:var(--dialog-shadow)}
  dialog::backdrop{background:var(--backdrop)}h2{font-size:17px;font-weight:600}p{margin:16px 0;line-height:1.7;overflow-wrap:anywhere;font-size:13px}.hint{color:var(--color-text-secondary)}.error{color:var(--danger)}
  .actions{display:flex;justify-content:flex-end;gap:10px;margin-top:24px}button{min-height:40px;padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer}.danger{background:var(--danger);color:white}button:disabled{opacity:.5;cursor:wait}button:focus-visible{outline:2px solid var(--primary);outline-offset:3px}
</style>
