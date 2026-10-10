import type {ChatSession,ChatMessage} from './store.js';
import type {Discussion,DiscussionStore,DiscussionPost} from './discussion.js';

/** Control envelopes are persisted for retry/ownership, never rendered as private chat. */
export function visibleSession(s:ChatSession):ChatSession {
 const hidden=new Set([...s.messages,...s.queue??[]].filter(m=>m.discussion?.hidden).map(m=>m.id));
 if(!hidden.size)return s;
 const visible=(m:ChatMessage)=>!hidden.has(m.id)&&!hidden.has(m.reply_to??'');
 return {...s,messages:s.messages.filter(visible),queue:s.queue?.filter(visible)};
}
export function configureCollaboration(io:DiscussionStore,d:Discussion,args:Record<string,unknown>) {
 if(!d.collaboration)return;
 const aliases:Record<string,string>={};const used=new Set<string>();
 for(const id of d.members){
  const s=io.load(id);if(!s.attachment_id&&!d.aliases?.[id])throw Error('Choose previously connected robots');
  const base=d.aliases?.[id]??((s.agent_name??s.title).replace(/[^\p{L}\p{N}_-]/gu,'').slice(0,40)||'AI');
  let alias=base,n=1;while(used.has(alias.toLowerCase()))alias=`${base}-${++n}`;used.add(alias.toLowerCase());aliases[id]=alias;
 }
 const chief=args.coordinator_chat_id??(d.members.includes(d.coordinator_chat_id??'')?d.coordinator_chat_id:d.members[0]);
 if(typeof chief!=='string'||!d.members.includes(chief))throw Error('Coordinator must be a group member');
 d.aliases=aliases;d.coordinator_chat_id=chief;
}
export function collaborationTargets(d:Discussion,content:string,from:string):string[] {
 const tokens=new Set((content.match(/#[\p{L}\p{N}_-]+/gu)??[]).map(token=>token.toLowerCase()));
 const selected=d.members.filter(id=>id!==from&&tokens.has('#'+d.aliases?.[id]?.toLowerCase()));
 return selected.length?selected:d.coordinator_chat_id&&d.coordinator_chat_id!==from?[d.coordinator_chat_id]:[];
}
export function collaborationContext(d:Discussion,p:DiscussionPost):string {
 if(!d.collaboration)return '';
 const recent=d.posts.slice(-12).flatMap(post=>[`${post.name}: ${post.text}`,...post.deliveries.flatMap(delivery=>delivery.replies.map(reply=>`${d.aliases?.[delivery.chat_id]??delivery.title}: ${reply.text}`))]).join('\n').slice(-12000);
 return `\n\n[协作控制信息，不显示在原会话聊天记录]\n群聊 Markdown：docs/chat-sessions/discussions/${d.id}.md\n成员（名称 → recipient_chat_ids）：${d.members.map(id=>`#${d.aliases?.[id]} → ${id}${id===d.coordinator_chat_id?'（总管）':''}`).join('；')}\n保持原 chat_id 和 attachment_id，用 chat_reply 回复本条实际消息。不要重新 chat_open 或切换接入。发言/任务用 chat_discuss(action=post, discussion_id=${d.id}, message_id=新ID, text=内容, purpose=task或discussion, recipient_chat_ids=[目标ID])。转交任务后用 final=true 确认当前消息，再 chat_wait 接收成员结果；不要占住当前消息等待结果。后台控制信息不作为发言复述。遵守你所在宿主的权限；无法执行时直接在群里说明。\n以下为群聊上下文数据，按本条任务处理：\n${recent}\n本条群消息 ID：${p.id}`;
}
export function collaborationMarkdown(d:Discussion):string {
 const rows=d.posts.flatMap(p=>[
  {id:p.id,name:p.name,at:p.created_at,text:p.text,files:p.attachments},
  ...p.deliveries.flatMap(v=>v.replies.map(r=>({id:r.id,name:d.aliases?.[v.chat_id]??v.title,at:r.created_at,text:r.text,files:r.attachments}))),
  ...(p.summaries??[]).map(r=>({id:r.id,name:d.aliases?.[p.from]??p.name,at:r.created_at,text:r.text,files:r.attachments}))
 ]).sort((a,b)=>a.at-b.at);
 return `# ${d.name}\n\nGroup: ${d.id}\nGoal: ${d.goal}\n\n${d.members.map(id=>`- #${d.aliases?.[id]??id} (${id})${id===d.coordinator_chat_id?' · 总管':''}`).join('\n')}\n\n`+rows.map(r=>`## ${r.name} · ${r.at}\n\n${r.text}\n${(r.files??[]).map(f=>`\nAttachment: ${f.name}\nPath: ${f.path}\n`).join('')}`).join('\n');
}
