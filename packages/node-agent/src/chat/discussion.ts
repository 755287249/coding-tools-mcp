import {configureCollaboration,collaborationTargets,collaborationContext,collaborationMarkdown} from './collaboration.js';
import {createHash,randomUUID} from 'node:crypto';
import {existsSync,mkdirSync,readFileSync,readdirSync,renameSync,rmSync,statSync,writeFileSync} from 'node:fs';
import type {ChatSession,ChatMessage,ChatFile} from './store.js';

export interface DiscussionStore {
  safe(relative:string):string; load(id:string):ChatSession; save(session:ChatSession):void;
  validateId(value:unknown):string; text(value:unknown,max?:number,preserve?:boolean):string;
}
export interface DiscussionDelivery {chat_id:string;message_id:string;title:string;status:string;replies:ChatMessage[];error?:string}
export interface DiscussionPost {attachments?:ChatFile[];attachment_chat_id?:string;summaries?:ChatMessage[];id:string;from:string;name:string;text:string;goal:string;purpose:string;created_at:number;targets:string[];deliveries:DiscussionDelivery[]}
export interface Discussion {files?:ChatFile[];paused?:boolean;pinned?:boolean;collaboration?:boolean;coordinator_chat_id?:string;aliases?:Record<string,string>;version:1;id:string;name:string;goal:string;members:string[];archived:boolean;created_at:number;updated_at:number;posts:DiscussionPost[]}
const DIR='docs/chat-sessions/discussions';
const LIMIT=8*1024*1024;
const digest=(s:string)=>createHash('sha256').update(s).digest('hex');
function read(io:DiscussionStore,id:unknown):Discussion {
  const file=io.safe(`${DIR}/${io.validateId(id)}.json`);
  if(statSync(file).size>LIMIT)throw Error('Discussion archive exceeds size limit');
  const d=JSON.parse(readFileSync(file,'utf8')) as Discussion;
  if(d.version!==1||d.id!==id||!Array.isArray(d.posts)||!Array.isArray(d.members))throw Error('Invalid discussion archive');
  return d;
}
function write(io:DiscussionStore,d:Discussion){
  const bytes=JSON.stringify(d,null,2);
  if(Buffer.byteLength(bytes)>LIMIT||d.posts.length>1000)throw Error('Discussion is full; archive it and create another group');
  mkdirSync(io.safe(DIR),{recursive:true});
  const temp=io.safe(`${DIR}/${d.id}.${randomUUID()}.tmp`);
  try{writeFileSync(temp,bytes,{mode:0o600,flag:'wx'});renameSync(temp,io.safe(`${DIR}/${d.id}.json`));}finally{rmSync(temp,{force:true})}
  if(d.collaboration){const md=io.safe(`${DIR}/${d.id}.${randomUUID()}.tmp`);try{writeFileSync(md,collaborationMarkdown(d),{mode:0o600,flag:'wx'});renameSync(md,io.safe(`${DIR}/${d.id}.md`));}finally{rmSync(md,{force:true})}}
}
function all(io:DiscussionStore):Discussion[]{
  if(!existsSync(io.safe(DIR)))return [];
  return readdirSync(io.safe(DIR)).filter(n=>/^[a-zA-Z0-9_-]{1,80}\.json$/.test(n)).map(n=>read(io,n.slice(0,-5)));
}
function members(io:DiscussionStore,value:unknown):string[]{
  if(!Array.isArray(value)||!value.length||value.length>16)throw Error('Choose 1–16 conversations');
  const ids=value.map(id=>io.validateId(id));
  if(new Set(ids).size!==ids.length)throw Error('Duplicate conversation');
  for(const id of ids){const s=io.load(id);if(s.closed||s.archived||s.mode==='group')throw Error('Choose active independent conversations');}
  return ids;
}
function projection(io:DiscussionStore,d:Discussion,offset:unknown=0){
  const start=Number(offset);if(!Number.isInteger(start)||start<0||start>1000)throw Error('Invalid history offset');
  const end=Math.max(0,d.posts.length-start),begin=Math.max(0,end-50);
  const {files,...publicGroup}=d;
  return {...publicGroup,archive_path:`${DIR}/${d.id}.md`,posts:d.posts.slice(begin,end),next_offset:begin?start+end-begin:null,total:d.posts.length,
    member_details:d.members.map(id=>{try{const s=io.load(id),pending=s.messages.find(m=>m.role==='user'&&!s.messages.some(r=>r.role==='assistant'&&r.reply_to===m.id&&r.final)),replies=pending?s.messages.filter(r=>r.role==='assistant'&&r.reply_to===pending.id):[];return {id,title:d.aliases?.[id]??s.agent_name??s.title,note:s.note??'',busy:!!pending&&(!!pending.received_at||replies.length>0),error:replies.filter(r=>r.tool_event).at(-1)?.tool_event?.status==='failed',status:s.closed?'closed':s.lease_until>Date.now()?'connected':'offline'}}catch{return {id,title:id,status:'missing'}}})};
}
function collect(io:DiscussionStore,d:Discussion){
  const before=JSON.stringify(d.posts);
  for(const post of d.posts)for(const delivery of post.deliveries){
    if(delivery.status==='completed')continue;
    try{
      const s=io.load(delivery.chat_id),m=[...s.messages,...(s.queue??[])].find(m=>m.id===delivery.message_id);
      if(!m){delivery.status=s.closed?'closed':'undelivered';continue;}
      delivery.replies=s.messages.filter(r=>r.role==='assistant'&&r.reply_to===m.id);
      const final=delivery.replies.filter(r=>r.final).at(-1);
      delivery.status=final?(final.awaiting_user?'awaiting_user':'completed'):s.closed?'closed':m.received_at?'processing':'queued';
      delete delivery.error;
    }catch{delivery.status='unavailable';delivery.error='Conversation unavailable; existing results retained';}
  }
  if(d.collaboration)for(const post of d.posts.filter(p=>p.from!=='user')){
    try{const s=io.load(post.from);const ids=new Set(s.messages.filter(m=>m.discussion?.id===d.id&&m.discussion.post_id===post.id&&m.discussion.purpose==='result').map(m=>m.id));post.summaries=s.messages.filter(m=>m.role==='assistant'&&ids.has(m.reply_to??''));}catch{/* Retain collected results if the source is no longer available. */}
  }
  if(before!==JSON.stringify(d.posts)||d.collaboration)write(io,d);
}
function deliver(io:DiscussionStore,d:Discussion,p:DiscussionPost){
  // The durable post is the outbox intent. A retry repairs only missing deliveries.
  for(const delivery of p.deliveries){
    const s=io.load(delivery.chat_id);
    if([...s.messages,...(s.queue??[])].some(m=>m.id===delivery.message_id))continue;
    if(s.closed||s.archived)throw Error('Recipient is closed or archived; existing deliveries are retained');
    const message:ChatMessage={id:delivery.message_id,role:'user',created_at:p.created_at,attachments:p.attachments??[],text:
      `[讨论组：${d.name}]\n目标：${p.goal||'未设置'}\n发送者：${p.name}\n用途：${p.purpose}\n群 ID：${d.id}\n任务/消息 ID：${p.id}\n\n${p.text}\n\n请用 chat_reply 回复本条实际消息 ID；结果会关联回讨论组。需要明确向其他成员发言时使用 chat_discuss。`+collaborationContext(d,p),
      discussion:{hidden:d.collaboration===true,id:d.id,post_id:p.id,source_chat_id:p.from,purpose:p.purpose}};
    (s.queue??=[]).push(message);s.updated_at=Date.now();io.save(s);
  }
}
/** An independent file manifest reuses the normal transfer engine without creating a work chat. */
export function discussionAttachmentStore(io:DiscussionStore,args:Record<string,unknown>) {
  const chat=String(args.chat_id);
  if(!['upload','upload_chunk','read_attachment','read_attachment_chunk','read_artifact','reveal_path'].includes(String(args.action)))throw Error('Unsupported group attachment action');
  const d=read(io,chat.slice('discussion:'.length));
  const session:ChatSession={version:1,id:'discussion-'+digest(d.id),title:d.name,created_at:d.created_at,updated_at:d.updated_at,closed:!!d.archived||!!d.paused,messages:[],files:d.files??[],attachment_id:'',lease_until:0};
  return {session,save:(s:ChatSession)=>{d.files=s.files;write(io,d)}};
}
function postFiles(io:DiscussionStore,d:Discussion,args:Record<string,unknown>,actor?:ChatSession):ChatFile[]{
  if(args.attachment_ids===undefined)return [];
  const ids=args.attachment_ids;
  if(!Array.isArray(ids)||new Set(ids).size!==ids.length)throw Error('Select unique attachments');
  return ids.map(id=>{const valid=io.validateId(id),f=(actor?.files??(actor?[]:d.files)??[]).find(f=>f.id===valid);if(!f)throw Error('Attachment does not belong to this conversation');return f;});
}
export function discussionAction(io:DiscussionStore,args:Record<string,unknown>,actor?:ChatSession):Record<string,unknown>{
  const action=String(args.action??'list').replace(/^discussion_/,'');
  if(action==='list')return {discussions:all(io).filter(d=>!actor||d.members.includes(actor.id)).map(({posts,files,...d})=>({...d,total:posts.length})).sort((a,b)=>b.updated_at-a.updated_at)};
  if(action==='create'){
    if(actor)throw Error('Create groups from the local interface');
    const gid=io.validateId(args.discussion_id),name=io.text(args.title,160),goal=args.goal?io.text(args.goal,2000):'',ids=members(io,args.member_chat_ids);
    if(existsSync(io.safe(`${DIR}/${gid}.json`))){const old=read(io,gid);if(old.name!==name||old.goal!==goal||JSON.stringify(old.members)!==JSON.stringify(ids)||!!old.collaboration!==(args.collaboration===true)||args.coordinator_chat_id!==undefined&&old.coordinator_chat_id!==args.coordinator_chat_id)throw Error('Group ID conflicts');return {discussion:projection(io,old)};}
    const now=Date.now(),d:Discussion={version:1,id:gid,name,goal,members:ids,archived:false,created_at:now,updated_at:now,posts:[],collaboration:args.collaboration===true};configureCollaboration(io,d,args);write(io,d);return {discussion:projection(io,d)};
  }
  const d=read(io,args.discussion_id);
  if(actor&&!d.members.includes(actor.id))throw Error('This conversation is not a discussion member');
  if(action==='update'){
    if(actor)throw Error('Manage groups from the local interface');
    if(args.title!==undefined)d.name=io.text(args.title,160);
    if(args.goal!==undefined)d.goal=args.goal?io.text(args.goal,2000):'';
    if(args.member_chat_ids!==undefined)d.members=members(io,args.member_chat_ids);
    if(args.archived!==undefined){if(typeof args.archived!=='boolean')throw Error('archived must be boolean');d.archived=args.archived;}
    for(const field of ['paused','pinned'] as const)if(args[field]!==undefined){if(typeof args[field]!=='boolean')throw Error(`${field} must be boolean`);d[field]=args[field];}
    configureCollaboration(io,d,args);d.updated_at=Date.now();write(io,d);
  }else if(action==='post'){
    const attachments=postFiles(io,d,args,actor);
    const id=io.validateId(args.message_id),content=io.text(args.text||(attachments.length?'📎':''),16000,true),purpose=String(args.purpose??'discussion'),from=actor?.id??'user';
    if(!['discussion','question','notice','task'].includes(purpose))throw Error('Invalid message purpose');
    const targets=args.recipient_chat_ids===undefined?(d.collaboration?collaborationTargets(d,content,from):d.members.filter(id=>id!==from)):args.recipient_chat_ids;
    if(!Array.isArray(targets)||!targets.length||targets.length>16||new Set(targets).size!==targets.length||targets.some(t=>typeof t!=='string'||!d.members.includes(t)||t===from))throw Error('Choose discussion members other than yourself');
    let p=d.posts.find(p=>p.id===id);
    if(p){if(p.from!==from||p.text!==content||p.purpose!==purpose||JSON.stringify(p.attachments??[])!==JSON.stringify(attachments)||JSON.stringify(p.targets)!==JSON.stringify(targets))throw Error('Message ID conflicts with a discussion post');}
    else{
      if(d.archived)throw Error('Discussion is archived');
      if(d.paused)throw Error('Group is disconnected; reconnect from the local interface');
      members(io,targets);
      p={id,from,attachments,attachment_chat_id:actor?.id??'discussion:'+d.id,name:actor?(d.aliases?.[actor.id]??actor.agent_name??actor.title):'你',text:content,goal:d.goal,purpose,targets:targets as string[],created_at:Date.now(),deliveries:targets.map(chat=>({chat_id:chat as string,message_id:'d-'+digest(`${d.id}:${id}:${chat}`),title:io.load(chat as string).title,status:'undelivered',replies:[]}))};
      d.posts.push(p);d.updated_at=Date.now();write(io,d);
    }
    deliver(io,d,p);
  }else if(action!=='read')throw Error('Unknown discussion action');
  collect(io,d);
  return {ok:true,persisted:true,discussion:projection(io,d,args.offset),...(args.message_id?{post_id:args.message_id}:{})};
}
export function discussionInbox(io:DiscussionStore,chatId:string){
  for(const d of all(io)){
    if(!d.posts.some(p=>p.from===chatId&&p.purpose==='task'))continue;
    collect(io,d);
    for(const p of d.posts.filter(p=>p.from===chatId&&p.purpose==='task'))for(const delivery of p.deliveries){
      if(!['completed','awaiting_user','closed','unavailable'].includes(delivery.status))continue;
      const last=delivery.replies.at(-1),id='r-'+digest(`${d.id}:${p.id}:${delivery.chat_id}:${last?.id??delivery.status}`),s=io.load(chatId);
      if(s.closed||[...s.messages,...(s.queue??[])].some(m=>m.id===id))continue;
      (s.queue??=[]).push({id,role:'user',created_at:Date.now(),attachments:last?.attachments??[],text:`[讨论组任务结果] ${d.name}\n任务 ID：${p.id}\n成员：${delivery.title}\n状态：${delivery.status}\n\n${last?.text??'会话不可用，请在讨论组查看状态。'}`,discussion:{hidden:d.collaboration===true,id:d.id,post_id:p.id,source_chat_id:delivery.chat_id,purpose:'result'}});s.updated_at=Date.now();io.save(s);
    }
  }
}
