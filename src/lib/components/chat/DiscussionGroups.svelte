<script lang="ts">
  import {onMount} from 'svelte';
  import {goto} from '$app/navigation';
  import {appUrl} from '$lib/app-path';
  import {chatLocation} from '$lib/chat/location';
  import {randomId} from '$lib/browser-tools';
  import {localChat,localDiscussion,type ChatSession,type Discussion,type DiscussionAction} from '$lib/api/chat';
  import {t} from '$lib/i18n';
  import type {MessageKey} from '$lib/i18n/catalog';
  let {workspaceId,folderId}:{workspaceId:string;folderId:string}=$props();
  let groups=$state<Discussion[]>([]),sessions=$state<ChatSession[]>([]),selected=$state(''),current=$state<Discussion>();
  let error=$state(''),busy=$state(false),loading=$state(true),editing=$state(false),creating=$state(false),showArchived=$state(false);
  let name=$state(''),goal=$state(''),members=$state<string[]>([]),text=$state(''),purpose=$state('discussion'),targets=$state<string[]>([]),pages=$state(1);
  let pending=$state<DiscussionAction|null>(null),createId='';let alive=true;
  const choices=$derived(sessions.filter(s=>(!s.closed&&!s.archived&&s.mode!=='group')||members.includes(s.id)));
  const missingMembers=$derived(members.filter(id=>!sessions.some(s=>s.id===id)));
  const visible=$derived(groups.filter(g=>showArchived||!g.archived));
  const states:Record<string,MessageKey>={undelivered:'discuss.undelivered',queued:'discuss.queued',processing:'discuss.processing',completed:'discuss.completed',awaiting_user:'discuss.awaiting',closed:'discuss.closed',unavailable:'discuss.unavailable'};
  const call=(args:DiscussionAction)=>localDiscussion(workspaceId,folderId,args);
  async function read(id:string){
    const result=await call({action:'discussion_read',discussion_id:id});let next=result.discussion;
    for(let page=1;next?.next_offset!=null&&page<pages;page++){
      const older=(await call({action:'discussion_read',discussion_id:id,offset:next.next_offset})).discussion;
      if(!older)break;next={...next,posts:[...(older.posts??[]),...(next.posts??[])],next_offset:older.next_offset};
    }
    if(alive&&selected===id){current=next;targets=targets.filter(id=>next?.members.includes(id));}
  }
  async function refresh(){
    const [list,chats]=await Promise.all([call({action:'discussion_list'}),localChat(workspaceId,folderId,{action:'list'})]);
    if(!alive)return;groups=list.discussions??[];sessions=chats.sessions??[];
    if(!selected&&!editing)selected=groups.find(g=>!g.archived)?.id??'';
    if(selected)await read(selected);
  }
  onMount(()=>{alive=true;let timer:ReturnType<typeof setTimeout>;async function poll(){try{if(!busy)await refresh()}catch(e){if(alive)error=String(e)}finally{if(alive){loading=false;timer=setTimeout(poll,3000)}}}void poll();return()=>{alive=false;clearTimeout(timer)}});
  async function select(id:string){if(busy||pending)return;selected=id;current=undefined;pages=1;editing=false;error='';targets=[];text='';try{await read(id)}catch(e){error=String(e)}}
  function edit(fresh=false){if(pending)return;creating=fresh;editing=true;name=fresh?'':current?.name??'';goal=fresh?'':current?.goal??'';members=fresh?[]:[...(current?.members??[])];if(fresh)createId=randomId();error='';}
  async function save(){if(busy)return;busy=true;error='';try{const result=await call({action:creating?'discussion_create':'discussion_update',discussion_id:creating?createId:selected,title:name,goal,member_chat_ids:members});if(result.discussion){selected=result.discussion.id;current=result.discussion;}editing=false;await refresh()}catch(e){error=String(e)}finally{busy=false}}
  async function archive(){if(!current||busy||pending)return;busy=true;error='';try{await call({action:'discussion_update',discussion_id:selected,archived:!current.archived});showArchived=true;await refresh()}catch(e){error=String(e)}finally{busy=false}}
  async function send(){if(busy||(!pending&&!text.trim()))return;busy=true;error='';try{pending??={action:'discussion_post',discussion_id:selected,message_id:randomId(),text,purpose,recipient_chat_ids:targets.length?[...targets]:[...(current?.members??[])]};const result=await call(pending);if(!result.persisted)throw Error($t('discuss.unconfirmed'));pending=null;text='';pages=1;await refresh()}catch(e){error=String(e)}finally{busy=false}}
  async function more(){busy=true;pages++;try{await read(selected)}catch(e){pages--;error=String(e)}finally{busy=false}}
  function open(id:string){void goto(appUrl(chatLocation(workspaceId,folderId,id)))}
</script>
<div class="discussions">
  <aside><header><strong>{$t('discuss.groups')}</strong><button onclick={()=>edit(true)} disabled={busy||!!pending}>{$t('discuss.create')}</button></header><label><input type="checkbox" bind:checked={showArchived}/>{$t('discuss.showArchived')}</label>
    {#each visible as group (group.id)}<button class="group-row" class:selected={selected===group.id} onclick={()=>select(group.id)} disabled={busy||!!pending}><strong>{group.name}</strong><small>{group.members.length} {$t('discuss.members')}{group.archived?' · '+$t('chat.archive'):''}</small></button>{:else}{#if !loading}<p>{$t('discuss.empty')}</p>{/if}{/each}
  </aside>
  <section>
    {#if error}<p role="alert">{error}</p>{/if}
    {#if editing}<form onsubmit={event=>{event.preventDefault();void save()}}><h2>{$t(creating?'discuss.create':'discuss.manage')}</h2><label>{$t('Name')}<input bind:value={name} maxlength="80" required disabled={busy}/></label><label>{$t('discuss.goal')}<textarea bind:value={goal} maxlength="1000" rows="3" disabled={busy}></textarea></label><fieldset disabled={busy}><legend>{$t('discuss.choose')}</legend>{#each choices as s (s.id)}<label><input type="checkbox" value={s.id} bind:group={members}/><span>{s.title}<small>{s.note}</small></span></label>{:else}<p>{$t('discuss.noSessions')}</p>{/each}{#each missingMembers as id}<label><input type="checkbox" value={id} bind:group={members}/><span>{current?.member_details?.find(m=>m.id===id)?.title??id}<small>{$t('discuss.unavailable')}</small></span></label>{/each}</fieldset><footer><button type="button" onclick={()=>editing=false} disabled={busy}>{$t('Cancel')}</button><button type="submit" disabled={busy||!name.trim()||!members.length}>{$t('Save')}</button></footer></form>
    {:else if current}<header class="group-heading"><div><h2>{current.name}</h2><p>{current.goal||$t('discuss.noGoal')}</p></div><button onclick={()=>edit()} disabled={busy||!!pending}>{$t('discuss.manage')}</button><button onclick={archive} disabled={busy||!!pending}>{$t(current.archived?'chat.unarchive':'chat.archive')}</button></header>
      <div class="members">{#each current.member_details??[] as member}<button onclick={()=>open(member.id)} disabled={member.status==='missing'} title={member.note}><span class:online={member.status==='connected'}></span>{member.title}<small>{$t(member.status==='connected'?'chat.98':'chat.22')}</small></button>{/each}</div>
      <p class="hint">{$t('discuss.hint')}</p>
      <div class="feed">{#if current.next_offset!=null}<button onclick={more} disabled={busy}>{$t('discuss.more')}</button>{/if}
      {#each current.posts??[] as post (post.id)}<article><header><strong>{post.name}</strong><span>{$t(('discuss.'+post.purpose) as MessageKey)}</span><small>{new Date(post.created_at).toLocaleString()}</small></header><p class="message">{post.text}</p><small class="task-id">{$t('discuss.id')}: {post.id}</small><div class="deliveries">{#each post.deliveries as delivery}<details open={post.purpose==='task'}><summary><span>{delivery.title}</span><span class:done={delivery.status==='completed'}>{$t(states[delivery.status]??'discuss.unavailable')}</span></summary>{#if delivery.error}<p role="alert">{delivery.error}</p>{/if}{#each delivery.replies as reply (reply.id)}<div class="result"><small>{$t(reply.final?'discuss.result':'discuss.progress')}</small><p class="message">{reply.text}</p>{#if reply.attachments?.length}<small>{$t('discuss.attachments')}</small>{/if}</div>{/each}<button onclick={()=>open(delivery.chat_id)}>{$t('discuss.open')}</button></details>{/each}</div></article>{:else}<p>{$t('discuss.start')}</p>{/each}</div>
      {#if !current.archived}<form class="composer" onsubmit={event=>{event.preventDefault();void send()}}><div class="send-options"><label>{$t('discuss.purpose')}<select bind:value={purpose} disabled={busy||!!pending}>{#each ['discussion','question','notice','task'] as value}<option {value}>{$t(('discuss.'+value) as MessageKey)}</option>{/each}</select></label><details><summary>{$t('discuss.recipients')} · {targets.length||current.members.length}</summary><p>{$t('discuss.allHint')}</p>{#each current.member_details??[] as member}<label><input type="checkbox" value={member.id} bind:group={targets} disabled={busy||!!pending}/>{member.title}</label>{/each}</details></div><textarea bind:value={text} aria-label={$t('discuss.message')} placeholder={$t('discuss.message')} rows="3" maxlength="8000" disabled={busy||!!pending}></textarea>{#if pending}<p>{$t('discuss.retryHint')}</p>{/if}<button type="submit" disabled={busy||(!pending&&!text.trim())}>{$t(pending?'discuss.retry':'discuss.send')}</button></form>{/if}
    {:else if loading}<p>{$t('Loading…')}</p>{:else}<p>{$t('discuss.empty')}</p>{/if}
  </section>
</div>
<style>
.discussions{display:grid;grid-template-columns:220px minmax(0,1fr);gap:22px;min-height:350px;font-size:13px}aside{border-right:1px solid var(--color-border);padding-right:18px}header,footer,.send-options{display:flex;gap:10px;align-items:center;justify-content:space-between}h2{font-size:20px;font-weight:600}p{line-height:1.7;overflow-wrap:anywhere;margin:8px 0}button{padding:7px 10px;border:1px solid var(--color-border);border-radius:8px;cursor:pointer;background:var(--surface-2)}button:hover{background:var(--surface-hover)}button:disabled{opacity:.5;cursor:default}.group-row{display:grid;width:100%;gap:4px;text-align:left;margin:7px 0}.group-row.selected{border-color:var(--primary);background:var(--surface-hover)}small,.hint{color:var(--color-text-muted)}small{font-size:11px}label{display:flex;gap:9px;align-items:center;margin:10px 0}form>label{align-items:stretch;flex-direction:column}input:not([type=checkbox]),textarea,select{border:1px solid var(--color-border);background:var(--card-bg);padding:9px;border-radius:8px;min-width:0;max-width:100%}textarea{width:100%;resize:vertical}fieldset{padding:12px;border:1px solid var(--color-border);border-radius:10px;max-height:260px;overflow:auto}fieldset small{display:block}footer{justify-content:flex-end;margin-top:12px}.group-heading{align-items:flex-start}.group-heading>div{flex:1;min-width:0}.members{display:flex;flex-wrap:wrap;gap:7px;margin:12px 0}.members button{display:flex;gap:6px;align-items:center}.members span{width:7px;height:7px;border-radius:50%;background:#888}.members .online{background:#35b27c}.feed{display:grid;gap:14px;margin:14px 0}article{padding:16px;border:1px solid var(--color-border);border-radius:12px;background:var(--surface-2)}article header{justify-content:flex-start;flex-wrap:wrap}.message{white-space:pre-wrap}.task-id{display:block;overflow-wrap:anywhere}.deliveries{display:grid;gap:7px;margin-top:12px}details{border:1px solid var(--color-border);border-radius:8px;padding:8px}summary{cursor:pointer}summary span+span{margin-left:12px;color:var(--color-text-muted)}summary .done{color:#36a578}.result{border-left:2px solid var(--color-border);padding:8px;margin:8px 0}.composer{border-top:1px solid var(--color-border);padding-top:15px}.composer>button{display:block;margin:8px 0 0 auto;background:var(--primary);color:white}.send-options{align-items:flex-start;flex-wrap:wrap}.send-options details{min-width:170px}.send-options p{font-size:11px}[role=alert]{color:var(--danger)}button:focus-visible,input:focus-visible,textarea:focus-visible,select:focus-visible{outline:2px solid var(--primary);outline-offset:2px}@media(max-width:850px){.discussions{grid-template-columns:1fr}aside{border-right:0;border-bottom:1px solid var(--color-border);padding:0 0 14px}.group-row{display:inline-grid;width:auto;max-width:100%;margin-right:8px}.group-heading{flex-wrap:wrap}}
</style>
