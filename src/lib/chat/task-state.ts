import type {ChatSession,ChatTaskPlan} from '../api/chat';
import {chatUserState,messageAnswered} from './status';
export type ChatTaskStatus='queued'|'processing'|'awaiting_user'|'completed'|'closed'|'interrupted';
/** Only messages delivered to this exact chat contribute; no workspace-wide feed. */
export function currentChatTask(session:ChatSession|null,requestId?:string) {
 const messages=session?.messages??[];
 const users=messages.filter(m=>m.role==='user'&&m.kind!=='connection_request');
 const pending=users.find(m=>!messageAnswered(session,m.id));
 const message=requestId?users.find(m=>m.id===requestId):pending??users.at(-1);
 if(!message)return null;
 const receipt=chatUserState(session,message.id)!;
 const status=receipt.status==='replied'?'completed':receipt.status==='processing'&&session?.status==='offline'?'interrupted':receipt.status;
 const related=new Set([message.id,...messages.filter(m=>m.kind==='assignment'&&m.reply_to===message.id).map(m=>m.id)]);
 const replies=messages.filter(m=>m.role==='assistant'&&m.reply_to&&related.has(m.reply_to));
 const latest=replies.filter(r=>!r.tool_event&&r.reply_to===message.id).at(-1);
 const coordinator=session?.members?.find(m=>m.role==='coordinator')?.id;
 const plan=message.agent_plans?.find(p=>p.agent_id===coordinator)?.plan??message.task_plan??(message.agent_plans?.length===1?message.agent_plans[0].plan:undefined);
 const reported=plan?.progress;
 const progress=reported&&reported.updated_ms>(latest?.created_at??0)?reported.message:latest?.text??reported?.message??'';
 const agentTasks=[message,...messages.filter(m=>m.kind==='assignment'&&m.reply_to===message.id)].flatMap(m=>(m.recipient_ids??[]).map(id=>({id:m.id+':'+id,name:session?.members?.find(p=>p.id===id)?.name??id,accepted:!!m.received_by?.includes(id)||messages.some(r=>r.reply_to===m.id&&r.agent_id===id),text:m.text,plan:m.agent_plans?.find(p=>p.agent_id===id)?.plan,complete:messages.some(r=>r.reply_to===m.id&&r.agent_id===id&&r.final===true)})));
 return {related,agentTasks,message,status:status as ChatTaskStatus,plan,progress,tools:replies.filter(r=>r.tool_event),completed:users.filter(u=>chatUserState(session,u.id)?.status==='replied').length};
}
export function taskPlanMarkdown(plan:ChatTaskPlan|undefined):string {
 if(!plan)return '';return `\n### 任务计划\n\n${plan.goal}\n\n${plan.todos.map(todo=>`- [${todo.status==='completed'?'x':' '}] ${todo.title} (${todo.status})`).join('\n')}\n${plan.progress?`\n${plan.progress.phase??''} ${plan.progress.message}${plan.progress.percent===undefined?'':` · ${plan.progress.percent}%`}\n`:''}`;
}
