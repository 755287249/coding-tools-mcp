import { getBackend } from '$lib/backend';
import type { ConnectionHealth } from '$lib/backend/types';
import { writable } from 'svelte/store';

export const connectionHealth = writable<{snapshot:ConnectionHealth|null; receivedAt:number; error:string}>({snapshot:null,receivedAt:0,error:''});
let users=0;
let generation=0;
let timer:ReturnType<typeof setInterval>|undefined;
let pending:Promise<void>|undefined;
export function refreshConnectionHealth():Promise<void> {
  if(pending)return pending;
  const requestGeneration=generation;
  pending=getBackend().workspaces.getConnectionHealth().then(snapshot=>{
    if(requestGeneration===generation)connectionHealth.set({snapshot,receivedAt:Date.now(),error:''});
  }).catch(error=>{
    if(requestGeneration===generation)connectionHealth.update(value=>({...value,error:String(error)}));
  }).finally(()=>{pending=undefined;});
  return pending;
}
function visible(){if(!document.hidden)void refreshConnectionHealth();}
export function acquireConnectionHealth():()=>void {
  users++;
  if(users===1){void refreshConnectionHealth();timer=setInterval(visible,10000);document.addEventListener('visibilitychange',visible);}
  return ()=>{if(--users===0){generation++;clearInterval(timer);timer=undefined;document.removeEventListener('visibilitychange',visible);connectionHealth.set({snapshot:null,receivedAt:0,error:''});}};
}
