<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { getMcpConnection, getRuntimeStatus } from '$lib/api/workspaces';
  import { getBackend, loadMcpAuthSecrets } from '$lib/backend';
  import { copyText } from '$lib/browser-tools';
  import { buildSharePrompt, isPublicEndpoint, type ShareAccess } from '$lib/connect/share-prompt';
  import { workspaceFolders, type WorkspaceProfile } from '$lib/types';
  import { t, locale } from '$lib/i18n';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';

  let { workspace, onClose }: { workspace: WorkspaceProfile; onClose: () => void } = $props();
  let dialog: HTMLDialogElement;
  const folders = $derived(workspaceFolders(workspace));
  // The dialog is opened per project, so the first folder is a stable default.
  let folderId = $state(untrack(() => workspaceFolders(workspace)[0]?.id ?? ''));
  let access = $state<ShareAccess>('readonly');
  let connection = $state<{endpoint:string;authType:string;clientId:string;password:string;bearerToken:string}|null>(null);
  let loading = $state(true), error = $state(''), copied = $state(false);

  const folder = $derived(folders.find(item => item.id === folderId) ?? folders[0]);
  const prompt = $derived.by(() => {
    if (!connection || !folder) return '';
    try { return buildSharePrompt({ ...connection, folderId: folder.id, folderName: folder.name, access }, $locale); }
    catch { return ''; }
  });
  const publicReady = $derived(!!connection && isPublicEndpoint(connection.endpoint));

  onMount(() => {
    dialog.showModal();
    let alive = true;
    void (async () => {
      try {
        const transport = await getMcpConnection();
        const status = await getRuntimeStatus(transport.id);
        const endpoint = status.publicEndpoint || status.localEndpoint;
        if (status.state !== 'running' || !endpoint) throw Error($t('share.noEndpoint'));
        const secrets = await loadMcpAuthSecrets(getBackend(), transport.id, transport.auth);
        const next = { endpoint, authType: transport.auth.type, clientId: secrets.oauth_client_id || transport.auth.oauth_client_id || '', password: secrets.oauth_password ?? '', bearerToken: secrets.bearer_token ?? '' };
        if ((next.authType === 'bearer' && !next.bearerToken) || (next.authType === 'oauth' && (!next.clientId || !next.password))) throw Error($t('share.missingAuth'));
        if (alive) connection = next;
      } catch (cause) { if (alive) error = cause instanceof Error ? cause.message : String(cause); }
      finally { if (alive) loading = false; }
    })();
    return () => { alive = false; };
  });
  $effect(() => { access; folderId; copied = false; });
  async function copy() {
    if (!prompt) return;
    try { await copyText(prompt); copied = true; } catch (cause) { error = String(cause); }
  }
</script>

<dialog bind:this={dialog} aria-labelledby="share-workspace-title" onclose={onClose}>
  <header><h2 id="share-workspace-title">{$t('share.title')}</h2><button class="icon" aria-label={$t('Close')} onclick={() => dialog.close()}><X size={18}/></button></header>
  <p class="hint">{$t('share.help', { name: workspace.name })}</p>
  {#if folders.length > 1}
    <label class="field"><span>{$t('share.folder')}</span>
      <select bind:value={folderId}>{#each folders as item (item.id)}<option value={item.id}>{item.name} — {item.path}</option>{/each}</select>
    </label>
  {/if}
  <fieldset>
    <legend>{$t('share.access')}</legend>
    <label><input type="radio" name="share-access" value="readonly" bind:group={access}/><span><b>{$t('share.readonly')}</b><small>{$t('share.readonlyHelp')}</small></span></label>
    <label><input type="radio" name="share-access" value="full" bind:group={access}/><span><b>{$t('share.full')}</b><small>{$t('share.fullHelp')}</small></span></label>
  </fieldset>
  {#if loading}<p role="status">{$t('Loading…')}</p>
  {:else if error}<p class="error" role="alert">{error}</p>
  {:else}
    {#if !publicReady}<p class="warn" role="status">{$t('share.localOnly')}</p>{/if}
    <textarea readonly aria-label={$t('share.prompt')} value={prompt}></textarea>
    <p class="warn">{$t('share.credentialWarning')}</p>
  {/if}
  <div class="actions">
    <button onclick={() => dialog.close()}>{$t('Close')}</button>
    <button class="primary" disabled={!prompt} onclick={copy}>{#if copied}<Check size={14}/>{$t('chat.25')}{:else}<Copy size={14}/>{$t('chat.24')}{/if}</button>
  </div>
</dialog>

<style>
  dialog{margin:auto;width:min(520px,calc(100vw - 32px));max-height:88dvh;overflow:auto;padding:22px;border:1px solid var(--color-border);border-radius:16px;background:var(--surface-1);color:var(--color-text);box-shadow:var(--dialog-shadow)}
  dialog::backdrop{background:var(--backdrop)}
  header{display:flex;align-items:center;justify-content:space-between;gap:12px}h2{font-size:17px;font-weight:600;margin:0}
  p{margin:12px 0;line-height:1.6;font-size:13px;overflow-wrap:anywhere}.hint{color:var(--color-text-secondary)}.error{color:var(--danger)}.warn{color:var(--warning);font-size:12px}
  .field{display:grid;gap:6px;font-size:13px;margin:12px 0}select{padding:8px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);color:inherit}
  fieldset{border:1px solid var(--color-border);border-radius:10px;padding:8px 12px;margin:12px 0}legend{font-size:12px;color:var(--color-text-secondary);padding:0 4px}
  fieldset label{display:flex;gap:10px;align-items:flex-start;padding:8px 0;cursor:pointer;font-size:13px}fieldset label span{display:grid;gap:2px}fieldset small{color:var(--color-text-secondary);font-size:12px}
  textarea{width:100%;min-height:170px;resize:vertical;padding:10px;border:1px solid var(--color-border);border-radius:10px;background:var(--card-bg);color:inherit;font:12px/1.55 ui-monospace,monospace;box-sizing:border-box}
  .actions{display:flex;justify-content:flex-end;gap:10px;margin-top:16px}
  button{display:inline-flex;align-items:center;gap:6px;min-height:38px;padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer;background:transparent;color:inherit}
  button.icon{min-height:32px;padding:6px;border:0}.primary{background:var(--primary);color:white;border-color:transparent}button:disabled{opacity:.5;cursor:not-allowed}
  button:focus-visible,select:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:2px}
</style>
