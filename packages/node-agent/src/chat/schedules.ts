import {readFileSync,statSync,writeFileSync,openSync,fsyncSync,closeSync,renameSync,rmSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import {redactSensitiveText} from '../redaction.js';
interface Store {safe(relative:string):string;validateChat(id:string):void;send(args:Record<string,unknown>):unknown}
interface Task {id:string;chatId?:string;title?:string;text?:string;due?:number;status:string;error?:string}
const FILE='docs/chat-sessions/.schedules.json', LIMIT=20*1024*1024;
function read(store:Store):Task[] {
  const file=store.safe(FILE);
  try {
    if(statSync(file).size>LIMIT)throw Error('Schedule storage exceeds limit');
    const value=JSON.parse(readFileSync(file,'utf8'));
    if(value.version!==1||!Array.isArray(value.tasks))throw Error('Invalid schedule storage');
    return value.tasks;
  } catch(error) {if((error as NodeJS.ErrnoException).code==='ENOENT')return [];throw error;}
}
function write(store:Store,tasks:Task[]) {
  const body=JSON.stringify({version:1,tasks});if(Buffer.byteLength(body)>LIMIT)throw Error('Schedule storage exceeds limit');
  const temp=store.safe(`docs/chat-sessions/.schedules.${randomUUID()}.tmp`);
  try {const fd=openSync(temp,'wx',0o600);try{writeFileSync(fd,body);fsyncSync(fd)}finally{closeSync(fd)}renameSync(temp,store.safe(FILE))}finally{rmSync(temp,{force:true})}
}
function id(value:unknown):string {if(typeof value!=='string'||!/^[A-Za-z0-9_-]{1,80}$/.test(value))throw Error('Invalid chat/message ID');return value}
function text(value:unknown,max:number):string {if(typeof value!=='string'||!value.trim()||Buffer.byteLength(value)>max)throw Error(`Text must contain 1–${max} bytes`);return redactSensitiveText(value).value}
function response(tasks:Task[]) {return {tasks:tasks.filter(t=>t.status!=='deleted')}}
export function scheduleAction(store:Store,args:Record<string,unknown>):Record<string,unknown> {
  const tasks=read(store);if(args.action==='schedule_list')return response(tasks);
  const taskId=id(args.id);
  if(args.action==='schedule_create'||args.action==='schedule_import') {
    const chatId=id(args.chatId),body=text(args.text,32000),due=args.due;
    if(typeof due!=='number'||!Number.isSafeInteger(due)||due<=0)throw Error('Invalid schedule time');
    const existing=tasks.find(t=>t.id===taskId);
    if(existing){if(existing.status!=='deleted'&&(existing.chatId!==chatId||existing.text!==body||existing.due!==due))throw Error('Schedule ID conflicts with existing task');return response(tasks)}
    if(tasks.filter(t=>t.status!=='deleted').length>=512)throw Error('Schedule limit reached');
    const status=args.action==='schedule_import'?String(args.status):'pending';
    if(!['pending','sending','sent','error','paused'].includes(status))throw Error('Invalid schedule status');
    if(!['sent','error'].includes(status))store.validateChat(chatId);
    tasks.push({id:taskId,chatId,title:text(args.title,240).trim(),text:body,due,status});
  } else if(args.action==='schedule_update') {
    const index=tasks.findIndex(t=>t.id===taskId),task=tasks[index];if(!task)throw Error('Schedule not found');
    if(task.status==='deleted'&&args.status==='deleted')return response(tasks);
    if(task.status!==args.expected_status)throw Error('Schedule changed; refresh before editing');
    const next=String(args.status),current=task.status;
    if(['sending','deleted'].includes(current)||!(next==='deleted'||current==='pending'&&next==='paused'||['paused','error'].includes(current)&&next==='pending'))throw Error('Invalid schedule transition');
    if(next==='deleted')tasks[index]={id:taskId,status:next};else{task.status=next;delete task.error}
  } else throw Error('Unknown schedule action');
  write(store,tasks);return response(tasks);
}
export function tickSchedules(store:Store) {
  const tasks=read(store);
  for(const task of tasks) {
    if(task.status!=='sending'&&!(task.status==='pending'&&task.due!<=Date.now()))continue;
    task.status='sending';write(store,tasks);
    try {store.send({action:'send',chat_id:task.chatId,message_id:task.id,text:task.text});task.status='sent';delete task.error}
    catch(error){task.status='error';task.error=error instanceof Error?error.message:String(error)}
    write(store,tasks);
  }
}
