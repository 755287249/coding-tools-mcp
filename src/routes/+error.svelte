<script lang="ts">
  import { page } from '$app/stores';
  import { t } from '$lib/i18n';
  import { appUrl } from '$lib/app-path';
</script>
<section class="load-error" role="alert">
  <h1>{$t('mobile.pageLoadFailed')}</h1>
  <p>{$t('mobile.pageLoadHint')}</p>
  <small>{$page.status}</small>
  <div><button type="button" onclick={()=>window.location.reload()}>{$t('mobile.reloadPage')}</button><a href={appUrl('/')} data-sveltekit-reload>{$t('chat.107')}</a></div>
</section>
<style>
.load-error{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:16px;min-height:50dvh;padding:32px 24px;text-align:center;color:var(--color-text)}h1{font-size:22px;font-weight:600}p{max-width:420px;line-height:1.7;color:var(--color-text-muted)}small{color:var(--color-text-muted)}.load-error div{display:flex;gap:12px;flex-wrap:wrap;justify-content:center}button,a{display:grid;place-items:center;min-height:44px;padding:10px 18px;border:1px solid var(--color-border);border-radius:10px;cursor:pointer}button{background:var(--primary);color:white}button:focus-visible,a:focus-visible{outline:2px solid var(--primary);outline-offset:3px}
</style>
