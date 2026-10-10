import type { IncomingMessage, ServerResponse } from 'node:http';
import { realpathSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import type { ToolContext } from '../types.js';
import { toolNamesForProfile } from '../catalog.js';
import { chatTool } from './store.js';
import { compatGrants } from './compat-grants.js';
import { compatSkill } from './compat-skill.generated.js';

export function checkCompatPolicy(ctx:ToolContext):void {
  const names=toolNamesForProfile(ctx.config.activeToolProfile);
  const hooks=ctx.config.extensions?.hooks;
  if(!['chat_open','chat_wait','chat_reply'].every(n=>names.includes(n)) || (!ctx.config.securityPolicyCustomized && ctx.config.permissionMode === 'read-only') || (hooks?.active && hooks.enabled.length)) {
    throw new Error('GET trial requires chat tools and no enabled hooks');
  }
}
function fragments(text:string):string[] {
  const chars=Array.from(text), result:string[]=[];
  for(let i=0;i<chars.length;i+=120)result.push(chars.slice(i,i+120).join(''));
  return result;
}
function replyData(raw:string):Record<string,unknown> {
  const data:unknown=JSON.parse(raw);
  if(!data||typeof data!=='object'||Array.isArray(data))throw new Error('Invalid reply data');
  const d=data as Record<string,unknown>;
  if(Object.keys(d).some(k=>!['message_id','reply_to','text','final','awaiting_user'].includes(k))||
    !/^[A-Za-z0-9_-]{1,80}$/.test(String(d.message_id??''))||!/^[A-Za-z0-9_-]{1,80}$/.test(String(d.reply_to??''))||
    typeof d.text!=='string'||!d.text.trim()||Buffer.byteLength(d.text)>2000||typeof d.final!=='boolean'||
    (d.awaiting_user!==undefined&&typeof d.awaiting_user!=='boolean'))throw new Error('Invalid reply data');
  return d;
}
export async function handleChatCompat(req:IncomingMessage,res:ServerResponse,url:URL,ctx:ToolContext):Promise<void> {
  res.setHeader('Cache-Control','private, no-store, max-age=0');res.setHeader('Referrer-Policy','no-referrer');
  res.setHeader('X-Content-Type-Options','nosniff');res.setHeader('Content-Type','application/json; charset=utf-8');
  const send=(status:number,body:unknown,key='')=>{res.statusCode=status;const text=JSON.stringify(body,null,2);res.end(key?text.replaceAll(key,'[redacted]'):text);};
  if(req.method!=='GET'){res.setHeader('Allow','GET');send(405,{ok:false,error:'GET required'});return;}
  let key='',held=false;
  let grant:ReturnType<typeof compatGrants.get>|undefined;
  try {
    if((req.url?.length??0)>16384)throw new Error('Request too large');
    const q=url.searchParams;
    const op=q.get('op');
    const allowed=['key','op','nonce',...(op==='reply'?['data']:op==='wait'?['timeout_ms']:[])];
    if([...q.keys()].some(k=>!allowed.includes(k)||q.getAll(k).length!==1)||!q.get('nonce')||q.get('nonce')!.length>80||!['info','open','wait','reply'].includes(op??''))throw new Error('Invalid compatibility request');
    key=q.get('key')??'';
    if(!/^[a-f0-9]{64}$/.test(key))throw new Error('Invalid authorization');
    grant=compatGrants.get(key,ctx.workspaceProfileId);
    const folder=ctx.config.folders.find(f=>f.id===grant!.folder&&realpathSync(f.path)===grant!.root);
    if(!folder)throw new Error('Configured folder changed');
    checkCompatPolicy(ctx);
    if(grant.busy) {send(409,{ok:false,error:'Request already running'},key);return;}
    grant.busy=true;held=true;
    const args:Record<string,unknown>={chat_id:grant.chat,...(grant.attachment?{attachment_id:grant.attachment}:{})};
    // info uses the same atomic scope/revocation guard, without attaching.
    let result:Record<string,unknown>;
    if(op==='info') result=chatTool(grant.root,'compat_info',args,grant.id);
    else if(op==='open') {
      result=chatTool(grant.root,'chat_open',{...args,agent_name:'GetChat'},grant.id);
      grant.attachment=String(result.attachment_id);
      result={ok:true,status:'connected',instruction_lines:compatSkill.split('\n')};
    } else {
      if(!grant.attachment)throw new Error('Call open first');
      if(op==='reply') {
        const data=replyData(q.get('data')??'');
        if(JSON.stringify(data).includes(key))throw new Error('Do not put authorization in replies');
        result=chatTool(grant.root,'chat_reply',{...args,...data},grant.id);
      } else {
        const raw=q.get('timeout_ms')??'10000';
        if(!/^\d{1,5}$/.test(raw)||Number(raw)>10000)throw new Error('timeout_ms must be 0..10000');
        const deadline=Date.now()+Number(raw);
        do {
          compatGrants.get(key,ctx.workspaceProfileId);checkCompatPolicy(ctx);
          if(req.destroyed)throw new Error('Download disconnected');
          result=chatTool(grant.root,'chat_wait',args,grant.id);
          if(result.status!=='idle'||Date.now()>=deadline)break;
          await sleep(Math.min(250,Math.max(0,deadline-Date.now())));
        } while(true);
        const m=result.message as Record<string,unknown>|undefined;
        result={ok:true,status:result.status,...(m?{message:{id:m.id,kind:m.kind,text_lines:fragments(String(m.text)),attachments:m.attachments??[]}}:{})};
      }
    }
    send(200,{...result,protocol:'chat-get-v1',chat_id:grant.chat,workspace_folder:{id:folder.id,name:folder.name,path:folder.path},expires_at:grant.expires_at},key);
  } catch(error) {
    // Never echo URLs, query values, stack traces or unexpected storage errors.
    const message=error instanceof Error?error.message:'';
    const safe=/^(Compatibility |GET trial |Conversation |Call open|Invalid |Unsupported |Request too large|Configured folder|timeout_ms|Message ID conflicts|Reply must|Do not put)/.test(message)?message:'Compatibility operation failed; inspect the conversation in the client';
    send(400,{ok:false,error:safe},key);
  } finally {if(held&&grant)grant.busy=false;}
}
