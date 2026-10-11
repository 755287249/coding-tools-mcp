<script lang="ts">
  import {seedRequest,seedBundle,type SeedInventory,type SeedTicket} from '$lib/api/seeds';
  import {copyText} from '$lib/browser-tools';
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  import {workspaces} from '$lib/stores/app';
  import {APP_VERSION} from '$lib/app-version';
  import {appUrl} from '$lib/app-path';
  import {untrack} from 'svelte';
  import {seedDrafts,seedDraftKey} from '$lib/chat/seed-drafts';
  let {workspaceId,folderId}:{workspaceId:string;folderId:string}=$props();
  let inventory=$state<SeedInventory>({enabled:false,seeds:[],retired_count:0});
  let account=$state(''),repo=$state(''),branch=$state('main'),count=$state(3),endpoint=$state('');
  let busy=$state(false),error=$state(''),bundle=$state(''),copied=$state(false),history=$state(false),filter=$state('');
  let persisted=$state(true),draftKey=$state('');
  const accounts=$derived([...new Set(inventory.seeds.map(s=>s.account))]);
  const rows=$derived(inventory.seeds.filter(s=>(history||!s.archived)&&(!filter||s.account===filter)));
  let scope=0;
  $effect(()=>{const ws=workspaceId,folder=folderId;const generation=++scope;
    untrack(()=>{
      draftKey=seedDraftKey(ws,folder);const saved=seedDrafts.load(draftKey);
      account=saved.account;repo=saved.repo;branch=saved.branch;count=saved.count;
      const profile=$workspaces.find(w=>w.id===ws);
      endpoint=saved.endpoint||(profile?.tunnel.public_url?profile.tunnel.public_url.replace(/\/$/,'').replace(/\/mcp$/,'')+'/mcp':'');
      bundle=seedDrafts.batch(draftKey);copied=false;error='';filter='';inventory={enabled:false,seeds:[],retired_count:0};
    });
    let stopped=false,running=false;
    const refresh=async()=>{if(running||stopped)return;running=true;try{const result=await seedRequest(ws,folder,{action:'seed_list'});if(!stopped&&scope===generation)inventory=result;}catch(e){if(!stopped&&scope===generation)error=String(e);}finally{running=false;}};
    void refresh();const timer=setInterval(refresh,5000);return()=>{stopped=true;clearInterval(timer);};
  });
  async function action(args:Record<string,unknown>){if(busy)return;busy=true;error='';const generation=scope;try{const result=await seedRequest(workspaceId,folderId,args);if(generation===scope)inventory=result;}catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  function saveDraft(){if(draftKey===seedDraftKey(workspaceId,folderId))persisted=seedDrafts.save(draftKey,{account,repo,branch,count,endpoint});}
  function clearBundle(){seedDrafts.setBatch(draftKey,'');bundle='';copied=false;}
  async function prepare(){if(busy)return;busy=true;error='';copied=false;saveDraft();const generation=scope,ws=workspaceId,folder=folderId,key=draftKey,url=endpoint,request={account,repo_id:repo.trim(),branch:branch.trim(),count};try{
    seedBundle(url,folder,[]);
    const fresh=await seedRequest(ws,folder,{action:'seed_list'});
    if(generation!==scope)return;
    const mismatched=fresh.seeds.filter(s=>s.repo_id!==request.repo_id||s.branch!==request.branch);
    if(mismatched.some(s=>s.last_seen||s.host_task_id||s.chat_id||s.unresolved_operations))throw Error($t('seeds.bound'));
    const replace=mismatched.some(s=>!s.retired_at);
    if(replace&&!window.confirm($t('seeds.correctConfirm')))return;
    const result=await seedRequest<{batch:SeedTicket[]}>(ws,folder,{action:'seed_batch',...request,replace_unconnected:replace});
    const text=seedBundle(url,folder,result.batch);seedDrafts.setBatch(key,text);
    if(generation===scope){bundle=text;const next=await seedRequest(ws,folder,{action:'seed_list'});if(generation===scope)inventory=next;}
  }catch(e){if(generation===scope)error=String(e);}finally{busy=false;}}
  async function copy(){try{await copyText(bundle);copied=true;}catch(e){error=String(e);}}
</script>
<div class="seed-library">
  <div class="overview">
    <div><span>{$t('seeds.ready')}</span><strong>{inventory.seeds.filter(s=>s.status==='ready').length}</strong></div>
    <div><span>{$t('seeds.assigned')}</span><strong>{inventory.seeds.filter(s=>s.status==='assigned'||s.status==='working').length}</strong></div>
    <div><span>{$t('seeds.offline')}</span><strong>{inventory.seeds.filter(s=>s.status==='offline').length}</strong></div>
    <div><span>{$t('seeds.accounts')}</span><strong>{accounts.length}</strong></div>
    <div><span>{$t('seeds.retiredTotal')}</span><strong>{inventory.retired_count}</strong></div>
  </div>
  <div class="toolbar"><label><input type="checkbox" checked={inventory.enabled} disabled={busy} onchange={e=>action({action:'seed_settings',enabled:e.currentTarget.checked})}/>{$t('seeds.auto')}</label><button disabled={busy} onclick={()=>action({action:'seed_cleanup'})}>{$t('seeds.cleanup')}</button></div>
  <p class="notice">{$t('seeds.notice')}</p>
  {#if error}<p role="alert">{error}</p>{/if}
  <details open><summary>{$t('seeds.add')}</summary>
    <p class="notice">{$t(persisted?'seeds.saved':'seeds.saveUnavailable')}</p>
    <form oninput={saveDraft} onsubmit={e=>{e.preventDefault();void prepare();}}>
      <label>{$t('seeds.account')}<input bind:value={account} maxlength="200" required/></label>
      <label>{$t('seeds.repo')}<input bind:value={repo} maxlength="200" required/></label>
      <label>{$t('seeds.branch')}<input bind:value={branch} maxlength="200" required/></label>
      <label>{$t('seeds.count')}<input type="number" bind:value={count} min="1" max="50" required/></label>
      <label class="wide">{$t('seeds.endpoint')}<input type="url" bind:value={endpoint} placeholder="https://example.com/mcp" required/></label>
      <div class="wide toolbar"><button class="primary" disabled={busy}>{$t('seeds.prepare')}</button><a href={`https://github.com/755287249/coding-tools-mcp/releases/download/v${APP_VERSION}/coderabbit-seeds.user.js`} target="_blank" rel="noreferrer">{$t('seeds.install')}</a></div>
    </form>
    {#if bundle}<section class="bundle"><p>{$t('seeds.bundleHint')}</p><textarea readonly value={bundle} rows="4" aria-label={$t('seeds.batch')}></textarea><div class="toolbar"><button onclick={copy}>{$t(copied?'seeds.copied':'seeds.copy')}</button><button onclick={clearBundle}>{$t('seeds.clear')}</button></div></section>{/if}
  </details>
  <div class="toolbar filters"><select aria-label={$t('seeds.account')} bind:value={filter}><option value="">{$t('seeds.allAccounts')}</option>{#each accounts as a}<option value={a}>{a}</option>{/each}</select><label><input type="checkbox" bind:checked={history}/>{$t('seeds.history')}</label></div>
  <div class="seed-list">{#each rows as seed(seed.id)}<article>
    <div class="seed-title"><strong>{seed.account}</strong><span class:alive={seed.status==='ready'||seed.status==='assigned'}>{$t(`seeds.${seed.status}` as MessageKey)}</span></div>
    <p class="identity">{seed.id.slice(0,8)} · {seed.repo_id} · {seed.branch}</p>
    <p class="time">{$t('seeds.lastSeen')}: {seed.last_seen?new Date(seed.last_seen).toLocaleString():$t('seeds.never')}</p>
    {#if seed.reason}<p class="notice">{$t(`seeds.reason.${seed.reason}` as MessageKey)}</p>{/if}
    {#if seed.unresolved_operations}<p role="status">{$t('seeds.unresolved')} ({seed.unresolved_operations})</p>{/if}
    <div class="toolbar">{#if seed.chat_id}<a href={appUrl(`/workspace/${encodeURIComponent(workspaceId)}?folder=${encodeURIComponent(folderId)}&chat=${encodeURIComponent(seed.chat_id)}`)}>{$t('seeds.conversation')}</a>{/if}
      {#if seed.host_task_id}<a href={`https://app.coderabbit.ai/code/tasks/${encodeURIComponent(seed.host_task_id)}`} target="_blank" rel="noreferrer">{$t('seeds.hostTask')}</a>{/if}
      {#if !seed.retired_at}<button disabled={busy||seed.unresolved_operations>0} onclick={()=>action({action:'seed_retire',seed_id:seed.id})}>{$t('seeds.retire')}</button>{/if}
    </div>
  </article>{:else}<div class="empty">{$t('seeds.empty')}</div>{/each}</div>
</div>
<style>
.seed-library{max-width:1000px;margin:auto}.overview{display:grid;grid-template-columns:repeat(5,1fr);gap:12px;margin-bottom:20px}.overview>div{border:1px solid var(--color-border);border-radius:12px;padding:16px;display:grid;gap:8px;background:var(--card-bg)}.overview span,.notice,.time,.identity{font-size:12px;color:var(--color-text-muted);line-height:1.7}.overview strong{font-size:28px}.toolbar{display:flex;gap:12px;align-items:center;flex-wrap:wrap}.toolbar label{display:flex;align-items:center;gap:8px}.notice{margin:12px 0}details{border:1px solid var(--color-border);padding:16px;border-radius:12px;margin:18px 0}summary{cursor:pointer;font-weight:600}form{display:grid;grid-template-columns:repeat(2,1fr);gap:14px;margin-top:18px}form label{display:grid;gap:6px;font-size:12px}.wide{grid-column:1/-1}input:not([type=checkbox]),select,textarea{padding:10px;border:1px solid var(--color-border);border-radius:8px;background:var(--card-bg);min-width:0;width:100%}select{width:auto}button{padding:8px 14px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer}button:disabled{opacity:.5;cursor:default}.primary{background:#2563eb;color:white}.bundle{margin-top:20px}.bundle p{font-size:12px;margin-bottom:10px}.bundle textarea{font-family:monospace;word-break:break-all;margin-bottom:10px}.filters{margin:20px 0}.seed-list{display:grid;grid-template-columns:repeat(2,1fr);gap:14px}article{padding:18px;border:1px solid var(--color-border);border-radius:12px;background:var(--card-bg)}.seed-title{display:flex;justify-content:space-between;gap:12px}.seed-title span{font-size:11px;border-radius:20px;padding:3px 10px;background:var(--color-border)}.seed-title .alive{background:#16a34a20;color:#16a34a}article .toolbar{margin-top:12px;font-size:12px}.empty{padding:36px;color:var(--color-text-muted);grid-column:1/-1;text-align:center}[role=alert]{color:var(--danger);overflow-wrap:anywhere}[role=status]{font-size:12px;color:#d97706}a{color:#3b82f6;cursor:pointer}@media(max-width:700px){.overview{grid-template-columns:repeat(2,1fr)}.seed-list,form{grid-template-columns:1fr}.overview strong{font-size:23px}}
</style>
