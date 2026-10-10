import type {Discussion,ChatMessage} from '../api/chat';
export function robotQuery(text:string,caret:number){const match=/(?:^|\s)#([\p{L}\p{N}_-]*)$/u.exec(text.slice(0,caret));return match?{start:caret-match[1].length-1,end:caret,query:match[1]}:null;}
export function memberTint(id:string):string {let hash=2166136261;for(const char of id)hash=Math.imul(hash^char.codePointAt(0)!,16777619);return `hsl(${(hash>>>0)/4294967296*360} 55% 72%)`;}
export function discussionTimeline(d:Discussion){
 const rows:(ChatMessage&{memberId:string;memberName:string;sourceChatId:string})[]=[];
 for(const post of d.posts??[]){
  rows.push({id:post.id,role:post.from==='user'?'user':'assistant',text:post.text,created_at:post.created_at,memberId:post.from,memberName:post.name,sourceChatId:post.from});
  for(const delivery of post.deliveries)for(const reply of delivery.replies)rows.push({...reply,id:delivery.chat_id+':'+reply.id,memberId:delivery.chat_id,memberName:d.aliases?.[delivery.chat_id]??delivery.title,sourceChatId:delivery.chat_id});
  for(const reply of post.summaries??[])rows.push({...reply,id:post.from+':'+reply.id,memberId:post.from,memberName:d.aliases?.[post.from]??post.name,sourceChatId:post.from});
 }
 return rows.sort((a,b)=>a.created_at-b.created_at);
}
