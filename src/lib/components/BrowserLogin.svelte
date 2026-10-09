<script lang="ts">
  import { browserLogin } from '$lib/backend/browser-session';
  import { t } from '$lib/i18n';
  let { onLogin }: { onLogin: () => Promise<void> } = $props();
  let password = $state(''); let busy = $state(false); let error = $state('');
  async function login() {
    if (busy) return; busy = true; error = '';
    try { await browserLogin(password); password = ''; await onLogin(); }
    catch (e) { error = String(e); } finally { busy = false; }
  }
</script>
<main class="browser-login"><form onsubmit={e=>{e.preventDefault();void login()}}><h1>Coding Tools MCP</h1><h2>{$t('sharing.login')}</h2><p>{$t('sharing.loginHint')}</p><label>{$t('sharing.password')}<input type="password" autocomplete="current-password" bind:value={password} required disabled={busy}/></label>{#if error}<p role="alert">{error}</p>{/if}<button type="submit" disabled={busy||!password}>{busy?$t('Loading…'):$t('sharing.login')}</button></form></main>
<style>.browser-login{min-height:100dvh;display:grid;place-items:center;padding:24px;background:#181818;color:#eee}.browser-login form{width:min(100%,400px);display:grid;gap:20px}h1{font-size:24px}h2{font-size:18px}p{font-size:13px;color:#aaa;line-height:1.6}label{display:grid;gap:8px}input,button{border-radius:8px;padding:12px;border:1px solid #ffffff30;background:#292929}button{cursor:pointer;background:#ddd;color:#111}button:disabled{opacity:.5}[role=alert]{color:#f99}</style>
