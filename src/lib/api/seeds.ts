import {getBackend} from '$lib/backend';
import type {ChatAction} from './chat';
export interface SeedRow {id:string;account:string;repo_id:string;branch:string;created_at:number;last_seen:number;status:string;reason:string;archived:boolean;retired_at:number;host_task_id:string;chat_id:string;generation:number;unresolved_operations:number}
export interface SeedInventory {enabled:boolean;seeds:SeedRow[];retired_count:number}
export interface SeedTicket {seed_id:string;ticket:string;repo_id:string;branch:string;account:string;expires_at:number}
export async function seedRequest<T=SeedInventory>(workspaceId:string,folderId:string,args:Record<string,unknown>):Promise<T>{
  return await getBackend().chat.request(workspaceId,folderId,args as ChatAction) as T;
}
export function seedBundle(endpoint:string,folderId:string,batch:SeedTicket[]):string {
  const url=new URL(endpoint.trim());
  if(url.protocol!=='https:'&&!(url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname)))throw Error('Use HTTPS or a loopback MCP endpoint');
  if(url.username||url.password||url.search||url.hash||!url.pathname.replace(/\/$/,'').endsWith('/mcp'))throw Error('Enter the full MCP endpoint ending in /mcp');
  url.pathname=url.pathname.replace(/\/$/,'');
  return JSON.stringify({version:1,endpoint:url.toString(),workspace_folder_id:folderId,seeds:batch});
}
