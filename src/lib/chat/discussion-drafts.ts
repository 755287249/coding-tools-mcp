import type {ChatFile,DiscussionAction} from '../api/chat';
type Draft={text:string;attachments:ChatFile[];pending:DiscussionAction|null};
const memory=new Map<string,Draft>();
const empty=():Draft=>({text:'',attachments:[],pending:null});
export const discussionDrafts={
 load(key:string):Draft{
  if(memory.has(key))return structuredClone(memory.get(key)!);
  try{const value=JSON.parse(localStorage.getItem(key)??'null');if(value&&typeof value.text==='string'&&Array.isArray(value.attachments)&&value.attachments.every((f:ChatFile)=>f&&['id','name','path','mime','sha256'].every(k=>typeof f[k as keyof ChatFile]==='string')&&Number.isFinite(f.size)&&f.size>0)&&(!value.pending||(value.pending.action==='discussion_post'&&typeof value.pending.discussion_id==='string'&&typeof value.pending.message_id==='string'&&typeof value.pending.text==='string')))return value;}catch{}
  return empty();
 },
 save(key:string,value:Draft):boolean{
  memory.set(key,structuredClone(value));
  try{if(value.text||value.attachments.length||value.pending)localStorage.setItem(key,JSON.stringify(value));else localStorage.removeItem(key);return true;}catch{return false;}
 }
};
