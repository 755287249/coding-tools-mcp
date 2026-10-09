import {randomUUID} from 'node:crypto';
import {redactSensitiveText} from '../redaction.js';
import {chatOperationOwner, writeChatOperation} from './store.js';

export interface ChatOperation {
  id:string; reply_to:string; agent_id?:string; agent_name:string; tool:string;
  kind:'read'|'search'|'edit'|'exec'|'other'; status:'running'|'completed'|'failed'|'interrupted';
  started_at:number; duration_ms?:number; paths:string[]; input:string; output?:string;
  diff?:string; truncated?:boolean; dry_run:boolean;
}
interface Binding {chat_id:string;attachment_id:string;reply_to?:string}
// Connection identity is supplied by the transport, never inferred from timestamps.
// Multiple live attachments on one identity are ambiguous, so record neither.
const bindings=new Map<string,Binding[]>();
const bindingKey=(root:string,key:string)=>JSON.stringify([root,key]);
const clean=(value:unknown,max=2000)=>redactSensitiveText(typeof value==='string'?value:JSON.stringify(value)??'').value.slice(0,max);
export function bindChatOperations(root:string,key:string|undefined,name:string,args:Record<string,unknown>,result:Record<string,unknown>):void {
  if(!key||result.ok!==true||!['chat_open','chat_wait','chat_close'].includes(name))return;
  const id=String(args.chat_id??'');const attachment=String(result.attachment_id??args.attachment_id??'');
  const k=bindingKey(root,key);let entries=bindings.get(k)??[];
  entries=entries.filter(b=>chatOperationOwner(root,b.chat_id,b.attachment_id)!==undefined);
  if(name==='chat_close')entries=entries.filter(b=>b.chat_id!==id);
  else if(attachment){
    let b=entries.find(b=>b.chat_id===id&&b.attachment_id===attachment);
    if(!b){b={chat_id:id,attachment_id:attachment};entries.push(b);}
    if(name==='chat_wait')b.reply_to=result.status==='message'?String((result.message as {id:string}).id):undefined;
  }
  bindings.delete(k);bindings.set(k,entries);
  while(bindings.size>256)bindings.delete(bindings.keys().next().value!);
}
function collectPaths(value:unknown,paths:string[],depth=0):void {
  if(depth>5||paths.length>=24||!value||typeof value!=='object')return;
  const add=(v:unknown)=>{if(typeof v==='string'&&v.trim()&&paths.length<24){const p=clean(v,1000);if(!paths.includes(p))paths.push(p);}};
  if(Array.isArray(value)){for(const item of value)collectPaths(item,paths,depth+1);return;}
  for(const [key,item] of Object.entries(value)){
    if(['path','destination','source','file','workdir','cwd'].includes(key))add(item);
    else if(['paths','files','files_changed'].includes(key)&&Array.isArray(item))for(const p of item){add(p);collectPaths(p,paths,depth+1);}
    else if(key==='patch'&&typeof item==='string')for(const line of item.split('\n')){const match=line.match(/^(?:\*\*\* (?:Update File|Add File|Delete File|Move to): |\+\+\+ b\/)(.+)$/);if(match)add(match[1]);}
    else if(!['content','text','output','stdout','stderr','diff'].includes(key))collectPaths(item,paths,depth+1);
  }
}
function kind(tool:string):ChatOperation['kind'] {
  if(/^(read_|list_files|view_image)/.test(tool))return 'read';
  if(/search|grep|glob/.test(tool))return 'search';
  if(/file_ops|patch|edit|write|format_files/.test(tool))return 'edit';
  if(/exec|command|send_input|kill_session/.test(tool))return 'exec';
  return 'other';
}
export function beginChatOperation(root:string,key:string|undefined,tool:string,args:Record<string,unknown>) {
  if(!key||tool.startsWith('chat_')||['set_todos','update_plan','report_progress'].includes(tool))return;
  const k=bindingKey(root,key);if(!bindings.has(k))return;const entries=(bindings.get(k)??[]).filter(b=>chatOperationOwner(root,b.chat_id,b.attachment_id));bindings.set(k,entries);
  if(entries.length!==1)return;
  const b=entries[0];if(!b.reply_to)return;
  const actor=chatOperationOwner(root,b.chat_id,b.attachment_id,b.reply_to);if(!actor)return;
  const paths:string[]=[];collectPaths(args,paths);
  const input=clean(Object.fromEntries(Object.entries(args).filter(([k])=>['cmd','command','program','args','script','pattern','query','start_line','end_line','dry_run'].includes(k))));
  const event:ChatOperation={id:randomUUID(),reply_to:b.reply_to,...actor,tool,kind:kind(tool),status:'running',started_at:Date.now(),paths,input:input==='{}'?'':input,dry_run:args.dry_run===true};
  let warning:string|undefined;
  try{writeChatOperation(root,b.chat_id,event);}catch{warning='Operation log could not be saved; do not retry the tool solely for this warning.';}
  return {event,finish(result:Record<string,unknown>,interrupted=false):string|undefined {
    event.duration_ms=Math.max(0,Date.now()-event.started_at);
    event.status=interrupted?'interrupted':result.ok===false||result.isError===true||typeof result.exit_code==='number'&&result.exit_code!==0?'failed':result.status==='running'||result.status==='background'?'running':'completed';
    event.dry_run ||= result.dry_run===true;
    collectPaths({affected_files:result.affected_files,files_changed:result.files_changed},event.paths);
    const summary=Object.fromEntries(Object.entries(result).filter(([k])=>['status','exit_code','error','message','bytes_read','total_lines','affected_files','files_changed','session_id','operation_id','stdout','stderr','output'].includes(k)));
    const output=clean(summary,4000);event.output=output==='{}'?'':output;
    if(typeof result.diff==='string'){event.diff=clean(result.diff,12000);event.truncated=result.diff.length>12000||result.diff_truncated===true;}
    event.truncated ||= JSON.stringify(summary).length>4000;
    try{writeChatOperation(root,b.chat_id,event);}catch{warning='Operation log could not be saved; do not retry the tool solely for this warning.';}
    return warning;
  }};
}
export function operationsMarkdown(events:ChatOperation[]):string {
  return '# MCP operations\n\nBounded recent operation history; running is not proof of completion.\n\n'+events.map(e=>`## ${e.agent_name} · ${e.tool} · ${e.status}\n\nRequest: ${e.reply_to}\nTime: ${e.started_at}\n${e.paths.join('\n')}\n${e.dry_run?'Dry run\n':''}${e.input}\n${e.output??''}\n${e.diff??''}\n${e.truncated?'Details truncated\n':''}`).join('\n');
}
