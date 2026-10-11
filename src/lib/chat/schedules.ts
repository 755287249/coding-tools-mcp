import {get,writable} from 'svelte/store';
import {getBackend} from '$lib/backend';
import {workspaces} from '$lib/stores/app';
import {workspaceFolders} from '$lib/types';
import type {ChatAction} from '$lib/api/chat';
import {startVisiblePolling} from './polling';
import {onChatCacheReset} from './cache-lifecycle.js';
import {workspaceBlocked,workspaceEpoch} from '$lib/stores/workspace-lifecycle';
export interface ScheduledChat {
  id:string; workspaceId:string; folderId:string; chatId:string; title:string; text:string;
  due:number; status:'pending'|'sending'|'sent'|'error'|'paused'; error?:string;
}
const KEY='ctmcp-chat-schedules-v1';
export const scheduledChats=writable<ScheduledChat[]>([]);
export const scheduleError=writable('');
let identity=0;
onChatCacheReset(()=>{identity++;scheduledChats.set([]);scheduleError.set('')});
async function request(workspaceId:string,folderId:string,args:Record<string,unknown>) {
  const epoch=identity,workspaceVersion=workspaceEpoch(workspaceId);
  if(workspaceBlocked(workspaceId))throw Error('Project unavailable');
  const result=await getBackend().chat.request(workspaceId,folderId,args as unknown as ChatAction) as unknown as {tasks:Omit<ScheduledChat,'workspaceId'|'folderId'>[]};
  if(epoch!==identity||workspaceVersion!==workspaceEpoch(workspaceId)||workspaceBlocked(workspaceId))throw Error('Schedule session changed; refresh before editing');
  if(!Array.isArray(result.tasks))throw Error('Invalid schedule response');
  const tasks=result.tasks.map(task=>({...task,workspaceId,folderId}));
  scheduledChats.update(current=>[...current.filter(t=>t.workspaceId!==workspaceId||t.folderId!==folderId),...tasks]);
  return tasks;
}
export async function createSchedule(task:ScheduledChat) {await request(task.workspaceId,task.folderId,{...task,action:'schedule_create'})}
export async function updateSchedule(task:ScheduledChat,status:ScheduledChat['status']|'deleted') {
  await request(task.workspaceId,task.folderId,{action:'schedule_update',id:task.id,status,expected_status:task.status});
}
function legacyTasks():ScheduledChat[] {
  let raw:string|null;try{raw=localStorage.getItem(KEY)}catch{return []}
  const tasks=JSON.parse(raw??'[]');
  if(!Array.isArray(tasks)||tasks.some(t=>!t||!['id','workspaceId','folderId','chatId','title','text'].every(k=>typeof t[k]==='string')||!Number.isFinite(t.due)||!['pending','sending','sent','error','paused'].includes(t.status)))throw Error('Invalid scheduled-task storage');
  return tasks;
}
/** Only synchronization lives in the browser. Delivery is owned by the server. */
export function startScheduleRunner() {
  let stopped=false;
  const stop=startVisiblePolling(async()=>{
    const pollIdentity=identity;
    try {
      const targets=get(workspaces).flatMap(ws=>workspaceFolders(ws).map(folder=>({workspaceId:ws.id,folderId:folder.id})));
      const warnings:string[]=[];
      let legacy:ScheduledChat[]=[];
      try{legacy=legacyTasks()}catch(error){warnings.push(String(error))}
      for(const task of legacy) {
        if(stopped||pollIdentity!==identity)return;
        // Folder IDs survive the one-directory project migration.
        const matches=targets.filter(t=>t.folderId===task.folderId);
        const target=targets.find(t=>t.workspaceId===task.workspaceId&&t.folderId===task.folderId)??(matches.length===1?matches[0]:undefined);
        if(!target)continue;
        try {
          await request(target.workspaceId,target.folderId,{...task,action:'schedule_import'});
          const latest=legacyTasks();
          localStorage.setItem(KEY,JSON.stringify(latest.filter(t=>!(t.id===task.id&&t.workspaceId===task.workspaceId&&t.folderId===task.folderId))));
        }catch(error){warnings.push(String(error))}
      }
      for(const target of targets) {
        if(stopped||pollIdentity!==identity)return;
        try{await request(target.workspaceId,target.folderId,{action:'schedule_list'})}catch(error){warnings.push(String(error))}
      }
      if(stopped||pollIdentity!==identity)return;
      const active=new Set(targets.map(t=>JSON.stringify([t.workspaceId,t.folderId])));
      scheduledChats.update(tasks=>tasks.filter(t=>active.has(JSON.stringify([t.workspaceId,t.folderId]))));
      if(!stopped&&pollIdentity===identity)scheduleError.set([...new Set(warnings)].join('\n'));
    }catch(error){if(!stopped&&pollIdentity===identity)scheduleError.set(String(error))}
  },()=>3000);
  return()=>{stopped=true;stop();scheduledChats.set([])};
}
