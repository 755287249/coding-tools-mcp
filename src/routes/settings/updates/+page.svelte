<script lang="ts">
  import { confirm as confirmAction } from '$lib/api/native';
  import { t } from '$lib/i18n';
  import { getBackend } from '$lib/backend';
  import { distributionCall, localDesktop, type AppRelease } from '$lib/api/distribution';
  import { onMount } from 'svelte';
  let repo=$state('755287249/coding-tools-mcp'),release=$state<AppRelease|null>(null),busy=$state(false),error=$state(''),ready=$state(false),checkedRepo=$state('');
  onMount(()=>{try{repo=localStorage.getItem('ctmcp-update-repo')||repo}catch{}});
  async function check(){busy=true;error='';ready=false;release=null;try{release=await distributionCall('check_app_update',{repo});checkedRepo=repo;try{localStorage.setItem('ctmcp-update-repo',repo)}catch{}}catch(e){error=String(e)}finally{busy=false}}
  async function download(){if(!release?.sha256)return;busy=true;error='';ready=false;try{await distributionCall('download_app_update',{repo:checkedRepo,expectedVersion:release.version,expectedSha256:release.sha256});ready=true}catch(e){error=String(e)}finally{busy=false}}
  async function install(){if(!release?.sha256||busy)return;busy=true;error='';try{if(!await confirmAction($t('updates.restartConfirm')))return;await distributionCall('install_app_update',{expectedVersion:release.version,expectedSha256:release.sha256})}catch(e){error=String(e);ready=false}finally{busy=false}}
  const releasePage=$derived(/^[-\w.]+\/[-\w.]+$/.test(repo)?`https://github.com/${repo}/releases`:'https://github.com/755287249/coding-tools-mcp/releases');
</script>
<section class="page-scroll"><header class="page-header"><h2 class="page-title">{$t('updates.title')}</h2><p>{$t('updates.description')}</p></header><div class="page-body update-page"><div class="tx-card"><label>{$t('updates.repository')}<input bind:value={repo} oninput={()=>{release=null;ready=false;checkedRepo='';error=''}} disabled={busy} placeholder="owner/repository"/></label><div class="buttons"><button type="button" onclick={check} disabled={busy||getBackend().capabilities.host==='node'}>{busy?$t('Loading…'):$t('updates.check')}</button><button type="button" onclick={()=>getBackend().native.openExternal(releasePage)}>{$t('updates.open')}</button></div>{#if error}<p role="alert">{error}</p>{/if}
{#if release}<h3>{release.currentVersion} → {release.version}</h3><p>{release.available?$t('updates.available'):$t('updates.latest')}</p><pre>{release.notes}</pre>{#if release.available&&localDesktop()}{#if release.assetUrl&&release.sha256}<button type="button" onclick={ready?install:download} disabled={busy}>{ready?$t('updates.install'):$t('updates.download')}</button>{:else}<p>{$t('updates.missingAsset')}</p>{/if}{/if}{/if}
{#if !localDesktop()}<p>{$t('updates.desktopOnly')}</p>{/if}<p>{$t('updates.integrity')}</p></div></div></section>
<style>.update-page{max-width:900px}.tx-card{padding:24px;display:grid;gap:18px}label{display:grid;gap:10px;font-size:13px}input{padding:10px;border:1px solid var(--color-border);border-radius:7px;background:var(--color-bg);min-width:0}.buttons{display:flex;gap:10px;flex-wrap:wrap}button{border:1px solid var(--color-border);border-radius:7px;padding:10px;cursor:pointer}button:disabled{opacity:.5}p{font-size:13px;line-height:1.7;color:var(--color-text-muted)}pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:40dvh;overflow:auto;font:13px/1.7 inherit}[role=alert]{color:#e77}</style>
