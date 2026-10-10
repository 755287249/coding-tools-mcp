<script lang="ts">
  import { onMount } from 'svelte';
  import { confirm as confirmAction } from '$lib/api/native';
  import { t } from '$lib/i18n';
  import { appUrl } from '$lib/app-path';
  import { distributionCall, localDesktop, type ShareStatus } from '$lib/api/distribution';
  import { listWorkspaces, getRuntimeStatus, startRuntime, restartRuntime, updateWorkspace } from '$lib/api/workspaces';
  import { restoreSharingSettings } from '$lib/sharing-settings';
  import { prepareSharingRuntime } from '$lib/sharing-runtime';
  import { startTunnel } from '$lib/api/tunnel';
  import type { WorkspaceProfile } from '$lib/types';
  import { copyText } from '$lib/browser-tools';
  let status = $state<ShareStatus>({enabled:false,origins:[],lanIp:null});
  let loaded=$state(false), loading=$state(true), password=$state(''), reveal=$state(false);
  let profiles=$state<WorkspaceProfile[]>([]), id=$state(''), publicUrl=$state(''), lan=$state(false), busy=$state(false), error=$state(''), copied=$state('');
  const profile=$derived(profiles.find(p=>p.id===id));
  async function inspect() {
    if(!id)return;
    try { const runtime=await getRuntimeStatus(id); const url=runtime.publicEndpoint;
      publicUrl=url ? new URL(url).origin : ''; }catch(e){error=String(e)}
  }
  async function load() {
    loading=true;error='';
    try {
      status=await distributionCall('browser_sharing_status');
      password=status.password??'';
      profiles=await listWorkspaces();
      const restored=restoreSharingSettings(status,profiles);
      id=restored.id;lan=restored.lan;publicUrl=restored.publicUrl;
      if(!status.enabled)await inspect();
      loaded=true;
    }catch(e){error=String(e)}finally{loading=false}
  }
  onMount(()=>{if(localDesktop())void load()});
  const runtimeActions = {
    getStatus: getRuntimeStatus, start: startRuntime, restart: restartRuntime,
    save: async (updated: WorkspaceProfile) => {
      await updateWorkspace(updated);
      profiles=profiles.map(p=>p.id===updated.id?updated:p);
    },
    confirmLan: () => confirmAction($t('sharing.lanConfirm')),
  };
  async function enable() {
    if(!profile||busy)return;busy=true;error='';
    try {
      const selected=await prepareSharingRuntime(profile,lan,runtimeActions);
      if(!selected)return;
      const origins=[`http://127.0.0.1:${selected.runtime.local_port}`];
      if(lan&&status.lanIp)origins.push(`http://${status.lanIp}:${selected.runtime.local_port}`);
      if(publicUrl.trim())origins.push(publicUrl.trim());
      status=await distributionCall('configure_browser_sharing',{enabled:true,origins,...(password?{password}:{})});password=status.password??'';
    }catch(e){error=String(e)}finally{busy=false}
  }
  async function disable(){busy=true;error='';try{status=await distributionCall('configure_browser_sharing',{enabled:false,origins:[]})}catch(e){error=String(e)}finally{busy=false}}
  async function tunnel(){if(!profile||busy)return;busy=true;error='';try{await prepareSharingRuntime(profile,false,runtimeActions);const result=await startTunnel(id,'mcp');publicUrl=result.publicUrl?new URL(result.publicUrl).origin:'';if(!publicUrl)throw Error($t('sharing.tunnelPending'));}catch(e){error=String(e)}finally{busy=false}}
  async function savePassword(){busy=true;error='';try{status=await distributionCall('configure_browser_sharing',{enabled:status.enabled,origins:status.origins,password});password=status.password??'';}catch(e){error=String(e)}finally{busy=false}}
  async function copy(value:string){try{await copyText(value);copied=value}catch(e){error=String(e)}}
</script>
<section class="page-scroll"><header class="page-header"><h2 class="page-title">{$t('sharing.title')}</h2><p>{$t('sharing.description')}</p></header><div class="page-body sharing-page">
{#if !localDesktop()}<p class="tx-card">{$t('sharing.desktopOnly')}</p>{:else}
{#if error}<p role="alert">{error}</p>{/if}
{#if loading}<p role="status">{$t("Loading…")}</p>{:else if !loaded}<button type="button" onclick={load}>{$t("shell.resume")}</button>{:else}
<details class="tx-card controls"><summary>{$t('sharing.connectionSettings')}</summary><label>{$t('chat.project')}<select bind:value={id} onchange={inspect} disabled={busy}>{#each profiles as p}<option value={p.id}>{p.name}</option>{/each}</select></label>
<label><input type="checkbox" bind:checked={lan} disabled={busy}/>{$t('sharing.lan')}</label>
<label>{$t('sharing.publicUrl')}<input type="url" bind:value={publicUrl} placeholder="https://your-domain.example" disabled={busy}/></label>
<p>{$t('sharing.routeHint')}</p><div class="buttons"><button type="button" onclick={tunnel} disabled={busy||!id}>{$t('sharing.useTunnel')}</button><a href={appUrl(`/quick-setup?workspace=${encodeURIComponent(id)}`)}>{$t('sharing.configureTunnel')}</a></div>
<p>{$t('sharing.permissionHint')}</p><div class="buttons"><button type="button" onclick={enable} disabled={busy||!id}>{status.enabled?$t('Save'):$t('sharing.enable')}</button><button type="button" onclick={disable} disabled={busy||!status.enabled}>{$t('sharing.disable')}</button></div></details>
<div class="tx-card"><h3>{status.enabled?$t('sharing.enabled'):$t('sharing.disabled')}</h3>{#if !status.enabled}<button type="button" onclick={enable} disabled={busy||!id}>{$t('sharing.enable')}</button>{/if}<label>{$t('sharing.password')}<input type={reveal?'text':'password'} bind:value={password} autocomplete="new-password" disabled={busy} placeholder={$t('sharing.customPassword')}/><button type="button" onclick={()=>reveal=!reveal}>{$t(reveal?'sharing.hidePassword':'sharing.showPassword')}</button><button type="button" disabled={!password} onclick={()=>copy(password)}>{$t('Copy')}</button></label><p>{$t('sharing.passwordHint')}</p><button type="button" disabled={busy||!password} onclick={savePassword}>{$t('sharing.savePassword')}</button>
{#each status.origins as origin}<div class="share-link"><input readonly value={origin+'/'}/><button type="button" onclick={()=>copy(origin+'/')}>{copied===origin+'/'?$t('Copied'):$t('Copy')}</button></div>{/each}
<p>{$t('sharing.lifetime')}</p><p>{$t('sharing.networkHint')}</p></div>
{/if}{/if}</div></section>
<style>.sharing-page{display:grid;gap:20px;max-width:900px}.tx-card{padding:20px;min-width:0}.controls{order:2}.controls[open]{display:grid;gap:15px}summary{cursor:pointer;font-size:13px}.controls:not([open])>*:not(summary){display:none}label{display:flex;flex-wrap:wrap;align-items:center;gap:10px;font-size:13px}label:has(input[type=url]),label:has(select){display:grid}input:not([type=checkbox]),select{padding:10px;border:1px solid var(--color-border);border-radius:7px;background:var(--color-bg);min-width:0;max-width:100%}.buttons,.share-link{display:flex;gap:10px;flex-wrap:wrap;align-items:center}.share-link{margin:12px 0}.share-link input{flex:1}button,a{border:1px solid var(--color-border);border-radius:7px;padding:10px;cursor:pointer}button:disabled{opacity:.5}p{font-size:13px;line-height:1.7;color:var(--color-text-muted);overflow-wrap:anywhere}h3{margin-bottom:14px}[role=alert]{color:#e77}</style>
