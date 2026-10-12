<script lang="ts">
  import {onMount} from 'svelte';
  import {page} from '$app/stores';
  import {goto} from '$app/navigation';
  import {appUrl} from '$lib/app-path';
  import {getMcpConnection,GLOBAL_MCP_ID,getRuntimeStatus,getActionsRuntimeStatus,updateWorkspace,startRuntime,stopRuntime,startActionsRuntime,stopActionsRuntime} from '$lib/api/workspaces';
  import {listFrpProfiles,type FrpProfileDto} from '$lib/api/settings';
  import {getBackend} from '$lib/backend';
  import {mcpConnection} from '$lib/stores/app';
  import {refreshConnectionHealth} from '$lib/stores/connection-health';
  import AutoWorkspaceView from '$lib/components/auto/AutoWorkspaceView.svelte';
  import McpWorkspacePanel,{type ServiceSection} from '$lib/components/workspace/McpWorkspacePanel.svelte';
  import ActionsWorkspacePanel from '$lib/components/workspace/ActionsWorkspacePanel.svelte';
  import type {TunnelFormConfig} from '$lib/components/TunnelConfigForm.svelte';
  import {actionsConfig,type WorkspaceProfile,type RuntimeStatus,type ActionsAuthDraft} from '$lib/types';
  import {t} from '$lib/i18n';
  let error=$state(''),notice=$state(''),busy=$state(false),advanced=$state(false);
  let service=$state<'mcp'|'actions'>('mcp'),section=$state<ServiceSection>('service');
  let mcp=$state<RuntimeStatus|null>(null),actions=$state<RuntimeStatus|null>(null);
  let frpProfiles=$state<FrpProfileDto[]>([]);
  const capabilities=getBackend().capabilities;
  $effect(()=>{if($page.url.searchParams.get('service')==='actions'&&capabilities.actions){service='actions';advanced=true;}});
  async function refresh(){
    await getMcpConnection();
    const [nextMcp,nextActions]=await Promise.all([getRuntimeStatus(GLOBAL_MCP_ID),capabilities.actions?getActionsRuntimeStatus(GLOBAL_MCP_ID):Promise.resolve(null)]);
    mcp=nextMcp;actions=nextActions;
  }
  onMount(()=>{let disposed=false,pending=false;const poll=async()=>{if(pending||disposed)return;pending=true;try{await refresh();}catch(e){if(!disposed)error=String(e);}finally{pending=false;}};
    void poll();const timer=setInterval(()=>{if(!document.hidden)void poll();},10000);
    if(capabilities.frpManagement)void listFrpProfiles().then(values=>frpProfiles=values).catch(e=>error=String(e));
    return()=>{disposed=true;clearInterval(timer);};
  });
  async function save(change:(profile:WorkspaceProfile)=>WorkspaceProfile){
    if(busy)return;busy=true;error='';notice='';
    try{const current=await getMcpConnection();await updateWorkspace(change(current));notice=$t('connection.saved');await refresh();}
    catch(e){error=String(e);throw e;}finally{busy=false;}
  }
  async function toggle(){
    if(busy)return;busy=true;error='';
    try{
      if(service==='mcp'){if(mcp?.state==='running')await stopRuntime(GLOBAL_MCP_ID);else await startRuntime(GLOBAL_MCP_ID);}
      else {if(actions?.state==='running')await stopActionsRuntime(GLOBAL_MCP_ID);else await startActionsRuntime(GLOBAL_MCP_ID);}
      notice='';
    }catch(e){error=String(e);}finally{busy=false;await refresh().catch(e=>error=String(e));void refreshConnectionHealth();}
  }
  function saveTunnel(config:TunnelFormConfig){return save(profile=>service==='mcp'?{...profile,tunnel:{...profile.tunnel,...config}}:{...profile,actions:{...actionsConfig(profile),tunnel_type:config.type,public_url:config.public_url,frp_server:config.frp_server,frp_subdomain:config.frp_subdomain,frp_profile_id:config.frp_profile_id,frp_server_port:config.frp_server_port,cloudflare_mode:config.cloudflare_mode,use_proxy:config.use_proxy}});}
  function saveActionsAuth(draft:ActionsAuthDraft){return save(profile=>({...profile,actions:{...actionsConfig(profile),auth_type:draft.authType,oauth_client_id:draft.oauthClientId,oauth_scopes:draft.oauthScopes,use_shared_secrets:draft.useSharedSecrets}}));}
  // An address already exists: the setup button offers to reconfigure instead.
  const configured=$derived(Boolean(mcp?.publicEndpoint||$mcpConnection?.tunnel?.public_url));
</script>
<section class="page-scroll">
  <header class="page-header connection-header">
    <div class="heading"><h1 class="page-title">{$t('connection.title')}</h1><p class="mt-2 text-sm text-[var(--color-text-muted)]">{$t('connection.description')}</p>
      {#if advanced}<div class="service-tabs" role="tablist" aria-label={$t('connection.title')}><button role="tab" aria-selected={service==='mcp'} class:active={service==='mcp'} onclick={()=>{service='mcp';section='service';}}>MCP</button>{#if capabilities.actions}<button role="tab" aria-selected={service==='actions'} class:active={service==='actions'} onclick={()=>{service='actions';section='service';}}>Actions</button>{/if}</div>{/if}
    </div>
    <div class="header-tools">
      <button class="setup-button" onclick={()=>void goto(appUrl('/quick-setup'))}>{configured?$t('connection.reconfigure'):$t('Quick setup')}</button>
      <div class="mode-pill" role="group" aria-label={$t('connection.viewMode')}><button aria-pressed={!advanced} class:active={!advanced} onclick={()=>{advanced=false;service='mcp';}}>{$t('connection.compact')}</button><button aria-pressed={advanced} class:active={advanced} onclick={()=>advanced=true}>{$t('Advanced')}</button></div>
    </div>
  </header>
  {#if error}<p role="alert" class="tx-alert tx-alert--error m-4">{error}</p>{/if}
  {#if notice}<p role="status" class="tx-info-block m-4">{notice}</p>{/if}
  {#if $mcpConnection}
    {#if service==='mcp'&&!advanced}<AutoWorkspaceView profile={$mcpConnection} onProfileChange={profile=>mcpConnection.set(profile)}/>
    {:else}<div class="page-body">
      {#if service==='mcp'}
        <McpWorkspacePanel workspaceId={GLOBAL_MCP_ID} profile={$mcpConnection} {section} status={mcp?.state??'stopped'} statusMessage={mcp?.localMessage??''} localEndpoint={mcp?.localEndpoint??''} publicEndpoint={mcp?.publicEndpoint??''} {busy} {frpProfiles} onSectionChange={value=>section=value} onToggle={toggle} onPortChange={port=>save(p=>({...p,runtime:{...p.runtime,local_port:port}}))} onBindAddressChange={address=>save(p=>({...p,runtime:{...p.runtime,bind_address:address}}))} onSaveTunnel={saveTunnel} onSaveAuth={auth=>save(p=>({...p,auth}))} onSavePolicy={async()=>{}}/>
      {:else}
        <ActionsWorkspacePanel workspaceId={GLOBAL_MCP_ID} profile={$mcpConnection} {section} status={actions?.state??'stopped'} statusMessage={actions?.localMessage??''} localEndpoint={actions?.localEndpoint??''} publicEndpoint={actions?.publicEndpoint??''} {busy} {frpProfiles} onSectionChange={value=>section=value} onToggle={toggle} onPortChange={port=>save(p=>({...p,actions:{...actionsConfig(p),local_port:port}}))} onBindAddressChange={address=>save(p=>({...p,actions:{...actionsConfig(p),bind_address:address}}))} onSaveTunnel={saveTunnel} onSaveAuth={saveActionsAuth} onSavePolicy={async()=>{}}/>
      {/if}
    </div>{/if}
  {/if}
</section>

<style>
.connection-header{display:flex;align-items:flex-start;justify-content:space-between;gap:16px;flex-wrap:wrap}.heading{min-width:0;flex:1}
.header-tools{display:flex;align-items:center;gap:10px;flex:none}
.setup-button{height:32px;padding:0 14px;border-radius:999px;border:1px solid var(--color-border);background:var(--card-bg);color:var(--color-text);font-size:13px;font-weight:500;cursor:pointer}.setup-button:hover{background:var(--surface-hover)}
.mode-pill{display:inline-flex;padding:3px;border-radius:999px;background:var(--surface-2,var(--color-surface));border:1px solid var(--color-border)}
.mode-pill button,.service-tabs button{height:26px;padding:0 13px;border-radius:999px;border:0;background:transparent;color:var(--color-text-muted);font-size:12.5px;cursor:pointer}
.mode-pill button.active,.service-tabs button.active{background:var(--card-bg);color:var(--color-text);box-shadow:0 1px 3px rgb(0 0 0 / .12)}
.service-tabs{display:inline-flex;gap:2px;margin-top:14px;padding:3px;border-radius:999px;background:var(--surface-2,var(--color-surface));border:1px solid var(--color-border)}
</style>
