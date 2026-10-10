import {randomUUID} from 'node:crypto';
import {redactSensitiveText} from '../redaction.js';
import type {ChatSession, ChatMessage} from './store.js';
export interface ChatMember {paused?:boolean;id:string;name:string;role:'coordinator'|'member';attachment_id:string;lease_until:number}
const lease=600_000;
export const grouped=(s:ChatSession)=>s.mode==='group';
export const members=(s:ChatSession)=>s.members??[];
export function memberName(value:unknown):string {
 if(typeof value!=='string'||!value.trim()||Buffer.byteLength(value)>80)throw new Error('Invalid agent name');
 const name=redactSensitiveText(value.trim()).value;
 if(/[\s@\[\]<>\x00-\x1f\x7f]/u.test(name))throw new Error('Agent name must be 1–80 bytes without spaces, @ or markup');
 return name;
}
export function memberFor(s:ChatSession,attachment:unknown,expired=false):ChatMember {
 const m=members(s).find(m=>!!attachment&&m.attachment_id===attachment);
 if(m?.paused)throw new Error('Group member is paused; resume it in the local UI');
 void expired;// Keepalive: a member's own attachment_id stays valid after its lease lapses.
 if(!m)throw new Error('Chat attachment expired; call chat_open again with the saved attachment_id');
 return m;
}
export function renew(s:ChatSession,attachment:unknown):void {if(grouped(s))memberFor(s,attachment).lease_until=Date.now()+lease;else s.lease_until=Date.now()+lease;}
export function openGroup(s:ChatSession,args:Record<string,unknown>):ChatMember {
 if(args.attachment_id){const m=memberFor(s,args.attachment_id,true);m.lease_until=Date.now()+lease;return m;}
 const name=memberName(args.agent_name);
 if(members(s).some(m=>m.name===name))throw new Error('Agent name already exists; resume with its saved attachment_id or choose another name');
 if(members(s).length>=16)throw new Error('Group member limit is 16');
 const m:ChatMember={id:randomUUID(),name,role:members(s).length?'member':'coordinator',attachment_id:randomUUID(),lease_until:Date.now()+lease};
 (s.members??=[]).push(m);return m;
}
export function taskComplete(s:ChatSession,m:ChatMessage):boolean {
 const replies=s.messages.filter(r=>r.role==='assistant'&&r.reply_to===m.id&&r.final===true);
 if(!m.recipient_ids?.length)return replies.length>0;
 return m.recipient_ids.every(id=>replies.some(r=>r.agent_id===id));
}
export function groupPending(s:ChatSession,agent?:string,waiting=false):ChatMessage|undefined {
 return s.messages.find(m=>{
  if(!agent)return m.role==='user'&&!taskComplete(s,m);
  if(waiting&&m.role==='user'&&members(s).some(p=>p.id===agent&&p.role==='coordinator')&&s.messages.some(child=>child.kind==='assignment'&&child.reply_to===m.id&&!taskComplete(s,child)))return false;
  const targets=m.recipient_ids?.length?m.recipient_ids:[members(s).find(p=>p.role==='coordinator')?.id];
  return (m.role==='user'||m.kind==='assignment')&&targets.includes(agent)&&!s.messages.some(r=>r.role==='assistant'&&r.reply_to===m.id&&r.agent_id===agent&&r.final===true)&&!(!m.recipient_ids?.length&&taskComplete(s,m));
 });
}
export function targetUser(s:ChatSession,m:ChatMessage):void {
 if(!grouped(s))return;
 const chief=members(s).find(p=>p.role==='coordinator');
 if(!chief)return; // First join binds queued requests, keeping the UI usable before connection.
 const tokens=new Set(m.text.split(/\s+/));
 m.recipient_ids=[chief.id,...members(s).filter(p=>p.id!==chief.id&&tokens.has('@'+p.name)).map(p=>p.id)];
}
export function bindTargets(s:ChatSession):void {
 if(grouped(s))for(const m of s.messages)if(m.role==='user'&&!m.recipient_ids?.length&&!taskComplete(s,m))targetUser(s,m);
}
export function replyIdentity(s:ChatSession,args:Record<string,unknown>,final:boolean):Partial<ChatMessage> {
 if(!grouped(s)){if(args.recipient_ids!==undefined)throw new Error('Assignments require group mode');return {};}
 const actor=memberFor(s,args.attachment_id);
 const result:Partial<ChatMessage>={agent_id:actor.id,agent_name:actor.name};
 if(args.recipient_ids!==undefined){
  const ids=args.recipient_ids;
  if(actor.role!=='coordinator'||final||!Array.isArray(ids)||!ids.length||new Set(ids).size!==ids.length||ids.some(id=>typeof id!=='string'||id===actor.id||!members(s).some(m=>m.id===id)))throw new Error('Only the coordinator can assign unique other members with final=false');
  result.recipient_ids=ids as string[];result.kind='assignment';
 }
 return result;
}
export function validateFinal(s:ChatSession,args:Record<string,unknown>):void {
 if(!grouped(s)||args.final===false)return;
 const actor=memberFor(s,args.attachment_id);
 if(args.awaiting_user===true&&actor.role!=='coordinator')throw new Error('Members should report questions to the coordinator');
 if(actor.role==='coordinator'){
  const parent=s.messages.find(m=>m.id===args.reply_to);
  const unfinished=s.messages.some(m=>m.kind==='assignment'&&m.reply_to===args.reply_to&&!taskComplete(s,m));
  const others=parent?.recipient_ids?.some(id=>id!==actor.id&&!s.messages.some(r=>r.reply_to===parent.id&&r.agent_id===id&&r.final===true));
  if(unfinished||others)throw new Error('Wait for assigned members to finish before finalizing the shared task');
 }
}
export function setMode(s:ChatSession,value:unknown):void {
 if(value!=='group'&&value!=='work')throw new Error('Invalid conversation mode');
 if((s.mode??'work')===value)return;
 if(s.closed)throw new Error('Conversation is closed');
 if([...s.messages,...s.queue??[]].some(m=>m.role==='user'&&m.kind!=='connection_request'))throw new Error('Conversation mode is fixed after the first user message');
 if(value==='group'){
  s.version=2;s.mode='group';s.members=[];
  if(s.attachment_id){s.members.push({id:s.work_member?.attachment_id===s.attachment_id?s.work_member.id:randomUUID(),name:s.agent_name??'AI',role:'coordinator',attachment_id:s.attachment_id,lease_until:s.lease_until});}
  const chief=s.members[0];if(chief)for(const m of s.messages)if(m.role==='assistant'&&!m.agent_id){m.agent_id=chief.id;m.agent_name=chief.name;}
  s.attachment_id='';s.lease_until=0;bindTargets(s);
 }else{
  const active=members(s).filter(m=>!m.paused);
  if(active.length>1||active.some(m=>m.role!=='coordinator'))throw new Error('Disconnect assisting members before switching to work');
  const chief=members(s).find(m=>m.role==='coordinator'&&!m.paused);
  const unfinished=s.messages.filter(m=>(m.role==='user'||m.kind==='assignment')&&!taskComplete(s,m));
  const needsOthers=(m:ChatMessage)=>m.recipient_ids?.some(id=>id!==chief?.id);
  if(unfinished.some(m=>m.kind==='assignment'||needsOthers(m))||(s.queue??[]).some(needsOthers))throw new Error('Finish assigned member tasks and queued member requests before switching to work');
  // Work replies have no group identity. Rebind only unfinished requests on the next group entry.
  for(const m of [...unfinished,...s.queue??[]])delete m.recipient_ids;
  s.work_member=chief;
  s.attachment_id=chief?.attachment_id??'';s.lease_until=chief?.lease_until??0;s.agent_name=chief?.name;s.mode='work';s.members=[];
 }
}
export function groupUi(s:ChatSession,args:Record<string,unknown>):void {
 if(args.action==='set_mode'){setMode(s,args.mode);return;}
 const m=members(s).find(m=>m.id===args.member_id);if(!m)throw new Error('Group member not found');
 if(args.action==='rename_member'){
  const name=memberName(args.name);if(members(s).some(p=>p.id!==m.id&&p.name===name))throw new Error('Agent name already exists');m.name=name;
 }else if(args.action==='detach_member'){m.lease_until=0;m.paused=true;}
 else if(args.action==='resume_member'){m.paused=false;}
 else if(args.action==='set_coordinator'){
  if(groupPending(s)||s.queue?.length)throw new Error('Finish pending tasks before changing coordinator');
  for(const p of members(s))p.role=p.id===m.id?'coordinator':'member';
 }
}
export function groupMarkdown(s:ChatSession,m?:ChatMessage):string {
 if(m)return (m.agent_name?`\nAgent: ${m.agent_name} (${m.agent_id})\n`:'')+(m.recipient_ids?.length?`\nRecipients: ${m.recipient_ids.map(id=>members(s).find(p=>p.id===id)?.name??id).join(', ')}\n`:'')+(m.agent_plans??[]).map(p=>`\n### ${members(s).find(m=>m.id===p.agent_id)?.name??p.agent_id}\n${p.plan.goal}\n${p.plan.todos.map(t=>`- [${t.status==='completed'?'x':' '}] ${t.title} (${t.status})`).join('\n')}\n${p.plan.progress?.message??''}\n`).join('');
 return grouped(s)?`Mode: group\n\n${members(s).map(p=>`- ${p.name} · ${p.role} (${p.id})`).join('\n')}\n\n`:'';
}
