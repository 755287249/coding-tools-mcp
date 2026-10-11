<script lang="ts">
  import ManualSeed from './ManualSeed.svelte';
  import {seedRequest,type SeedInventory,type SeedRow} from '$lib/api/seeds';
  import {copyText} from '$lib/browser-tools';
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  import {workspaces} from '$lib/stores/app';
  import {APP_VERSION} from '$lib/app-version';
  import {appUrl} from '$lib/app-path';
  import {untrack} from 'svelte';
  import {pairingKey,pairingAddress,loadPairing,savePairing,newPairingToken} from '$lib/chat/seed-pairing';
  let {workspaceId,folderId}:{workspaceId:string;folderId:string}=$props();
  const empty=():SeedInventory=>({enabled:false,seeds:[],retired_count:0});
  let inventory=$state<SeedInventory>(empty()),address=$state(''),token=$state('');
  let busy=$state(false),error=$state(''),notice=$state(''),history=$state(false),filter=$state(''),status=$state(''),search=$state(''),page=$state(0),sourceFilter=$state(''),keyword=$state('');
  let pairingOpen=$state(true),showToken=$state(false),selectedAccount=$state(''),loading=$state(true);
  let scope=0;
  const accounts=$derived(inventory.accounts??[]),counts=$derived(inventory.counts??{});
  const pageCount=$derived(Math.max(1,Math.ceil((inventory.filtered_total??0)/50)));
  $effect(()=>{const key=pairingKey(workspaceId,folderId);untrack(()=>{
    const saved=loadPairing(key),profile=$workspaces.find(w=>w.id===workspaceId);
    address=saved.address;token=saved.token;
    if(!address&&profile?.tunnel.public_url){try{address=pairingAddress(profile.tunnel.public_url.replace(/\/$/,'').replace(/\/mcp$/,'')+'/mcp',folderId);}catch{}}
    pairingOpen=true;showToken=false;selectedAccount='';
    inventory=empty();error='';notice='';history=false;filter='';status='';search='';page=0;sourceFilter='';keyword='';
  });});
  $effect(()=>{const ws=workspaceId,folder=folderId,query={action:'seed_list',limit:50,offset:page*50,account:selectedAccount||filter,status,search,source:sourceFilter,keyword,include_archived:history};const generation=++scope;loading=true;
    let stopped=false,running=false;
    const refresh=async()=>{if(running||stopped)return;running=true;try{const result=await seedRequest(ws,folder,query);if(!stopped&&scope===generation){inventory=result;error='';const last=Math.max(0,Math.ceil((result.filtered_total??0)/50)-1);if(page>last)page=last;}}catch(e){if(!stopped&&scope===generation)error=String(e);}finally{running=false;if(!stopped&&scope===generation)loading=false;}};
    void refresh();const timer=setInterval(()=>{if(!document.hidden)void refresh();},5000);return()=>{stopped=true;clearInterval(timer);};
  });
  function query(){return {limit:50,offset:page*50,account:selectedAccount||filter,status,search,source:sourceFilter,keyword,include_archived:history};}
  async function action(args:Record<string,unknown>){if(busy)return;busy=true;error='';const generation=scope;try{const result=await seedRequest(workspaceId,folderId,{...query(),...args});if(generation===scope)inventory=result;}catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function connect(){if(busy)return;busy=true;error='';notice='';const generation=scope,ws=workspaceId,folder=folderId;try{
    const saved={address:pairingAddress(address,folder),token:token.trim()};
    if(!/^[a-f0-9]{64}$/.test(saved.token))throw Error($t('seeds.tokenInvalid'));
    const result=await seedRequest(ws,folder,{...query(),action:'seed_pairing',token:saved.token});
    const stored=savePairing(pairingKey(ws,folder),saved);
    if(generation===scope){address=saved.address;token=saved.token;inventory=result;notice=$t(stored?'seeds.pairSaved':'seeds.pairStorage');}
  }catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function copy(value:string){try{await copyText(value);notice=$t('seeds.copied');}catch(e){error=String(e);}}
  const date=(value:number)=>value?new Date(value).toLocaleString():$t('seeds.never');
  function details(seed:SeedRow){return [seed.account,`${$t('seeds.source')}: ${seed.source}`,`${$t('seeds.prefix')}: ${seed.prefix||'—'}`,seed.metadata?.repo_name||seed.repo_id,seed.branch,`ID: ${seed.id}`,...Object.entries(seed.metadata??{}).filter(([,v])=>v).map(([k,v])=>`${k}: ${v}`),`${$t('seeds.created')}: ${date(seed.created_at)}`,`${$t('seeds.lastSeen')}: ${date(seed.last_seen)}`,`${$t('seeds.hostTask')}: ${seed.host_task_id||'—'}`,`${$t('seeds.conversation')}: ${seed.chat_id||'—'}`,`${$t('seeds.generation')}: ${seed.generation}`,`${$t('seeds.unresolved')}: ${seed.unresolved_operations}`,seed.reason?$t(`seeds.reason.${seed.reason}` as MessageKey):''].filter(Boolean).join('\n');}
</script>
<div class="seed-library">
  <div class="overview">
    <div><span>{$t('seeds.ready')}</span><strong>{counts.ready??0}</strong></div>
    <div><span>{$t('seeds.assigned')}</span><strong>{(counts.assigned??0)+(counts.working??0)}</strong></div>
    <div><span>{$t('seeds.preparing')}</span><strong>{counts.preparing??0}</strong></div>
    <div><span>{$t('seeds.offline')}</span><strong>{counts.offline??0}</strong></div>
    <div><span>{$t('seeds.total')}</span><strong>{inventory.total??0}<small> / {inventory.capacity??10000}</small></strong></div>
  </div>
  <div class="toolbar"><label><input type="checkbox" checked={inventory.enabled} disabled={busy} onchange={e=>action({action:'seed_settings',enabled:e.currentTarget.checked})}/>{$t('seeds.auto')}</label><button disabled={busy} onclick={()=>action({action:'seed_cleanup'})}>{$t('seeds.cleanup')}</button><span class="muted">{$t('seeds.retiredTotal')}: {inventory.retired_count}</span></div>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  <details class="pairing" bind:open={pairingOpen}>
    <summary>{$t('seeds.browserPair')} <span class:connected={inventory.pairing_enabled}>{$t(inventory.pairing_enabled?'seeds.pairEnabled':'seeds.pairDisabled')}</span></summary>
    <p class="notice">{$t('seeds.pairHelp')}</p>
    <form onsubmit={e=>{e.preventDefault();void connect();}}>
      <label>{$t('seeds.pairAddress')}<div class="input-row"><input bind:value={address} type="url" required placeholder="https://example.com/mcp"/><button type="button" onclick={()=>copy(address)}>{$t('seeds.copy')}</button></div></label>
      <label>{$t('seeds.pairToken')}<div class="input-row"><input bind:value={token} type={showToken?'text':'password'} autocomplete="off" maxlength="64" required/><button type="button" aria-pressed={showToken} onclick={()=>showToken=!showToken}>{$t(showToken?'sharing.hidePassword':'sharing.showPassword')}</button><button type="button" onclick={()=>copy(token)}>{$t('seeds.copy')}</button><button type="button" disabled={busy} onclick={()=>{token=newPairingToken();notice=$t('seeds.tokenUnsaved');}}>{$t('seeds.generateToken')}</button></div></label>
      <div class="toolbar"><button class="primary" disabled={busy}>{$t('seeds.savePair')}</button><button type="button" disabled={busy||!inventory.pairing_enabled} onclick={()=>action({action:'seed_pairing',enabled:false})}>{$t('seeds.revokePair')}</button><a href={`https://github.com/755287249/coding-tools-mcp/releases/download/v${APP_VERSION}/coderabbit-seeds.user.js`} target="_blank" rel="noreferrer">{$t('seeds.install')}</a></div>
    </form>
  </details>
  <ManualSeed {workspaceId} {folderId} {address} onCreated={()=>void action({action:'seed_list'})}/>
  <div class="toolbar filters">
    <input class="search" type="search" aria-label={$t('seeds.search')} placeholder={$t('seeds.search')} bind:value={search} oninput={()=>page=0}/>
    <select aria-label={$t('seeds.source')} bind:value={sourceFilter} onchange={()=>page=0}><option value="">{$t('seeds.allSources')}</option>{#each inventory.sources??[] as source}<option value={source}>{source}</option>{/each}</select>
    <input aria-label={$t('seeds.keyword')} placeholder={$t('seeds.keyword')} bind:value={keyword} oninput={()=>page=0}/>
    <select aria-label={$t('seeds.account')} bind:value={filter} onchange={()=>{page=0;selectedAccount='';}}><option value="">{$t('seeds.allAccounts')}</option>{#each accounts as a}<option value={a}>{a}</option>{/each}</select>
    <select aria-label={$t('seeds.allStates')} bind:value={status} onchange={()=>page=0}><option value="">{$t('seeds.allStates')}</option>{#each ['ready','assigned','working','preparing','offline','retired'] as state}<option value={state}>{$t(`seeds.${state}` as MessageKey)}</option>{/each}</select>
    <label><input type="checkbox" bind:checked={history} onchange={()=>page=0}/>{$t('seeds.history')}</label>
  </div>
  {#if selectedAccount}
    <div class="toolbar account-heading"><button onclick={()=>{selectedAccount='';page=0;}}>{$t('seeds.backAccounts')}</button><h2>{selectedAccount}</h2></div>
  {/if}
  <div class="list-head muted"><span>{$t(selectedAccount?'seeds.hoverDetails':'seeds.accountSummaryHelp')}</span><span>{inventory.filtered_total??0} {$t('seeds.items')}</span></div>
  {#if loading}
    <div class="empty" role="status">{$t('seeds.inventoryLoading')}</div>
  {:else if !selectedAccount}
    <div class="account-grid">
      {#each inventory.account_summaries??[] as account(account.account)}
        <article class="account-card">
          <header><span class="dot" class:alive={account.valid>0}></span><h2 title={account.account}>{account.account}</h2></header>
          <div class="account-ratio"><strong>{account.valid}</strong><span>/ {account.total}</span></div>
          <p class="ratio-label" title={$t('seeds.validHelp')}>{$t('seeds.validTotal')}</p>
          <dl class="account-stats">
            <div><dt>{$t('seeds.ready')}</dt><dd>{account.counts.ready??0}</dd></div>
            <div><dt>{$t('seeds.assigned')}</dt><dd>{(account.counts.assigned??0)+(account.counts.working??0)}</dd></div>
            <div><dt>{$t('seeds.preparing')}</dt><dd>{account.counts.preparing??0}</dd></div>
            <div><dt>{$t('seeds.offline')}</dt><dd>{account.counts.offline??0}</dd></div>
            <div><dt>{$t('seeds.retired')}</dt><dd>{account.counts.retired??0}</dd></div>
          </dl>
          <button class="account-manage" onclick={()=>{selectedAccount=account.account;page=0;}}>{$t('seeds.manageAccount')}</button>
        </article>
      {:else}<div class="empty">{$t('seeds.noMatches')}</div>{/each}
    </div>
  {:else}

  <div class="seed-list">{#each inventory.seeds as seed(seed.id)}
    <details class="seed-row">
      <summary title={details(seed)}><span class="dot" class:alive={seed.status==='ready'||seed.status==='assigned'||seed.status==='working'}></span><strong>{seed.account}</strong><span class="identity">{seed.source} · {seed.prefix||seed.metadata?.repo_name||seed.repo_id||$t('seeds.unlabelled')}</span><code>{seed.id.slice(0,8)}</code><span class="badge">{$t(`seeds.${seed.status}` as MessageKey)}</span></summary>
      <div class="row-details"><pre>{details(seed)}</pre>
        <form class="row-labels" onsubmit={e=>{e.preventDefault();const fields=new FormData(e.currentTarget);void action({action:'seed_label',seed_id:seed.id,source:fields.get('source'),prefix:fields.get('prefix')});}}>
          <label>{$t('seeds.source')}<input name="source" required value={seed.source} maxlength="200"/></label>
          <label>{$t('seeds.prefix')}<input name="prefix" value={seed.prefix} maxlength="200"/></label>
          <button disabled={busy}>{$t('seeds.saveLabel')}</button>
        </form><div class="toolbar">
        {#if seed.chat_id}<a href={appUrl(`/workspace/${encodeURIComponent(workspaceId)}?folder=${encodeURIComponent(folderId)}&chat=${encodeURIComponent(seed.chat_id)}`)}>{$t('seeds.conversation')}</a>{/if}
        {#if seed.host_task_id}<a href={`https://app.coderabbit.ai/code/tasks/${encodeURIComponent(seed.host_task_id)}`} target="_blank" rel="noreferrer">{$t('seeds.hostTask')}</a>{/if}
        {#if !seed.retired_at}<button disabled={busy||seed.unresolved_operations>0} onclick={()=>action({action:'seed_retire',seed_id:seed.id})}>{$t('seeds.retire')}</button>{/if}
      </div></div>
    </details>
  {:else}<div class="empty">{$t('seeds.empty')}</div>{/each}</div>
  <div class="toolbar pagination"><button disabled={page===0} onclick={()=>page--}>{$t('seeds.previous')}</button><span>{page+1} / {pageCount}</span><button disabled={page+1>=pageCount} onclick={()=>page++}>{$t('seeds.next')}</button></div>
  {/if}
</div>
<style>
.account-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,270px),1fr));gap:14px}.account-card{border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg);padding:20px;min-width:0}.account-card header{display:flex;align-items:center;gap:9px}.account-card h2,.account-heading h2{font-size:14px;margin:0;overflow-wrap:anywhere}.account-card h2{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.account-ratio{display:flex;align-items:baseline;gap:8px;margin-top:22px;font-variant-numeric:tabular-nums}.account-ratio strong{font-size:34px;line-height:1.2}.account-ratio span{font-size:20px;color:var(--color-text-muted)}.ratio-label{font-size:12px;color:var(--color-text-muted);margin:6px 0 18px}.account-stats{display:grid;grid-template-columns:repeat(3,1fr);gap:14px 10px;border-top:1px solid var(--color-border);padding-top:16px;margin:0 0 18px}.account-stats dt{font-size:11px;color:var(--color-text-muted)}.account-stats dd{font-size:15px;font-weight:600;margin:4px 0 0;font-variant-numeric:tabular-nums}.account-manage{width:100%;font-size:12px}.account-heading{margin:18px 0 12px}button:focus-visible{outline:2px solid #6366f1;outline-offset:3px}
.seed-library{max-width:1180px;margin:auto}.overview{display:grid;grid-template-columns:repeat(5,1fr);gap:12px;margin-bottom:20px}.overview>div{border:1px solid var(--color-border);border-radius:12px;padding:16px;display:grid;gap:8px;background:var(--card-bg)}.overview span,.muted,.notice,.identity{font-size:12px;color:var(--color-text-muted);line-height:1.7}.overview strong{font-size:27px}.overview small{font-size:12px;font-weight:400}.toolbar,.input-row{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.toolbar label{display:flex;align-items:center;gap:6px}.notice{margin:12px 0}.pairing{border:1px solid var(--color-border);padding:16px;border-radius:12px;margin:18px 0}.pairing summary{display:flex;justify-content:space-between}.pairing summary span{font-size:12px;color:var(--color-text-muted)}.pairing summary span.connected{color:#16a34a}summary{cursor:pointer}form{display:grid;gap:14px;margin-top:16px}form label{display:grid;gap:6px;font-size:12px}.input-row input{flex:1;min-width:160px}input:not([type=checkbox]),select{padding:9px 10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);min-width:0}button{padding:8px 12px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer;white-space:nowrap}button:disabled{opacity:.5;cursor:default}.primary{background:#2563eb;color:white}.filters{margin:18px 0}.search{flex:1;min-width:180px!important}.list-head{display:flex;justify-content:space-between;margin:8px 0}.seed-list{border:1px solid var(--color-border);border-radius:12px;overflow:hidden}.seed-row+ .seed-row{border-top:1px solid var(--color-border)}.seed-row summary{display:flex;align-items:center;gap:12px;padding:13px 16px;background:var(--card-bg)}.seed-row summary:hover{background:var(--color-border)}.seed-row strong{font-size:13px;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.identity{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dot{width:7px;height:7px;border-radius:50%;background:#94a3b8;flex-shrink:0}.dot.alive{background:#22c55e}.badge{font-size:11px;border-radius:12px;padding:3px 8px;background:var(--color-border);white-space:nowrap}code{font-size:11px;color:var(--color-text-muted)}.row-labels{display:flex;align-items:end;flex-wrap:wrap;margin-bottom:14px}.row-details{padding:0 16px 16px}.row-details pre{font:12px/1.8 system-ui;white-space:pre-wrap;overflow-wrap:anywhere;margin:12px 0;color:var(--color-text-muted)}.empty{padding:36px;color:var(--color-text-muted);text-align:center}.pagination{justify-content:center;margin:18px 0;font-size:12px}[role=alert]{color:var(--danger);overflow-wrap:anywhere}a{color:#3b82f6;cursor:pointer;font-size:12px}@media(max-width:700px){.overview{grid-template-columns:repeat(2,1fr)}.overview strong{font-size:23px}.seed-row summary{gap:7px;padding:12px 8px}.seed-row code{display:none}.seed-row strong{max-width:110px}.input-row button{padding:8px}.identity{font-size:11px}}
</style>
