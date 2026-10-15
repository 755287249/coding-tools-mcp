<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { getMcpConnection, getRuntimeStatus } from '$lib/api/workspaces';
  import { getBackend } from '$lib/backend';
  import { copyText } from '$lib/browser-tools';
  import { buildSharePrompt, isPublicEndpoint, shareUrl, type ShareAccess } from '$lib/connect/share-prompt';
  import { workspaceFolders, type WorkspaceProfile } from '$lib/types';
  import { t, locale } from '$lib/i18n';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';

  interface ShareStatus { enabled: boolean; access?: ShareAccess; url_key?: string }
  let { workspace, onClose }: { workspace: WorkspaceProfile; onClose: () => void } = $props();
  let dialog: HTMLDialogElement;
  const folders = $derived(workspaceFolders(workspace));
  // The dialog is opened per project, so the first folder is a stable default.
  let folderId = $state(untrack(() => workspaceFolders(workspace)[0]?.id ?? ''));
  let access = $state<ShareAccess>('readonly');
  let endpoint = $state(''), status = $state<ShareStatus | null>(null), key = $state('');
  let loading = $state(true), busy = $state(false), error = $state(''), copied = $state(false);

  // The server stores only a hash, so this browser keeps the key it generated.
  const cacheKey = (folder: string) => `ctmcp-share:${JSON.stringify([workspace.id, folder])}`;
  const readKey = (folder: string) => { try { return localStorage.getItem(cacheKey(folder)) ?? ''; } catch { return ''; } };
  const writeKey = (folder: string, value: string) => { try { if (value) localStorage.setItem(cacheKey(folder), value); else localStorage.removeItem(cacheKey(folder)); } catch { /* private mode */ } };
  const request = (args: Record<string, unknown>) => getBackend().chat.request(workspace.id, folderId, args as never) as Promise<ShareStatus>;

  const url = $derived.by(() => { if (!endpoint || !status?.enabled || !key) return ''; try { return shareUrl(endpoint, folderId, key); } catch { return ''; } });
  const prompt = $derived(url ? buildSharePrompt(url, $locale) : '');
  const matches = $derived(!!prompt && status?.access === access);

  async function refresh(folder: string) {
    error = ''; copied = false;
    try {
      const next = await request({ action: 'share_status' });
      if (folder !== folderId) return;
      status = next; key = next.enabled ? readKey(folder) : '';
      if (next.enabled && next.access) access = next.access;
      if (!next.enabled) writeKey(folder, '');
    } catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
  }
  onMount(() => {
    dialog.showModal();
    let alive = true;
    void (async () => {
      try {
        const transport = await getMcpConnection();
        const runtime = await getRuntimeStatus(transport.id);
        const value = runtime.publicEndpoint || runtime.localEndpoint;
        if (runtime.state !== 'running' || !value) throw Error($t('share.noEndpoint'));
        if (alive) endpoint = value;
        await refresh(folderId);
      } catch (cause) { if (alive) error = cause instanceof Error ? cause.message : String(cause); }
      finally { if (alive) loading = false; }
    })();
    return () => { alive = false; };
  });
  async function generate() {
    busy = true; error = ''; copied = false;
    const folder = folderId;
    try {
      const next = await request({ action: 'share_create', access });
      if (!next.url_key) throw Error('Share link was not created');
      writeKey(folder, next.url_key);
      if (folder === folderId) { status = next; key = next.url_key; }
    } catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
    finally { busy = false; }
  }
  async function revoke() {
    busy = true; error = '';
    try { status = await request({ action: 'share_revoke' }); key = ''; writeKey(folderId, ''); copied = false; }
    catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
    finally { busy = false; }
  }
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
      <select bind:value={folderId} onchange={() => { if (endpoint) void refresh(folderId); }}>{#each folders as item (item.id)}<option value={item.id}>{item.name} — {item.path}</option>{/each}</select>
    </label>
  {/if}
  <fieldset disabled={busy}>
    <legend>{$t('share.access')}</legend>
    <label><input type="radio" name="share-access" value="readonly" bind:group={access}/><span><b>{$t('share.readonly')}</b><small>{$t('share.readonlyHelp')}</small></span></label>
    <label><input type="radio" name="share-access" value="full" bind:group={access}/><span><b>{$t('share.full')}</b><small>{$t('share.fullHelp')}</small></span></label>
  </fieldset>
  {#if loading}<p role="status">{$t('Loading…')}</p>
  {:else if error}<p class="error" role="alert">{error}</p>
  {/if}
  {#if !loading && endpoint}
    {#if !isPublicEndpoint(endpoint)}<p class="warn" role="status">{$t('share.localOnly')}</p>{/if}
    {#if matches}
      <textarea readonly aria-label={$t('share.prompt')} value={prompt}></textarea>
      <p class="hint">{$t('share.linkNote')}</p>
    {:else if status?.enabled}
      <p class="hint">{$t(prompt ? 'share.accessChanged' : 'share.keyMissing')}</p>
    {:else}
      <p class="hint">{$t('share.inactive')}</p>
    {/if}
  {/if}
  <div class="actions">
    {#if status?.enabled}<button class="danger" disabled={busy} onclick={revoke}>{$t('share.revoke')}</button>{/if}
    <span class="spacer"></span>
    {#if matches}
      <button class="primary" onclick={copy}>{#if copied}<Check size={14}/>{$t('chat.25')}{:else}<Copy size={14}/>{$t('chat.24')}{/if}</button>
    {:else}
      <button class="primary" disabled={busy || !endpoint} onclick={generate}>{$t(status?.enabled ? 'share.regenerate' : 'share.generate')}</button>
    {/if}
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
  textarea{width:100%;min-height:120px;resize:vertical;padding:10px;border:1px solid var(--color-border);border-radius:10px;background:var(--card-bg);color:inherit;font:12px/1.55 ui-monospace,monospace;box-sizing:border-box}
  .actions{display:flex;align-items:center;gap:10px;margin-top:16px}.spacer{flex:1}
  button{display:inline-flex;align-items:center;gap:6px;min-height:38px;padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer;background:transparent;color:inherit}
  button.icon{min-height:32px;padding:6px;border:0}.primary{background:var(--primary);color:white;border-color:transparent}.danger{color:var(--danger)}button:disabled{opacity:.5;cursor:not-allowed}
  button:focus-visible,select:focus-visible,input:focus-visible{outline:2px solid var(--primary);outline-offset:2px}
</style>
