/** Seed registry. Every operation runs under the same disk lock as chat writes. */
import {createHash, randomUUID, randomBytes} from 'node:crypto';
import {existsSync,readFileSync, writeFileSync, renameSync, rmSync} from 'node:fs';
import type {ChatSession} from './store.js';
export interface SeedStore {
  path(relative:string):string;
  read(id:string):ChatSession;
  save(chat:ChatSession):void;
  ids():string[];
  locked<T>(run:()=>T):T;
}
export interface Seed {
  source?:string; prefix?:string; manual?:boolean;
  provision_key?:string; metadata?:Record<string,string>;
  id:string; account:string; repo_id:string; branch:string; created_at:number;
  url_hash?:string; enrollment_hash:string; access_hash:string; enrollment_until:number; redeemed_at:number;
  access_until:number; last_seen:number; ready:boolean; retired_at:number; reason:string;
  archived:boolean; host_task_id:string; chat_id:string; attachment_id:string;
  generation:number; inflight:string[]; operations:string[];
  /** A still-valid AI that left a chat and now waits as an assignable standby seed. */
  standby?:boolean;
  /** Direct-connect URL for one chat ([接入AI]): assigned only to that chat. */
  bound_chat?:string;
}
interface Library {version:1;enabled:boolean;seeds:Seed[];pairing_hash?:string}
const FILE='docs/chat-sessions/seed-library.seeds.json';
export const SEED_LIBRARY_LIMIT=10000;
const REGISTRY_BYTES=32*1024*1024;
const exchanges=new Map<string,{token:string;expires:number}>();
/** Set by the chat store: a redeemed standby URL must stop being handed out. */
let urlRedeemedHook:((key:string)=>void)|undefined;
export function onSeedUrlRedeemed(hook:(key:string)=>void):void {urlRedeemedHook=hook;}
const scans=new Map<string,number>();
export function seedInvalidate(store:SeedStore):void {const base=store.path(FILE)+'\0';for(const key of scans.keys())if(key.startsWith(base))scans.delete(key);}
export const SEED_SUSPECT_MS=600_000, SEED_REPLACE_MS=720_000;
const hash=(s:string)=>createHash('sha256').update(s).digest('hex');
const uid=(s:unknown):string=>{if(typeof s!=='string'||! /^[a-zA-Z0-9_-]{1,80}$/.test(s))throw Error('Invalid seed ID');return s;};
const label=(s:unknown,max=200):string=>{if(typeof s!=='string'||!s.trim()||Buffer.byteLength(s)>max)throw Error('Invalid seed field');return s.trim();};
export interface SeedFilter {source:string;keyword:string;prefix?:string;account?:string;standby?:boolean;seed_id?:string}
function optionalLabel(value:unknown,max=200):string {if(value===undefined||value===null||value==='')return '';const result=label(value,max);if(/[\r\n\0]/.test(result))throw Error('Invalid seed label');return result;}
export function parseSeedFilter(value:unknown):SeedFilter {
  if(value===undefined||value===null)return {source:'',keyword:''};
  if(!value||typeof value!=='object'||Array.isArray(value))throw Error('Invalid seed filter');
  const filter=value as Record<string,unknown>;return {source:optionalLabel(filter.source).toLowerCase(),keyword:optionalLabel(filter.keyword),...(filter.prefix!==undefined&&filter.prefix!==null?{prefix:optionalLabel(filter.prefix)}:{}),...(filter.account!==undefined&&filter.account!==null?{account:optionalLabel(filter.account)}:{}),...(filter.standby===true?{standby:true}:{}),...(typeof filter.seed_id==='string'&&filter.seed_id?{seed_id:seedIdFilter(filter.seed_id)}:{})};
}
const sourceOf=(s:Seed)=>s.source||'coderabbit';
function seedIdFilter(value:string):string {const v=optionalLabel(value);if(v.length>80)throw Error('Invalid seed filter');return v;}
function matches(s:Seed,filter:SeedFilter):boolean {return (!filter.source||sourceOf(s)===filter.source)&&(!filter.keyword||(s.prefix??'').toLowerCase().includes(filter.keyword.toLowerCase()))&&(filter.prefix===undefined||(s.prefix??'')===filter.prefix)&&(!filter.account||s.account===filter.account)&&(filter.standby!==true||s.standby===true)&&(!filter.seed_id||s.id===filter.seed_id);}
export function readSeeds(store:SeedStore):Library {
  let body:Buffer;try{body=readFileSync(store.path(FILE));}catch(e){if((e as NodeJS.ErrnoException).code==='ENOENT')return {version:1,enabled:false,seeds:[]};throw e;}
  if(body.length>REGISTRY_BYTES)throw Error('Seed registry exceeds limit');
  const data=JSON.parse(body.toString()) as Library;
  if(data.version!==1||typeof data.enabled!=='boolean'||!Array.isArray(data.seeds)||data.seeds.length>SEED_LIBRARY_LIMIT)throw Error('Invalid seed registry');
  for(const s of data.seeds){uid(s.id);if(!Array.isArray(s.inflight)||!Array.isArray(s.operations))throw Error('Invalid seed registry');}
  return data;
}
function saveSeeds(store:SeedStore,data:Library):void {
  const temp=store.path(`${FILE}.${randomUUID()}.tmp`),body=JSON.stringify(data);
  if(Buffer.byteLength(body)>REGISTRY_BYTES)throw Error('Seed registry exceeds limit');
  try{writeFileSync(temp,body,{mode:0o600,flag:'wx'});renameSync(temp,store.path(FILE));}finally{rmSync(temp,{force:true});}
}
function retirement(s:Seed,reason:string,now:number){s.retired_at=now;s.reason=reason;s.ready=false;}
function eligibleChat(chat:ChatSession):boolean {
  const last=chat.messages.filter(m=>m.role==='user').at(-1);
  const awaiting=last&&chat.messages.some(m=>m.role==='assistant'&&m.reply_to===last.id&&m.final&&m.awaiting_user);
  return chat.seed_auto===true&&!chat.closed&&!chat.archived&&chat.mode!=='group'&&!awaiting;
}
function publicSeed(s:Seed,now:number){
  const state=s.retired_at?'retired':s.inflight.length||s.operations.length?'working':!s.redeemed_at?'preparing':now-s.last_seen>SEED_SUSPECT_MS?'offline':s.chat_id?'assigned':s.ready?'ready':'preparing';
  return {enrollment_until:s.enrollment_until,redeemed_at:s.redeemed_at,access_until:s.access_until,id:s.id,source:sourceOf(s),prefix:s.prefix??'',manual:s.manual===true,account:s.account,repo_id:s.repo_id,branch:s.branch,metadata:s.metadata??{},created_at:s.created_at,last_seen:s.last_seen,status:state,reason:s.reason,archived:s.archived,retired_at:s.retired_at,host_task_id:s.host_task_id,chat_id:s.chat_id,generation:s.generation,unresolved_operations:s.inflight.length+s.operations.length,standby:s.standby===true};
}
function inventory(data:Library,args:Record<string,unknown>,now:number){
  const groups=new Map<string,{source:string;prefix:string;total:number;ready:number}>();
  for(const seed of data.seeds.filter(s=>!s.retired_at)){const source=sourceOf(seed),prefix=seed.prefix??'',key=JSON.stringify([source,prefix]),g=groups.get(key)??{source,prefix,total:0,ready:0};g.total++;if(publicSeed(seed,now).status==='ready')g.ready++;groups.set(key,g);}
  const readyGroups=new Map<string,{source:string;prefix:string;account:string;ready:number}>();
  for(const seed of data.seeds){if(publicSeed(seed,now).status!=='ready')continue;const source=sourceOf(seed),prefix=seed.prefix??'',account=seed.account,key=JSON.stringify([source,prefix,account]),group=readyGroups.get(key)??{source,prefix,account,ready:0};group.ready++;readyGroups.set(key,group);}
  // Every non-retired pool, including empty ones, so a new chat always shows the categories.
  const poolGroups=new Map<string,{source:string;prefix:string;account:string;standby:boolean;total:number;ready:number}>();
  for(const seed of data.seeds.filter(s=>!s.retired_at&&!s.bound_chat)){const source=sourceOf(seed),prefix=seed.prefix??'',account=seed.account,standby=seed.standby===true,key=JSON.stringify([source,prefix,account,standby]),g=poolGroups.get(key)??{source,prefix,account,standby,total:0,ready:0};g.total++;if(publicSeed(seed,now).status==='ready')g.ready++;poolGroups.set(key,g);}
  const selection=parseSeedFilter({source:args.source,keyword:args.keyword});
  const all=data.seeds.map(s=>publicSeed(s,now)),counts:Record<string,number>={},accounts=[...new Set(all.map(s=>s.account))].sort();
  for(const s of all)counts[s.status]=(counts[s.status]??0)+1;
  const search=typeof args.search==='string'?args.search.trim().toLowerCase():'';
  const filtered=all.filter(s=>matches(s as unknown as Seed,selection)&&(args.include_archived!==false||!s.archived)&&(!args.account||s.account===args.account)&&(!args.status||s.status===args.status)&&(!search||[s.id,s.account,s.source,s.prefix,s.repo_id,s.branch,s.host_task_id,...Object.values(s.metadata)].some(v=>v.toLowerCase().includes(search))));
  const summaries=new Map<string,{account:string;total:number;valid:number;counts:Record<string,number>}>();
  for(const seed of filtered){const group=summaries.get(seed.account)??{account:seed.account,total:0,valid:0,counts:{}};group.total++;group.counts[seed.status]=(group.counts[seed.status]??0)+1;if(['ready','assigned','working'].includes(seed.status))group.valid++;summaries.set(seed.account,group);}
  const account_summaries=accounts.flatMap(account=>{const group=summaries.get(account);return group?[group]:[];});
  const limit=Number.isInteger(args.limit)?Math.max(1,Math.min(200,Number(args.limit))):all.length;
  const offset=Number.isInteger(args.offset)?Math.max(0,Number(args.offset)):0;
  return {pool_groups:[...poolGroups.values()],ready_groups:[...readyGroups.values()],account_summaries,groups:[...groups.values()],sources:[...new Set(all.map(s=>s.source))].sort(),enabled:data.enabled,seeds:filtered.slice(offset,offset+limit),retired_count:counts.retired??0,total:all.length,filtered_total:filtered.length,offset,limit,counts,accounts,pairing_enabled:!!data.pairing_hash,capacity:SEED_LIBRARY_LIMIT};
}
// Called by chatUi while it already owns the folder lock.
function ensureSettled(seed:Seed):void {
  if(seed.inflight.length||seed.operations.length)throw Error('Seed has unresolved operations; finish or inspect them before release');
}
function releaseSeed(store:SeedStore,seed:Seed):void {
  ensureSettled(seed);
  if(seed.chat_id&&existsSync(store.path(`docs/chat-sessions/${seed.chat_id}.json`))){
    const chat=store.read(seed.chat_id);
    if(chat.seed_owner===seed.id&&chat.attachment_id===seed.attachment_id){
      chat.attachment_id='';chat.seed_owner='';chat.seed_auto=false;chat.seed_pending=false;
      chat.seed_generation=(chat.seed_generation??0)+1;chat.updated_at=Date.now();store.save(chat);
    }
  }
  seed.chat_id='';seed.attachment_id='';seed.generation++;seed.last_seen=0;seed.ready=false;seed.reason='workspace_removed';
}
export function seedReleaseWorkspace(store:SeedStore):void {
  if(!existsSync(store.path(FILE)))return;
  store.locked(()=>{
    const data=readSeeds(store);data.seeds.forEach(ensureSettled);
    data.enabled=false;saveSeeds(store,data);
    // Explicit pool choices bypass `enabled`; withdraw them as well.
    for(const id of store.ids()){let chat;try{chat=store.read(id);}catch{continue;}if(chat.seed_explicit||chat.seed_direct){if(chat.seed_direct&&!chat.seed_owner){chat.seed_auto=false;chat.seed_direct=false;}chat.seed_explicit=false;store.save(chat);}}
    for(const seed of data.seeds)if(!seed.retired_at&&seed.bound_chat&&!seed.chat_id)retirement(seed,'workspace_released',Date.now());
    for(const seed of data.seeds)if(!seed.retired_at)releaseSeed(store,seed);
    saveSeeds(store,data);seedInvalidate(store);
  });
}
/** Return a still-valid seed that owns the chat to the library as standby. Caller holds the lock and detaches the chat. */
export function seedReleaseToStandby(store:SeedStore,chat:ChatSession):boolean {
  if(!chat.seed_owner||!existsSync(store.path(FILE)))return false;
  const data=readSeeds(store),now=Date.now(),seed=data.seeds.find(s=>s.id===chat.seed_owner);
  if(!seed||seed.retired_at||!seed.redeemed_at||seed.chat_id!==chat.id||seed.attachment_id!==chat.attachment_id||seed.inflight.length||seed.operations.length||now-seed.last_seen>SEED_SUSPECT_MS)return false;
  seed.chat_id='';seed.attachment_id='';seed.generation++;seed.standby=true;delete seed.bound_chat;seed.ready=true;seed.reason='standby';
  saveSeeds(store,data);seedInvalidate(store);return true;
}
/** A direct-connect URL that was never used stops working when its chat is disconnected or switched. */
export function seedRetireUnusedDirect(store:SeedStore,chatId:string):void {
  if(!existsSync(store.path(FILE)))return;
  const data=readSeeds(store),now=Date.now();let changed=false;
  for(const s of data.seeds)if(s.bound_chat===chatId&&!s.retired_at&&!s.chat_id){retirement(s,'conversation_detached',now);changed=true;}
  if(changed){saveSeeds(store,data);seedInvalidate(store);}
}
/** Enroll a directly connected AI that left a chat as a standby seed; returns its one-hour URL key. */
export function seedEnrollDirectStandby(store:SeedStore,agentName:string):string {
  const name=[...agentName].filter(c=>c>=' '&&!(c>='\u007f'&&c<='\u009f')).slice(0,60).join('').trim()||'AI';
  // Standby URLs never connected within an hour are retired so the standby card stays clean.
  const data=readSeeds(store),now=Date.now();let changed=false;
  for(const s of data.seeds)if(s.standby&&!s.retired_at&&!s.redeemed_at&&now>s.enrollment_until){retirement(s,'inactive',now);changed=true;}
  if(changed)saveSeeds(store,data);
  const created=seedUi(store,{action:'seed_manual',source:'standby',prefix:name,account:name,_standby:true}) as {batch:{url_key:string}[]};
  const key=created.batch[0]?.url_key;if(!key)throw Error('Standby enrollment failed');return key;
}
/** [接入AI] without OAuth: a one-hour URL carrying its own credential, bound to one chat. */
function seedDirectUrl(store:SeedStore,args:Record<string,unknown>):Record<string,unknown> {
  const chat=store.read(uid(args.chat_id));
  if(chat.closed||chat.mode==='group')throw Error('Only open work chats support direct connect URLs');
  if(chat.attachment_id)throw Error('Disconnect the current AI before connecting another one');
  const data=readSeeds(store),now=Date.now();let changed=false;
  // Replace every URL of this chat that is not serving it; drop never-used rows so they do not pile up.
  const before=data.seeds.length;data.seeds=data.seeds.filter(s=>!(s.bound_chat===chat.id&&!s.redeemed_at));changed=data.seeds.length!==before;
  for(const s of data.seeds)if(s.bound_chat===chat.id&&!s.retired_at&&!s.chat_id){retirement(s,'replaced',now);changed=true;}
  if(changed)saveSeeds(store,data);
  // A stale owner (e.g. a busy seed that was plainly detached) would block the bound seed.
  if(chat.seed_owner){chat.seed_owner='';chat.seed_pending=false;chat.seed_generation=(chat.seed_generation??0)+1;}
  const created=seedUi(store,{action:'seed_manual',source:'direct',prefix:'接入AI',account:'direct',_bound_chat:chat.id}) as {batch:{seed_id:string}[]};
  const seed=created.batch[0];if(!seed)throw Error('Direct connect URL failed');
  chat.seed_auto=true;chat.seed_explicit=true;chat.seed_direct=true;chat.seed_filter={source:'',keyword:'',seed_id:seed.seed_id};chat.updated_at=now;store.save(chat);seedInvalidate(store);
  return {ok:true,seed};
}
export function seedUi(store:SeedStore,args:Record<string,unknown>):Record<string,unknown> {
  if(args.action==='seed_direct')return seedDirectUrl(store,args);
  const data=readSeeds(store),now=Date.now();
  if(args.action==='seed_pairing'){
    if(args.enabled===false)data.pairing_hash='';
    else {const token=String(args.token??'');if(!/^[a-f0-9]{64}$/.test(token))throw Error('Pairing token must contain 64 hexadecimal characters');data.pairing_hash=hash(token);}
    saveSeeds(store,data);
  }
  else if(args.action==='seed_settings') {if(typeof args.enabled!=='boolean')throw Error('enabled must be boolean');data.enabled=args.enabled;saveSeeds(store,data);}
  else if(args.action==='seed_batch'||args.action==='seed_manual') {
    const manual=args.action==='seed_manual',source=optionalLabel(args.source)|| (manual?'custom':'coderabbit'),prefix=optionalLabel(args.prefix);
    const count=manual?1:Number(args.count);if(!Number.isInteger(count)||count<1||count>50||data.seeds.length+count>SEED_LIBRARY_LIMIT)throw Error('Batch must contain 1–50 seeds (library limit 10000)');
    const account=label(args.account),repo_id=manual?'':label(args.repo_id),branch=manual?'':label(args.branch),batch=[];
    // Repository/branch describe this host task, not the MCP folder's pool.
    // Adding another host must not revoke pending tickets or existing owners.
    for(let i=0;i<count;i++){
      const id=randomUUID(),provision_key=String(args._provision_key??'');
      const ticket=provision_key?hash(String(args._provision_token)+'\0'+provision_key+'\0'+id):randomBytes(32).toString('hex');
      const url_key=hash('seed-url-v1\0'+ticket);
      const metadata:Record<string,string>={};for(const key of ['account_id','provider_user_id','repo_name','organization_id','workspace_id'])if((args.metadata as Record<string,unknown>|undefined)?.[key])metadata[key]=label((args.metadata as Record<string,unknown>)[key]);
      const seed:Seed={id,source:source.toLowerCase(),prefix,manual,provision_key,metadata,account,repo_id,branch,created_at:now,url_hash:hash(url_key),enrollment_hash:hash(ticket),access_hash:'',enrollment_until:now+3600_000,redeemed_at:0,access_until:0,last_seen:0,ready:false,retired_at:0,reason:'',archived:false,host_task_id:'',chat_id:'',attachment_id:'',generation:0,inflight:[],operations:[],standby:args._standby===true,...(typeof args._bound_chat==='string'&&args._bound_chat?{bound_chat:args._bound_chat}:{})};
      data.seeds.push(seed);batch.push({seed_id:seed.id,ticket,url_key,source:seed.source,prefix,manual,repo_id,branch,account,expires_at:seed.enrollment_until});
    }
    saveSeeds(store,data);return {batch};
  } else if(['seed_label','seed_retire','seed_purge','seed_release'].includes(String(args.action))) {
    const values=Array.isArray(args.seed_ids)?args.seed_ids:[args.seed_id];
    if(!values.length||values.length>100)throw Error('Select 1–100 seeds');
    const ids=values.map(uid),selected=new Set(ids);
    if(ids.length!==selected.size)throw Error('Duplicate seed ID');
    const seeds=ids.map(id=>{const seed=data.seeds.find(s=>s.id===id);if(!seed)throw Error('Seed not found');return seed;});
    const source=optionalLabel(args.source).toLowerCase(),prefix=optionalLabel(args.prefix);
    for(const seed of seeds){
      if(args.action!=='seed_label')ensureSettled(seed);
      if(args.action==='seed_purge'&&!seed.retired_at)throw Error('Only retired seeds can be permanently deleted');
      if(args.action==='seed_release'&&seed.retired_at)throw Error('Retired seeds cannot be released');
    }
    for(const seed of seeds){
      if(args.action==='seed_label'){seed.source=source||sourceOf(seed);seed.prefix=prefix;}
      if(args.action==='seed_retire'&&!seed.retired_at)retirement(seed,'manual',now);
      if(args.action==='seed_release')releaseSeed(store,seed);
    }
    if(args.action==='seed_purge')data.seeds=data.seeds.filter(s=>!selected.has(s.id));
    saveSeeds(store,data);seedInvalidate(store);return {...inventory(data,args,now),affected_count:selected.size};
  } else if(args.action==='seed_reissue') {
    const seed=data.seeds.find(s=>s.id===uid(args.seed_id));if(!seed)throw Error('Seed not found');ensureSettled(seed);
    if(seed.retired_at||seed.redeemed_at||now<=seed.enrollment_until||seed.chat_id)throw Error('Only expired, unconnected seeds can be reissued');
    const ticket=randomBytes(32).toString('hex'),url_key=hash('seed-url-v1\0'+ticket);
    seed.enrollment_hash=hash(ticket);seed.url_hash=hash(url_key);seed.enrollment_until=now+3600_000;seed.reason='reissued';seed.host_task_id='';
    saveSeeds(store,data);return {batch:[{seed_id:seed.id,ticket,url_key,source:sourceOf(seed),prefix:seed.prefix,manual:seed.manual,account:seed.account,repo_id:seed.repo_id,branch:seed.branch,expires_at:seed.enrollment_until}]};
  } else if(args.action==='seed_cleanup') {
    let affected=0;
    for(const s of data.seeds){
      if(!s.retired_at&&s.reason!=='workspace_removed'&&!s.chat_id&&!s.inflight.length&&!s.operations.length&&((!s.redeemed_at&&now>s.enrollment_until)||(s.redeemed_at&&now-s.last_seen>SEED_REPLACE_MS))){retirement(s,'inactive',now);affected++;}
      if(s.retired_at)s.archived=true;
    }saveSeeds(store,data);return {...inventory(data,args,now),affected_count:affected};
  } else if(args.action==='seed_resume_chat') {
    const chat=store.read(uid(args.chat_id));if(chat.closed||chat.mode==='group')throw Error('Only open work chats support seeds');
    const explicit=!!args.seed_filter&&typeof args.seed_filter==='object'&&!Array.isArray(args.seed_filter);
    // A stale seed_owner without an attachment is replaced once that seed is gone; only a live attachment blocks.
    if(explicit&&chat.attachment_id)throw Error('Disconnect the current AI before choosing a seed pool');
    chat.seed_auto=true;if(explicit){chat.seed_filter=parseSeedFilter(args.seed_filter);chat.seed_explicit=true;chat.seed_direct=false;}chat.updated_at=now;store.save(chat);seedInvalidate(store);
  } else if(args.action!=='seed_list')throw Error('Unknown seed action');
  return inventory(data,args,now);
}
/** The browser capability can inspect capacity and provision seeds, never invoke project tools. */
export function seedProvision(store:SeedStore,token:string,args:Record<string,unknown>):Record<string,unknown>{
  return store.locked(()=>{
    const data=readSeeds(store);
    if(!/^[a-f0-9]{64}$/.test(token)||!data.pairing_hash||hash(token)!==data.pairing_hash)throw Error('Pairing token invalid or revoked');
    if(args.action==='status')return {ok:true,total:data.seeds.length,capacity:SEED_LIBRARY_LIMIT,batch_limit:50};
    if(args.action!=='batch')throw Error('Pairing only supports status and batch');
    const request=uid(args.request_id),key=hash(token+'\0'+request),existing=data.seeds.filter(s=>s.provision_key===key);
    if(existing.length){
      if(existing.length!==args.count||existing.some(s=>s.account!==args.account||s.repo_id!==args.repo_id||s.branch!==args.branch||(s.prefix??'')!==optionalLabel(args.prefix)||s.retired_at||s.enrollment_hash!==hash(hash(token+'\0'+key+'\0'+s.id))||s.enrollment_until<=Date.now()))throw Error('Provision request changed or expired; use a new request ID');
      return {batch:existing.map(s=>({seed_id:s.id,ticket:hash(token+'\0'+key+'\0'+s.id),...(s.url_hash?{url_key:hash('seed-url-v1\0'+hash(token+'\0'+key+'\0'+s.id))}:{}),source:sourceOf(s),prefix:s.prefix??'',manual:s.manual===true,repo_id:s.repo_id,branch:s.branch,account:s.account,expires_at:s.enrollment_until}))};
    }
    return seedUi(store,{action:'seed_batch',source:'coderabbit',prefix:args.prefix,account:args.account,repo_id:args.repo_id,branch:args.branch,count:args.count,metadata:args.metadata,_provision_key:key,_provision_token:token});
  });
}
export function seedInitialize(store:SeedStore,seedId:string,ticket:string,now=Date.now()):string {
  return store.locked(()=>{
    const data=readSeeds(store),s=data.seeds.find(s=>s.id===uid(seedId));
    if(!s||s.retired_at||!ticket||hash(ticket)!==s.enrollment_hash||now>s.enrollment_until||(s.redeemed_at&&now-s.redeemed_at>60_000))throw Error('Seed enrollment expired or invalid');
    const key=store.path(FILE)+':'+seedId;
    for(const [key,value] of exchanges)if(value.expires<now)exchanges.delete(key);
    if(s.redeemed_at){const receipt=exchanges.get(key);if(!receipt)throw Error('Enrollment already used; initialize receipt unavailable');return receipt.token;}
    const token=randomBytes(32).toString('hex');
    s.redeemed_at=now;s.access_until=now+7*86400_000;s.access_hash=hash(token);s.last_seen=now;saveSeeds(store,data);
    exchanges.set(key,{token,expires:now+60000});return token;
  });
}
/** Resolve a URL capability without reflecting it. GET never redeems or renews it. */
export function seedUrl(store:SeedStore,key:string,redeem=false,now=Date.now()):string {
  return store.locked(()=>{
    if(!/^[a-f0-9]{64}$/.test(key))throw Error('Seed URL invalid or expired');
    const data=readSeeds(store),s=data.seeds.find(s=>s.url_hash===hash(key));
    if(!s||s.retired_at||now>(s.redeemed_at?s.access_until:s.enrollment_until))throw Error('Seed URL invalid or expired');
    if(redeem&&!s.redeemed_at){s.redeemed_at=now;s.access_until=now+7*86400_000;s.last_seen=now;saveSeeds(store,data);urlRedeemedHook?.(key);}
    return s.id;
  });
}
function authorized(data:Library,id:string,token:string,now:number):Seed {
  const s=data.seeds.find(s=>s.id===uid(id));
  if(!s||s.retired_at||!s.redeemed_at||!token||(hash(token)!==s.access_hash&&hash(token)!==s.url_hash)||now>s.access_until)throw Error('Seed access expired, retired or invalid');
  return s;
}
export function seedCreated(store:SeedStore,id:string,ticket:string,taskId:string):void {
  store.locked(()=>{const data=readSeeds(store),s=data.seeds.find(s=>s.id===uid(id));
    if(!s||s.retired_at||hash(ticket)!==s.enrollment_hash||Date.now()>s.enrollment_until)throw Error('Invalid enrollment');
    uid(taskId);if(s.host_task_id&&s.host_task_id!==taskId)throw Error('Seed already bound to another task');s.host_task_id=taskId;saveSeeds(store,data);
  });
}
export function seedAuthenticate(store:SeedStore,id:string,token:string):void {store.locked(()=>{authorized(readSeeds(store),id,token,Date.now());});}
export function seedPoll(store:SeedStore,id:string,token:string,now=Date.now()):Record<string,unknown> {
  return store.locked(()=>{
    const data=readSeeds(store),s=authorized(data,id,token,now),wasReady=s.ready;
    let dirty=!s.ready;if(now-s.last_seen>=60000){s.last_seen=now;dirty=true;}s.ready=true;
    const key=store.path(FILE)+'\0'+JSON.stringify([sourceOf(s),s.prefix??'',s.account]),lastScan=scans.get(key);
    if(!s.chat_id&&wasReady&&lastScan!==undefined&&now>=lastScan&&now-lastScan<1000){
      if(dirty)saveSeeds(store,data);
      return {status:'idle',instruction:'Make one new independent wait (chat_wait or seed_wait, timeout_ms:25000). Idle is normal. The user writes in the app at any time; idle only means no new message yet, so just wait again. Respect host execution limits.'};
    }
    if(!s.chat_id){if(scans.size>1024)for(const [key,time] of scans)if(now-time>60000)scans.delete(key);scans.set(key,now);}
    // Reconcile interrupted registry saves using the authoritative chat owner.
    const chats=s.chat_id?(existsSync(store.path('docs/chat-sessions/'+s.chat_id+'.json'))?[store.read(s.chat_id)]:[]):store.ids().map(id=>store.read(id)),byId=new Map(data.seeds.map(s=>[s.id,s]));
    for(const chat of chats){if(chat.seed_owner&&chat.seed_auto&&chat.attachment_id){const owner=byId.get(chat.seed_owner);if(owner&&!owner.retired_at){const generation=chat.seed_generation??1;if(owner.chat_id!==chat.id||owner.attachment_id!==chat.attachment_id||owner.generation!==generation)dirty=true;owner.chat_id=chat.id;owner.attachment_id=chat.attachment_id;owner.generation=generation;}}}
    if(s.chat_id){const chat=chats.find(c=>c.id===s.chat_id);if(!chat||chat.closed||!chat.seed_auto||chat.seed_owner!==s.id||chat.attachment_id!==s.attachment_id){retirement(s,'conversation_detached',now);saveSeeds(store,data);return {status:'retired'};}}
    if(!s.chat_id){
      for(const chat of chats.sort((a,b)=>a.created_at-b.created_at)){
        // The library switch only governs automatic assignment; an explicitly chosen pool is always served.
        if(!data.enabled&&chat.seed_explicit!==true)continue;
        // A direct-connect seed serves only its own chat.
        if(s.bound_chat&&chat.id!==s.bound_chat)continue;
        if(!eligibleChat(chat)||!matches(s,parseSeedFilter(chat.seed_filter)))continue;
        const old=chat.seed_owner?byId.get(chat.seed_owner):undefined;
        if(old){
          if(old.inflight.length||old.operations.length)continue;
          if(!old.retired_at&&(now-old.last_seen<=SEED_REPLACE_MS||chat.lease_until>now))continue;
          if(!old.retired_at)retirement(old,'activity_timeout',now);
        } else if(chat.attachment_id)continue; // Never take over a manually connected AI.
        saveSeeds(store,data); // Revoke old identity before writing the new owner.
        const attachment=randomUUID();chat.seed_owner=s.id;chat.seed_generation=(chat.seed_generation??0)+1;chat.attachment_id=attachment;chat.lease_until=0;chat.seed_pending=true;chat.agent_name=`Seed-${s.id.slice(0,8)}`;chat.updated_at=now;
        store.save(chat);s.chat_id=chat.id;s.attachment_id=attachment;s.generation=chat.seed_generation;s.standby=false;s.reason='';dirty=true;scans.delete(key);break;
      }
    }
    if(dirty)saveSeeds(store,data);
    return s.chat_id?{status:'assigned',needs_open:store.read(s.chat_id).seed_pending===true,chat_id:s.chat_id,attachment_id:s.attachment_id,generation:s.generation,instruction:'Call chat_open with this chat_id and attachment_id. Read the complete returned skill and history. Preserve the existing pending message, plan and user constraints; inspect files and prior operation results before resuming. Work through this MCP workspace. Do not replay completed actions or use an old host sandbox as the project source.'}:{status:'idle',instruction:'Make one new independent wait (chat_wait or seed_wait, timeout_ms:25000). Idle is normal. The user writes in the app at any time; idle only means no new message yet, so just wait again. Respect host execution limits.'};
  });
}
export function seedBegin(store:SeedStore,id:string,token:string,name:string,args:Record<string,unknown>,requestId:string):{chat_id:string;attachment_id:string;call_id:string} {
  return store.locked(()=>{
    const data=readSeeds(store),s=authorized(data,id,token,Date.now());
    if(!s.chat_id)throw Error('Seed is not assigned; use seed_wait');
    const chat=store.read(s.chat_id);
    if(chat.closed||!chat.seed_auto||chat.seed_owner!==s.id||chat.attachment_id!==s.attachment_id)throw Error('Seed assignment replaced or paused');
    if(chat.seed_pending&&name!=='chat_open')throw Error('Confirm assignment with chat_open before project work');
    if(args.chat_id!==undefined&&args.chat_id!==s.chat_id)throw Error('Seed may only access its assigned chat');
    if(args.attachment_id!==undefined&&args.attachment_id!==s.attachment_id)throw Error('Seed attachment mismatch');
    if(['wait_command','send_input','kill_session','read_output'].includes(name)&&!s.operations.includes(String(args.session_id)))throw Error('Process does not belong to this seed');
    const call_id=hash(requestId);if(s.inflight.includes(call_id))throw Error('Request already in flight; do not repeat it');
    if(s.inflight.length>=4)throw Error('Too many seed requests in flight');
    if(['exec_command','apply_patch','edit','edit_file','write_file','file_ops','format_files','send_input','kill_session'].includes(name))s.inflight.push(call_id);s.last_seen=Date.now();saveSeeds(store,data);return {chat_id:s.chat_id,attachment_id:s.attachment_id,call_id};
  });
}
export function seedFinish(store:SeedStore,id:string,callId:string,name:string,args:Record<string,unknown>,result:Record<string,unknown>):void {
  store.locked(()=>{
    const data=readSeeds(store),s=data.seeds.find(s=>s.id===id);if(!s)return;
    const process=result.process_still_running===true||result.status==='running'||result.status==='background';
    const sid=String(result.session_id??'');
    if(process&&sid&&!s.operations.includes(sid))s.operations.push(sid);
    if(!process&&result.process_still_running===false&&args.session_id!==undefined)s.operations=s.operations.filter(id=>id!==String(args.session_id));
    // An unknown transport outcome keeps its in-flight marker for inspection.
    if(result.__unknown!==true)s.inflight=s.inflight.filter(id=>id!==callId);
    s.last_seen=Date.now();saveSeeds(store,data);
  });
}
/** Read-only inventory for the server's recovery probe; never expose capabilities. */
export function seedPendingProcesses(store:SeedStore):{seed_id:string;session_id:string}[]{
  return store.locked(()=>readSeeds(store).seeds.flatMap(s=>s.operations.map(session_id=>({seed_id:s.id,session_id}))).slice(0,16));
}
export function seedProcessSettled(store:SeedStore,seedId:string,sessionId:string):void{
  store.locked(()=>{const data=readSeeds(store),s=data.seeds.find(s=>s.id===seedId);if(s&&s.operations.includes(sessionId)){s.operations=s.operations.filter(id=>id!==sessionId);saveSeeds(store,data);}});
}
