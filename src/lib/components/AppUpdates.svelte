<script lang="ts">
  import { confirm as confirmAction } from '$lib/api/native';
  import { t } from '$lib/i18n';
  import { displayReleaseNotes } from '$lib/update-notes';
  import { getBackend } from '$lib/backend';
  import { distributionCall, localDesktop } from '$lib/api/distribution';
  import { onMount } from 'svelte';
  import {APP_VERSION} from '$lib/app-version';
  import {appUpdate,appDownload,checkAppUpdate,updateRepository} from '$lib/stores/app-updates';
  import {downloadBusy,downloadPercent,formatDownloadBytes} from '$lib/update-download';
  let repo=$state('755287249/coding-tools-mcp'),release=$derived($appUpdate.release),installing=$state(false),error=$state(''),ready=$derived($appDownload.phase==='ready'),checkedRepo=$derived($appUpdate.checkedRepo);
  const transferring=$derived(downloadBusy($appDownload));
  const busy=$derived(installing||transferring);
  const percent=$derived(downloadPercent($appDownload));
  const phaseLabel=$derived($t($appDownload.phase==='connecting'?'updates.connecting':$appDownload.phase==='verifying'?'updates.verifying':'updates.downloading'));
  onMount(()=>{repo=updateRepository()});
  async function check(){error='';await checkAppUpdate(repo)}
  async function download(){if(!release?.sha256||busy||$appUpdate.busy)return;error='';await appDownload.start({repo:checkedRepo,version:release.version,sha256:release.sha256})}
  async function install(){if(!release?.sha256||busy||!ready)return;installing=true;error='';try{if(!await confirmAction($t('updates.restartConfirm')))return;await distributionCall('install_app_update',{expectedVersion:release.version,expectedSha256:release.sha256})}catch(e){error=String(e);appDownload.reset()}finally{installing=false}}
  const notes=$derived(displayReleaseNotes(release?.notes??'', $t('updates.portableHint')));
</script>
<section class="page-scroll">
  <header class="page-header">
    <p class="current-version"><span>Coding Tools MCP</span> · {$t('updates.current')} {APP_VERSION}</p>
    <h2 class="page-title">{$t('updates.title')}</h2>
    <p>{$t('updates.description')}</p>
  </header>
  <div class="page-body update-page"><div class="tx-card">
    <div class="buttons">
      <button type="button" onclick={check} disabled={busy||$appUpdate.busy||getBackend().capabilities.host==='node'}>{$appUpdate.busy?$t('updates.checking'):$t('updates.check')}</button>
    </div>
    {#if error||$appDownload.error||$appUpdate.error}
      <div role="alert"><p>{$t('updates.failed')}</p><details><summary>{$t('updates.errorDetails')}</summary><pre>{error||$appDownload.error||$appUpdate.error}</pre></details></div>
    {/if}
    {#if release}
      {#if release.available}
        <h3>{$t('updates.newVersion')} {release.version}</h3>
        <p>{$t('updates.available')}</p>
        {#if notes}<section class="release-notes"><h4>{$t('updates.notes')}</h4><pre>{notes}</pre></section>{/if}
        {#if localDesktop()}
          {#if release.assetUrl&&release.sha256}
            {#if ready&&!busy}<p role="status">{$t('updates.ready')}</p>{/if}
            {#if transferring}
              <div class="download-progress">
                <p role="status">{phaseLabel}</p>
                {#if $appDownload.phase==='connecting'||percent===undefined}
                  <progress max="100" aria-label={phaseLabel}></progress>
                {:else}
                  <progress max="100" value={percent} aria-label={phaseLabel}></progress>
                {/if}
                <p class="download-counts">
                  <span>{formatDownloadBytes($appDownload.downloaded)}{#if $appDownload.total!==null} / {formatDownloadBytes($appDownload.total)}{/if}</span>
                  {#if percent!==undefined}<span>{percent}%</span>{/if}
                </p>
              </div>
            {/if}
            <button type="button" onclick={ready?install:download} disabled={busy||$appUpdate.busy}>{installing?$t('updates.installing'):transferring?phaseLabel:$t(ready?'updates.install':'updates.download')}</button>
          {:else}<p>{$t('updates.missingAsset')}</p>{/if}
        {/if}
      {:else}<p class="up-to-date" role="status">{$t('updates.latest')}</p>{/if}
    {/if}
    {#if !localDesktop()}<p>{$t('updates.desktopOnly')}</p>{/if}
    <p>{$t('updates.integrity')}</p>
  </div></div>
</section>
<style>
.download-progress{display:grid;gap:8px}.download-progress progress{width:100%;height:10px;accent-color:var(--color-accent,#6b8afd)}.download-counts{display:flex;justify-content:space-between;font-variant-numeric:tabular-nums}
.update-page{max-width:900px}.tx-card{padding:24px;display:grid;gap:18px}.buttons{display:flex;gap:10px;flex-wrap:wrap}button{border:1px solid var(--color-border);border-radius:7px;padding:10px;cursor:pointer}button:disabled{opacity:.5}p{font-size:13px;line-height:1.7;color:var(--color-text-muted)}pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:40dvh;overflow:auto;font:13px/1.7 inherit}.release-notes{display:grid;gap:8px}.release-notes h4{font-size:13px;font-weight:600}.up-to-date{color:var(--color-text)}[role=alert] p,[role=alert] details{color:#e77}summary{cursor:pointer;font-size:12px}
</style>
