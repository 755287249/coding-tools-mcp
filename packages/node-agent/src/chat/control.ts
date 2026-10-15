// User control over work in progress: recall, pause, jump a queued correction ahead.
// Mirrors src-tauri/src/tools/chat_control.rs; stored fields are shared.
import type {ChatMessage,ChatSession} from './store.js';

export const REASON_PAUSE='pause';
export const REASON_CORRECTION='correction';
export const RECALLED_TEXT='（用户已撤回此消息）';
export const LATE_RECALLED_INSTRUCTION='The user recalled this message; the reply was not saved. Do not continue that work. Call chat_wait now for the next message.';
export const LATE_INTERRUPTED_INSTRUCTION="Saved. The user paused or replaced this task; do not continue it. Call chat_wait now for the user's next message.";

type Message=ChatMessage&{recalled_at?:number;interrupted_at?:number;interrupt_reason?:string;stop_acknowledged_at?:number};
export const stopped=(m:Message)=>typeof m.recalled_at==='number'||typeof m.interrupted_at==='number';

export interface ControlHelpers {
  grouped(s:ChatSession):boolean;
  pending(s:ChatSession):ChatMessage|undefined;
  fingerprint(text:string,attachments:ChatMessage['attachments']):string;
}

function answered(s:ChatSession,id:string){return s.messages.some(r=>r.role==='assistant'&&r.reply_to===id&&r.final===true);}
function markInterrupted(s:ChatSession,id:string,reason:string,time:number){
  const m=s.messages.find(item=>item.id===id) as Message|undefined;
  if(m&&m.interrupted_at===undefined){m.interrupted_at=time;m.interrupt_reason=reason;}
}
function resetDerivedTitle(s:ChatSession,text:string){
  const derived=[...text.split(/\s+/).filter(Boolean).join(' ')].slice(0,36).join('');
  if(!s.title_custom&&!s.title_agent_name&&s.title===derived)s.title='新对话';
}

/** UI actions `recall`, `interrupt`, `jump_queue`; the caller saves the session. Returns the recalled text. */
export function controlUi(s:ChatSession,action:string,messageId:string|undefined,h:ControlHelpers):{text:string;attachments?:ChatMessage['attachments']}|null {
  let recalled:{text:string;attachments?:ChatMessage['attachments']}|null=null;
  if(s.closed)throw Error('Conversation is closed');
  if(h.grouped(s))throw Error('Recall and pause are only available in work chats');
  const time=Date.now();
  if(action==='recall'){
    const index=s.queue?.findIndex(m=>m.id===messageId)??-1;
    if(index>=0){
      const message=s.queue![index];
      if(message.discussion)throw Error('Collaboration deliveries cannot be recalled');
      Object.defineProperty(s.queue_receipts??={},messageId!,{value:h.fingerprint(message.text,message.attachments??[]),enumerable:true,writable:true,configurable:true});
      recalled={text:message.text,attachments:message.attachments};
      s.queue!.splice(index,1);
    }else{
      const message=s.messages.find(m=>m.id===messageId) as Message|undefined;
      if(!message)throw Error('Message not found');
      if(message.role!=='user'||message.kind==='connection_request')throw Error('Only your own messages can be recalled');
      if(answered(s,message.id))throw Error('The AI already answered this message');
      message.recalled_at??=time;
      recalled={text:message.text,attachments:message.attachments};
      resetDerivedTitle(s,message.text);
    }
  }else if(action==='interrupt'){
    const current=h.pending(s);
    if(!current)throw Error('No task is in progress');
    if(current.kind==='connection_request')throw Error('Wait until the AI has connected');
    if(messageId!==undefined&&current.id!==messageId)throw Error('The task changed; refresh and try again');
    if(typeof current.received_at!=='number')throw Error('The AI has not picked up this message yet; recall it instead');
    markInterrupted(s,current.id,REASON_PAUSE,time);
    s.queue_paused=true;
  }else if(action==='jump_queue'){
    const index=s.queue?.findIndex(m=>m.id===messageId)??-1;
    if(index<0)throw Error('Message is no longer queued');
    if(s.queue![index].discussion)throw Error('Collaboration deliveries cannot jump the queue');
    const current=h.pending(s);
    if(current?.kind==='connection_request')throw Error('Wait until the AI has connected');
    const message={...s.queue!.splice(index,1)[0],created_at:time};
    delete s.queue_paused;
    if(current&&typeof current.received_at==='number'){markInterrupted(s,current.id,REASON_CORRECTION,time);s.messages.push(message);}
    else if(current)s.messages.splice(s.messages.indexOf(current),0,message);
    else s.messages.push(message);
  }else throw Error('Unknown chat action');
  s.updated_at=time;
  return recalled;
}

/** The stopped message the AI may still be working on, as a tool error result. */
export function stopNotice(s:ChatSession,grouped:boolean):Record<string,unknown>|undefined {
  if(s.closed||grouped)return undefined;
  const m=[...s.messages].reverse().find(item=>{const x=item as Message;return x.role==='user'&&typeof x.received_at==='number'&&stopped(x)&&x.stop_acknowledged_at===undefined;}) as Message|undefined;
  if(!m)return undefined;
  const recalled=typeof m.recalled_at==='number',correction=m.interrupt_reason===REASON_CORRECTION;
  const message=recalled
    ?'用户撤回了你正在处理的消息。立即停止这项工作，不要回复它，也不要继续相关工具调用；马上调用 chat_wait 等待用户的新消息，会话保持连接。The user recalled the message you were working on: stop now and call chat_wait.'
    :correction
      ?'用户发来了纠正，替换了你正在处理的任务。立即停止当前工作，可用一条简短 chat_reply 说明已停在哪里，然后马上调用 chat_wait 接收纠正内容。The user sent a correction: stop and call chat_wait to receive it.'
      :'用户请你暂停（稍等）。立即停止当前工作，可用一条简短 chat_reply 说明进度和停在哪里，然后马上调用 chat_wait 等待用户的下一条消息；不要结束会话。The user paused you: stop, optionally post a short status reply, then call chat_wait.';
  return {ok:false,error:{code:recalled?'USER_RECALLED':correction?'USER_CORRECTION':'USER_PAUSED',category:'user_control',retryable:false,message_id:m.id,message}};
}

export function acknowledge(s:ChatSession):void {
  const time=Date.now();
  for(const item of s.messages){const m=item as Message;if(m.role==='user'&&typeof m.received_at==='number'&&stopped(m)&&m.stop_acknowledged_at===undefined)m.stop_acknowledged_at=time;}
}

export function lateReply(s:ChatSession,replyTo:string):'recalled'|'interrupted'|undefined {
  const m=s.messages.find(item=>item.id===replyTo&&item.role==='user') as Message|undefined;
  if(!m)return undefined;
  if(typeof m.recalled_at==='number')return 'recalled';
  // Only the most recently stopped task the AI received may get a closing status reply.
  const latest=[...s.messages].reverse().find(item=>{const x=item as Message;return x.role==='user'&&typeof x.received_at==='number'&&typeof x.interrupted_at==='number';});
  return latest?.id===replyTo?'interrupted':undefined;
}

/** Recalled text never reaches the model, in excerpts or archives. */
export function redact<T extends {text?:unknown;attachments?:unknown;recalled_at?:unknown}>(m:T):T {
  if(typeof m.recalled_at!=='number')return m;
  const {attachments:_dropped,...rest}=m;
  return {...rest,text:RECALLED_TEXT} as T;
}
