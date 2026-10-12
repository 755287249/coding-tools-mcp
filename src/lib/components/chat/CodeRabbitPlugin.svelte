<script lang="ts">
  import { t } from '$lib/i18n';
  import { mcpConnection } from '$lib/stores/app';
  import { copyText } from '$lib/browser-tools';
  import { DEFAULT_SEED_COUNT, SESSION_SNIPPET } from '$lib/plugins/coderabbit/engine';
  import { canOpenLoginWindow, loginWithText, loginWithWindow, startSeeding } from '$lib/plugins/coderabbit/runner';
  import { coderabbitSession, githubDeviceCode, seedingJob } from '$lib/plugins/coderabbit/store';
  import LogIn from '@lucide/svelte/icons/log-in';
  import Sprout from '@lucide/svelte/icons/sprout';
  let { workspaceId, folderId }: { workspaceId: string; folderId: string } = $props();
  const desktop = canOpenLoginWindow();
  let count = $state(DEFAULT_SEED_COUNT), prefix = $state(''), pasted = $state(''), busy = $state(false), error = $state('');
  const running = $derived($seedingJob?.status === 'running');
  async function login() {
    if (busy) return; busy = true; error = '';
    try { if (desktop) await loginWithWindow(); else { loginWithText(pasted); pasted = ''; } }
    catch (e) { error = e instanceof Error ? e.message : String(e); } finally { busy = false; }
  }
  function start() {
    error = '';
    try { startSeeding({ workspaceId, folderId, publicUrl: $mcpConnection?.tunnel.public_url, count, prefix }); }
    catch (e) { error = e instanceof Error ? e.message : String(e); }
  }
</script>
<div class="plugin">
  <p class="notice">{$t('rabbit.help')}</p>
  {#if $coderabbitSession}
    <div class="account"><span class="dot"></span><div><strong>{$coderabbitSession.accountName}</strong><small>{$coderabbitSession.orgName}</small></div>
      <button type="button" class="link" disabled={busy || running} onclick={() => coderabbitSession.set(null)}>{$t('rabbit.logout')}</button></div>
    <div class="row">
      <label>{$t('rabbit.count')}<input type="number" min="1" max="1000" bind:value={count} disabled={running}/></label>
      <label>{$t('seeds.prefix')}<input maxlength="200" bind:value={prefix} placeholder="Opus / GPT-6-astra" disabled={running}/></label>
    </div>
    <p class="hint">{$t('rabbit.repoHint')}</p>
    <button type="button" class="primary" disabled={running || !Number.isInteger(count) || count < 1} onclick={start}><Sprout size={16}/>{running ? $t('rabbit.running') : $t('rabbit.start', { count })}</button>
  {:else if desktop}
    <button type="button" class="primary" disabled={busy} onclick={login}><LogIn size={16}/>{busy ? $t('rabbit.loggingIn') : $t('rabbit.login')}</button>
    <p class="hint">{$t('rabbit.loginHint')}</p>
  {:else}
    <p class="hint">{$t('rabbit.pasteHint')}</p>
    <div class="snippet"><code>{SESSION_SNIPPET}</code><button type="button" onclick={() => void copyText(SESSION_SNIPPET)}>{$t('seeds.copy')}</button></div>
    <textarea rows="3" bind:value={pasted} placeholder={$t('rabbit.pastePlaceholder')}></textarea>
    <button type="button" class="primary" disabled={busy || !pasted.trim()} onclick={login}><LogIn size={16}/>{$t('rabbit.useSession')}</button>
  {/if}
  {#if $githubDeviceCode}<div class="device" role="status"><p>{$t('rabbit.deviceHint')}</p><strong>{$githubDeviceCode.user_code}</strong><button type="button" onclick={() => void copyText($githubDeviceCode?.user_code ?? '')}>{$t('seeds.copy')}</button>{#if !desktop}<a href={$githubDeviceCode.verification_uri} target="_blank" rel="noreferrer">{$githubDeviceCode.verification_uri}</a>{/if}</div>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>
<style>
  .plugin{display:grid;gap:12px}.notice,.hint{margin:0;font-size:12px;line-height:1.6;color:var(--color-text-muted)}
  .account{display:flex;align-items:center;gap:10px;padding:10px 12px;border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg)}
  .account div{display:flex;flex-direction:column;flex:1;min-width:0}.account small{font-size:11.5px;color:var(--color-text-muted)}
  .dot{width:8px;height:8px;border-radius:50%;background:#16a34a}
  .row{display:grid;grid-template-columns:120px 1fr;gap:10px}label{display:grid;gap:5px;font-size:12px;color:var(--color-text-muted)}
  input,textarea{padding:8px 10px;border:1px solid var(--color-border);border-radius:9px;background:var(--input-bg,var(--card-bg));color:var(--color-text);font:inherit;font-size:13px}
  .primary{display:inline-flex;align-items:center;justify-content:center;gap:8px;height:38px;border:0;border-radius:10px;background:var(--action,#2563eb);color:#fff;font-weight:600;cursor:pointer}.primary:disabled{opacity:.55;cursor:default}
  .link{border:0;background:transparent;color:var(--color-text-muted);font-size:12px;cursor:pointer}
  .snippet{display:flex;gap:8px;align-items:flex-start}.snippet code{flex:1;font-size:11px;word-break:break-all;padding:8px;border-radius:8px;background:var(--surface-2,var(--color-surface))}
  .snippet button,.device button{padding:6px 10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);color:var(--color-text);font-size:12px}
  .device{display:flex;flex-wrap:wrap;align-items:center;gap:10px;padding:12px;border:1px dashed #2563eb;border-radius:12px}.device p{flex-basis:100%;margin:0;font-size:12px}.device strong{font:600 20px/1 ui-monospace,monospace;letter-spacing:.12em}
  .error{margin:0;color:var(--danger,#dc2626);font-size:12.5px}
</style>
