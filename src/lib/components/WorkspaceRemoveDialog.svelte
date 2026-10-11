<script lang="ts">
  import { removalImpact, type RemovalImpact } from '$lib/workspace/removal-impact';
  import { workspaceFolders } from '$lib/types';
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
  let impact=$state<RemovalImpact|null>(null),impactLoading=$state(true);
  const profile=$derived($workspaces.find(item=>item.id===workspace.id));
  const folders=$derived(profile?workspaceFolders(profile):[]);
  onMount(() => {
    dialog.showModal();let alive=true;
    void removalImpact(workspace.id,folders).then(value=>{if(alive){impact=value;impactLoading=false;}});
    return()=>{alive=false;};
  });
  async function remove() {
    if (busy) return;
    const id = workspace.id;
    const retained = get(workspaces).find(item=>item.id===id);
    busy = true; error = '';
    if (sessionOf(get(sessions), id).working) {
      error = $t('workspace.waitOperation'); busy = false; return;
    }
    blockWorkspace(id);
    try { await deleteWorkspace(id); }
    catch (cause) {
      restoreWorkspace(id);
      if(retained)workspaces.update(items=>items.some(item=>item.id===id)?items:[...items,retained]);
      error = messageOf(cause); busy = false;
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
  <ul>{#each folders as folder(folder.id)}<li>{folder.name}<small>{folder.path}</small></li>{/each}</ul>
  {#if impactLoading}<p role="status">{$t('workspace.impactLoading')}</p>{:else if impact}<p>{$t('workspace.impact',{folders:impact.folders,chats:impact.chats,seeds:impact.seeds})}</p>{#if impact.unavailable}<p class="hint">{$t('workspace.impactUnknown')}</p>{/if}{/if}
  <p class="hint">{$t('workspace.seedImpactHint')}</p>
  {#if busy}<p role="status">{$t('workspace.removingProgress')}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button onclick={() => dialog.close()}>{$t('Cancel')}</button>
    <button class="danger" disabled={busy||impactLoading} onclick={remove}>{busy ? $t('Working…') : $t('workspace.remove')}</button>
  </div>
</dialog>

<style>
  ul{padding-left:18px;font-size:13px}li{margin:8px 0}small{display:block;color:var(--color-text-secondary);overflow-wrap:anywhere}

  dialog{margin:auto;width:min(440px,calc(100vw - 32px));max-height:85dvh;overflow:auto;padding:24px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-1);color:var(--color-text);box-shadow:var(--dialog-shadow)}
  dialog::backdrop{background:var(--backdrop)}h2{font-size:17px;font-weight:600}p{margin:16px 0;line-height:1.7;overflow-wrap:anywhere;font-size:13px}.hint{color:var(--color-text-secondary)}.error{color:var(--danger)}
  .actions{display:flex;justify-content:flex-end;gap:10px;margin-top:24px}button{min-height:40px;padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer}.danger{background:var(--danger);color:white}button:disabled{opacity:.5;cursor:wait}button:focus-visible{outline:2px solid var(--primary);outline-offset:3px}
</style>
