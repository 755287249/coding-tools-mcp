/** Seed registry. Every operation runs under the same disk lock as chat writes. */
import {createHash, randomUUID, randomBytes} from 'node:crypto';
import {readFileSync, writeFileSync, renameSync, rmSync} from 'node:fs';
import type {ChatSession} from './store.js';
export interface SeedStore {
  path(relative:string):string;
  read(id:string):ChatSession;
  save(chat:ChatSession):void;
  ids():string[];
  locked<T>(run:()=>T):T;
}
export interface Seed {
  id:string; account:string; repo_id:string; branch:string; created_at:number;
  enrollment_hash:string; access_hash:string; enrollment_until:number; redeemed_at:number;
  access_until:number; last_seen:number; ready:boolean; retired_at:number; reason:string;
  archived:boolean; host_task_id:string; chat_id:string; attachment_id:string;
  generation:number; inflight:string[]; operations:string[];
}
interface Library {version:1;enabled:boolean;seeds:Seed[]}
const FILE='docs/chat-sessions/seed-library.seeds.json';
const exchanges=new Map<string,{token:string;expires:number}>();
export const SEED_SUSPECT_MS=600_000, SEED_REPLACE_MS=720_000;
const hash=(s:string)=>createHash('sha256').update(s).digest('hex');
const uid=(s:unknown):string=>{if(typeof s!=='string'||! /^[a-zA-Z0-9_-]{1,80}$/.test(s))throw Error('Invalid seed ID');return s;};
const label=(s:unknown,max=200):string=>{if(typeof s!=='string'||!s.trim()||Buffer.byteLength(s)>max)throw Error('Invalid seed field');return s.trim();};
export function readSeeds(store:SeedStore):Library {
  let body:Buffer;try{body=readFileSync(store.path(FILE));}catch(e){if((e as NodeJS.ErrnoException).code==='ENOENT')return {version:1,enabled:false,seeds:[]};throw e;}
  if(body.length>2*1024*1024)throw Error('Seed registry exceeds limit');
  const data=JSON.parse(body.toString()) as Library;
  if(data.version!==1||typeof data.enabled!=='boolean'||!Array.isArray(data.seeds)||data.seeds.length>1000)throw Error('Invalid seed registry');
  for(const s of data.seeds){uid(s.id);if(!Array.isArray(s.inflight)||!Array.isArray(s.operations))throw Error('Invalid seed registry');}
  return data;
}
function saveSeeds(store:SeedStore,data:Library):void {
  const temp=store.path(`${FILE}.${randomUUID()}.tmp`),body=JSON.stringify(data);
  if(Buffer.byteLength(body)>2*1024*1024)throw Error('Seed registry exceeds limit');
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
  return {id:s.id,account:s.account,repo_id:s.repo_id,branch:s.branch,created_at:s.created_at,last_seen:s.last_seen,status:state,reason:s.reason,archived:s.archived,retired_at:s.retired_at,host_task_id:s.host_task_id,chat_id:s.chat_id,generation:s.generation,unresolved_operations:s.inflight.length+s.operations.length};
}
// Called by chatUi while it already owns the folder lock.
export function seedUi(store:SeedStore,args:Record<string,unknown>):Record<string,unknown> {
  const data=readSeeds(store),now=Date.now();
  if(args.action==='seed_settings') {if(typeof args.enabled!=='boolean')throw Error('enabled must be boolean');data.enabled=args.enabled;saveSeeds(store,data);}
  else if(args.action==='seed_batch') {
    const count=Number(args.count);if(!Number.isInteger(count)||count<1||count>50||data.seeds.length+count>1000)throw Error('Batch must contain 1–50 seeds (library limit 1000)');
    const account=label(args.account),repo_id=label(args.repo_id),branch=label(args.branch),batch=[];
    const mismatched=data.seeds.filter(s=>s.repo_id!==repo_id||s.branch!==branch);
    if(mismatched.length){
      if(mismatched.some(s=>s.redeemed_at||s.host_task_id||s.chat_id||s.inflight.length||s.operations.length))throw Error('Connected seeds bind this folder to another repository or branch; use a separate project folder');
      const pending=mismatched.filter(s=>!s.retired_at);
      if(pending.length&&args.replace_unconnected!==true)throw Error('Confirm replacing unconnected batches to correct the repository or branch');
      for(const s of pending){retirement(s,'configuration_corrected',now);s.archived=true;}
    }
    for(let i=0;i<count;i++){
      const ticket=randomUUID().replaceAll('-','')+randomUUID().replaceAll('-','');
      const seed:Seed={id:randomUUID(),account,repo_id,branch,created_at:now,enrollment_hash:hash(ticket),access_hash:'',enrollment_until:now+3600_000,redeemed_at:0,access_until:0,last_seen:0,ready:false,retired_at:0,reason:'',archived:false,host_task_id:'',chat_id:'',attachment_id:'',generation:0,inflight:[],operations:[]};
      data.seeds.push(seed);batch.push({seed_id:seed.id,ticket,repo_id,branch,account,expires_at:seed.enrollment_until});
    }
    saveSeeds(store,data);return {batch};
  } else if(args.action==='seed_retire') {
    const seed=data.seeds.find(s=>s.id===uid(args.seed_id));if(!seed)throw Error('Seed not found');
    if(seed.inflight.length||seed.operations.length)throw Error('Seed has unresolved operations; finish or inspect them before retirement');
    if(!seed.retired_at)retirement(seed,'manual',now);saveSeeds(store,data);
  } else if(args.action==='seed_cleanup') {
    for(const s of data.seeds){
      if(!s.retired_at&&!s.chat_id&&!s.inflight.length&&!s.operations.length&&((!s.redeemed_at&&now>s.enrollment_until)||(s.redeemed_at&&now-s.last_seen>SEED_REPLACE_MS)))retirement(s,'inactive',now);
      if(s.retired_at)s.archived=true;
    }saveSeeds(store,data);
  } else if(args.action==='seed_resume_chat') {
    const chat=store.read(uid(args.chat_id));if(chat.closed||chat.mode==='group')throw Error('Only open work chats support seeds');chat.seed_auto=true;store.save(chat);
  } else if(args.action!=='seed_list')throw Error('Unknown seed action');
  return {enabled:data.enabled,seeds:data.seeds.map(s=>publicSeed(s,now)),retired_count:data.seeds.filter(s=>s.retired_at).length};
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
function authorized(data:Library,id:string,token:string,now:number):Seed {
  const s=data.seeds.find(s=>s.id===uid(id));
  if(!s||s.retired_at||!s.access_hash||!token||hash(token)!==s.access_hash||now>s.access_until)throw Error('Seed access expired, retired or invalid');
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
    const data=readSeeds(store),before=JSON.stringify(data),s=authorized(data,id,token,now);if(now-s.last_seen>=15000)s.last_seen=now;s.ready=true;
    // Reconcile interrupted registry saves using the authoritative chat owner.
    const chats=store.ids().map(id=>store.read(id));
    for(const chat of chats){if(chat.seed_owner&&chat.seed_auto&&chat.attachment_id){const owner=data.seeds.find(seed=>seed.id===chat.seed_owner);if(owner&&!owner.retired_at){owner.chat_id=chat.id;owner.attachment_id=chat.attachment_id;owner.generation=chat.seed_generation??1;}}}
    if(s.chat_id){const chat=chats.find(c=>c.id===s.chat_id);if(!chat||chat.closed||!chat.seed_auto||chat.seed_owner!==s.id||chat.attachment_id!==s.attachment_id){retirement(s,'conversation_detached',now);saveSeeds(store,data);return {status:'retired'};}}
    if(data.enabled&&!s.chat_id){
      for(const chat of chats.sort((a,b)=>a.created_at-b.created_at)){
        if(!eligibleChat(chat))continue;
        const old=chat.seed_owner?data.seeds.find(seed=>seed.id===chat.seed_owner):undefined;
        if(old){
          if(old.inflight.length||old.operations.length)continue;
          if(!old.retired_at&&(now-old.last_seen<=SEED_REPLACE_MS||chat.lease_until>now))continue;
          if(!old.retired_at)retirement(old,'activity_timeout',now);
        } else if(chat.attachment_id)continue; // Never take over a manually connected AI.
        saveSeeds(store,data); // Revoke old identity before writing the new owner.
        const attachment=randomUUID();chat.seed_owner=s.id;chat.seed_generation=(chat.seed_generation??0)+1;chat.attachment_id=attachment;chat.lease_until=0;chat.seed_pending=true;chat.agent_name=`Seed-${s.id.slice(0,8)}`;chat.updated_at=now;
        store.save(chat);s.chat_id=chat.id;s.attachment_id=attachment;s.generation=chat.seed_generation;break;
      }
    }
    if(JSON.stringify(data)!==before)saveSeeds(store,data);
    return s.chat_id?{status:'assigned',chat_id:s.chat_id,attachment_id:s.attachment_id,generation:s.generation,instruction:'Call chat_open with this chat_id and attachment_id. Read the complete returned skill and history. Preserve the existing pending message, plan and user constraints; inspect files and prior operation results before resuming. Work through this MCP workspace. Do not replay completed actions or use an old host sandbox as the project source.'}:{status:'idle',instruction:'Make one new independent seed_wait(timeout_ms:25000). Idle is normal. Respect host execution limits.'};
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
