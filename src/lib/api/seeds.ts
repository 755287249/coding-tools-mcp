import {getBackend} from '$lib/backend';
import type {ChatAction} from './chat';
export interface SeedFilter {source:string;keyword:string;prefix?:string;account?:string}
export interface SeedGroup {source:string;prefix:string;total:number;ready:number}
export interface SeedRow {enrollment_until?:number;redeemed_at?:number;access_until?:number;source:string;prefix:string;manual?:boolean;id:string;account:string;repo_id:string;branch:string;metadata?:Record<string,string>;created_at:number;last_seen:number;status:string;reason:string;archived:boolean;retired_at:number;host_task_id:string;chat_id:string;generation:number;unresolved_operations:number}
export interface SeedAccountSummary {account:string;total:number;valid:number;counts:Record<string,number>}
export interface ReadySeedGroup {source:string;prefix:string;account:string;ready:number}
export interface SeedInventory {affected_count?:number;ready_groups?:ReadySeedGroup[];account_summaries?:SeedAccountSummary[];groups?:SeedGroup[];sources?:string[];enabled:boolean;seeds:SeedRow[];retired_count:number;total?:number;filtered_total?:number;counts?:Record<string,number>;accounts?:string[];pairing_enabled?:boolean;capacity?:number}
export interface SeedTicket {source?:string;prefix?:string;manual?:boolean;seed_id:string;ticket:string;url_key?:string;repo_id:string;branch:string;account:string;expires_at:number}
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
