import type { ChatAction, ChatFile, ChatMessage, ChatResult } from '../api/chat';
type Request=(args:ChatAction)=>Promise<ChatResult>;
export function messagePreview(text:string):string {
  const characters=Array.from(text);
  if(characters.length<=700&&text.split('\n').length<=14)return text;
  return characters.slice(0,600).join('').split('\n').slice(0,12).join('\n').trimEnd()+'…';
}
export function messageClipboardText(message:ChatMessage):string {
  return [message.text,...(message.attachments??[]).map(file=>`${file.label?'@'+file.label+' ':''}${file.name}\n${file.path}`)].filter(Boolean).join('\n\n');
}
export interface MessageTransfer { messageId:string; uploads:Map<string,string>; files:Map<string,ChatFile> }
export function createMessageTransfer(id:()=>string):MessageTransfer {return {messageId:id(),uploads:new Map(),files:new Map()};}
function remapText(text:string,files:ChatFile[],transferred:ChatFile[]):string {
  const replacements=new Map<string,string>();
  files.forEach((file,index)=>{const next=transferred[index];replacements.set(file.path,next.path);if(file.label&&next.label)replacements.set('@'+file.label,'@'+next.label);});
  if(!replacements.size)return text;
  const escape=(s:string)=>s.replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
  const pattern=[...replacements.keys()].sort((a,b)=>b.length-a.length).map(key=>escape(key)+(key.startsWith('@')?'(?![0-9])':''));
  return text.replace(new RegExp(pattern.join('|'),'g'),value=>replacements.get(value)??value);
}
/** Chunked transfer avoids loading large attachments into browser memory. */
export async function forwardMessage(message:ChatMessage,sourceChat:string,targetChat:string,source:Request,target:Request,state:MessageTransfer,id:()=>string):Promise<ChatResult> {
  if(new TextEncoder().encode(message.text).length>32000)throw Error('Message exceeds 32000 bytes');
  const files=message.attachments??[], transferred:ChatFile[]=[];
  for(const file of files){
    let complete=state.files.get(file.id);
    if(!complete){
      if(!Number.isSafeInteger(file.size)||file.size<=0)throw Error('Invalid attachment size');
      let uploadId=state.uploads.get(file.id);if(!uploadId){uploadId=id();state.uploads.set(file.id,uploadId);}
      for(let offset=0;offset<file.size;){
        const read=await source({action:'read_attachment_chunk',chat_id:sourceChat,upload_id:file.id,offset});
        const bytes=read.data_base64?atob(read.data_base64).length:0;
        if(!bytes||bytes>512*1024||offset+bytes>file.size||read.next_offset!==offset+bytes||read.attachment?.sha256!==file.sha256||read.attachment.size!==file.size)throw Error('Attachment content changed');
        const uploaded=await target({action:'upload_chunk',chat_id:targetChat,upload_id:uploadId,name:file.name,offset,total_size:file.size,data_base64:read.data_base64});
        if(uploaded.next_offset!==offset+bytes)throw Error('Invalid attachment upload offset');
        offset+=bytes;
        if(offset===file.size)complete=uploaded.attachment;
      }
      if(!complete||complete.sha256!==file.sha256||complete.size!==file.size)throw Error('Attachment transfer integrity check failed');
      state.files.set(file.id,complete);
    }
    transferred.push(complete);
  }
  const text=remapText(message.text,files,transferred);
  if(new TextEncoder().encode(text.trim()).length>32000)throw Error('Message exceeds 32000 bytes');
  return target({action:'send',chat_id:targetChat,message_id:state.messageId,text,attachment_ids:transferred.map(file=>file.id)});
}
