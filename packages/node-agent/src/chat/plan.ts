import { redactSensitiveText } from '../redaction.js';
export interface ChatTodo {id:string;title:string;status:'pending'|'in_progress'|'completed'}
export interface ChatTaskPlan {goal:string;todos:ChatTodo[];updated_ms:number;progress?:{message:string;phase?:string;percent?:number;todo_id?:string;updated_ms:number}}
function field(args:Record<string,unknown>,key:string,max:number,required=false):string|undefined {
 const value=args[key];if(value===undefined||value===null){if(required)throw new Error(`${key} is required`);return;}
 if(typeof value!=='string'||[...value].length>max||(required&&!value.trim()))throw new Error(`Invalid ${key}`);
 return redactSensitiveText(value.trim()).value;
}
export function reduceChatPlan(previous:ChatTaskPlan|undefined,name:string,args:Record<string,unknown>):ChatTaskPlan|undefined {
 const now=Date.now();
 if(name==='report_progress'){
  const message=field(args,'message',2000,true)!;const phase=field(args,'phase',160);const todo_id=field(args,'todo_id',80)||undefined;
  const percent=args.percent;if(percent!==undefined&&percent!==null&&(!Number.isInteger(percent)||Number(percent)<0||Number(percent)>100))throw new Error('percent must be an integer from 0 to 100');
  const plan:ChatTaskPlan=previous??{goal:'',todos:[],updated_ms:now};
  if(todo_id&&!plan.todos.some(todo=>todo.id===todo_id))throw new Error('Unknown todo_id');
  return {...plan,updated_ms:now,progress:{message,phase,...(typeof percent==='number'?{percent}:{}),todo_id:todo_id??plan.todos.find(t=>t.status==='in_progress')?.id,updated_ms:now}};
 }
 const key=name==='set_todos'?'todos':'plan';const items=args[key];if(!Array.isArray(items)||items.length>24)throw new Error(`${key} must contain at most 24 items`);
 const goal=field(args,'goal',400)??previous?.goal??'';const explanation=name==='update_plan'?field(args,'explanation',2000):undefined;
 const remaining=[...(previous?.todos??[])];const used=new Set<string>();let nextId=1;
 const todos=items.map((value):ChatTodo=>{
  if(!value||typeof value!=='object')throw new Error('Invalid todo');const item=value as Record<string,unknown>;
  const title=field(item,name==='set_todos'?'title':'step',400,true)!;let id:string;
  if(name==='set_todos')id=field(item,'id',80,true)!;
  else {const index=remaining.findIndex(todo=>todo.title===title);if(index>=0)id=remaining.splice(index,1)[0].id;else{do{id=`todo-${nextId++}`;}while(used.has(id)||remaining.some(todo=>todo.id===id));}}
  if(used.has(id))throw new Error('Duplicate todo id');used.add(id);
  if(typeof item.status!=='string'||!['pending','in_progress','completed'].includes(item.status))throw new Error('Invalid todo status');
  return {id,title,status:item.status as ChatTodo['status']};
 });
 if(todos.filter(t=>t.status==='in_progress').length>1)throw new Error('At most one todo may be in_progress');
 if(!todos.length)return undefined;
 return {goal,todos,updated_ms:now,...(explanation?{progress:{message:explanation,updated_ms:now}}:{})};
}
export function chatPlanSummary(plan:ChatTaskPlan|undefined) {
 if(!plan)return {cleared:true,todos:[]};const completed=plan.todos.filter(t=>t.status==='completed').length;const current=plan.todos.find(t=>t.status==='in_progress');
 return {...plan,completed,total:plan.todos.length,current:current?{id:current.id,title:current.title}:null,status:plan.todos.length&&completed===plan.todos.length?'completed':current?'in_progress':'pending'};
}
export function chatPlanMarkdown(plan:ChatTaskPlan|undefined):string {
 if(!plan)return '';return `\n### 任务计划\n\n${plan.goal}\n\n${plan.todos.map(todo=>`- [${todo.status==='completed'?'x':' '}] ${todo.title} (${todo.status})`).join('\n')}\n${plan.progress?`\n${plan.progress.phase??''} ${plan.progress.message}${plan.progress.percent===undefined?'':` · ${plan.progress.percent}%`}\n`:''}`;
}
