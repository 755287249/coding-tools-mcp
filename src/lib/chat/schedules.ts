import { writable } from 'svelte/store';
import { localChat } from '$lib/api/chat';
export interface ScheduledChat {
  id:string; workspaceId:string; folderId:string; chatId:string; title:string; text:string;
  due:number; status:'pending'|'sending'|'sent'|'error'|'paused'; error?:string;
}
const KEY='ctmcp-chat-schedules-v1';
export const scheduledChats=writable<ScheduledChat[]>([]);
export const scheduleError=writable('');
function read():ScheduledChat[] {
  const value=JSON.parse(localStorage.getItem(KEY)??'[]');
  if(!Array.isArray(value)||value.some(t=>!t||!['id','workspaceId','folderId','chatId','title','text'].every(k=>typeof t[k]==='string')||!Number.isFinite(t.due)||!['pending','sending','sent','error','paused'].includes(t.status)))throw new Error('Invalid scheduled-task storage');
  return value;
}
function write(tasks:ScheduledChat[]) {localStorage.setItem(KEY,JSON.stringify(tasks));scheduledChats.set(tasks);scheduleError.set('');}
export async function changeSchedule(change:(tasks:ScheduledChat[])=>ScheduledChat[]) {
  if(!navigator.locks)throw new Error('This browser does not support safe scheduled-task locking.');
  await navigator.locks.request(KEY,()=>write(change(read())));
}
export function startScheduleRunner() {
  let stopped=false, running=false;
  const refresh=()=>{try{scheduledChats.set(read())}catch(e){scheduleError.set(String(e))}};
  async function run(){
    if(stopped||running||!navigator.locks)return;running=true;
    try{await navigator.locks.request(KEY,{ifAvailable:true},async lock=>{
      if(!lock||stopped)return;
      const tasks=read();
      for(const task of tasks) {
        if(stopped)break;
        if(!((task.status==='pending'&&task.due<=Date.now())||task.status==='sending'))continue;
        task.status='sending';write(tasks);
        try {
          // The stable message ID and immutable payload also make recovery after a
          // lost response safe. Competing windows share this Web Lock.
          await localChat(task.workspaceId,task.folderId,{action:'send',chat_id:task.chatId,message_id:task.id,text:task.text});
          task.status='sent';task.error=undefined;
        }catch(e){task.status='error';task.error=String(e)}
        write(tasks);
      }
      scheduledChats.set(tasks);
    })}catch(e){scheduleError.set(String(e))}finally{running=false}
  }
  refresh();void run();const timer=setInterval(()=>void run(),1000);
  const storage=(event:StorageEvent)=>{if(event.key===KEY)refresh()};window.addEventListener('storage',storage);
  return()=>{stopped=true;clearInterval(timer);window.removeEventListener('storage',storage)};
}
