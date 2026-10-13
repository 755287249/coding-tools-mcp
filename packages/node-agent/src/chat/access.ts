import {createHash, randomUUID} from 'node:crypto';
/** Fixed full access for both direct AI connections and seed assignments. */
export type AccessMode='full';
export interface PendingApproval {id:string;tool:string;summary:string;args_hash:string;created_at:number;status:'pending'|'approved'|'denied';resolved_at?:number}
export interface AccessState {access_mode?:AccessMode;approvals?:PendingApproval[]}
export const APPROVAL_TTL_MS=30*60_000;
const MAX_APPROVALS=20;
export function accessMode(_s:AccessState):AccessMode {return 'full';}
export function setAccessMode(s:AccessState,_mode:unknown):void {
  s.access_mode='full';
  s.approvals=[];
}
function stable(value:unknown):unknown {
  if(Array.isArray(value))return value.map(stable);
  if(value&&typeof value==='object')return Object.fromEntries(Object.keys(value as object).sort().map(k=>[k,stable((value as Record<string,unknown>)[k])]));
  return value;
}
const IGNORED=new Set(['approval_id','confirm','chat_id','attachment_id','workspace_folder_id','operation_id']);
export function approvalHash(tool:string,args:Record<string,unknown>):string {
  const kept=Object.fromEntries(Object.entries(args).filter(([k])=>!IGNORED.has(k)));
  return createHash('sha256').update(tool+'\0'+JSON.stringify(stable(kept))).digest('hex').slice(0,32);
}
export function approvalSummary(tool:string,args:Record<string,unknown>):string {
  const pick=['path','paths','command','cmd','target','source','destination','operation','pattern'].filter(k=>args[k]!==undefined).map(k=>`${k}: ${typeof args[k]==='string'?args[k]:JSON.stringify(args[k])}`);
  return [tool,...pick].join('\n').slice(0,600);
}
export function pruneApprovals(s:AccessState,now=Date.now()):void {
  s.approvals=(s.approvals??[]).filter(a=>now-a.created_at<APPROVAL_TTL_MS).slice(-MAX_APPROVALS);
}
/** Returns the approval for this exact call, creating a pending one when needed. */
export function requestApproval(s:AccessState,tool:string,args:Record<string,unknown>,now=Date.now()):PendingApproval {
  pruneApprovals(s,now);
  const hash=approvalHash(tool,args),id=typeof args.approval_id==='string'?args.approval_id:'';
  const existing=[...(s.approvals??[])].reverse().find(a=>(!id||a.id===id)&&a.args_hash===hash&&a.tool===tool);
  if(existing)return existing;
  const created:PendingApproval={id:randomUUID().replace(/-/g,'').slice(0,16),tool,summary:approvalSummary(tool,args),args_hash:hash,created_at:now,status:'pending'};
  (s.approvals??=[]).push(created);pruneApprovals(s,now);
  return created;
}
/** An approval is single use: consuming it removes it. */
export function consumeApproval(s:AccessState,id:string):void {s.approvals=(s.approvals??[]).filter(a=>a.id!==id);}
export function resolveApproval(s:AccessState,id:unknown,approve:unknown,now=Date.now()):PendingApproval {
  if(typeof approve!=='boolean')throw new Error('approve must be a boolean');
  pruneApprovals(s,now);
  const found=(s.approvals??[]).find(a=>a.id===id);
  if(!found)throw new Error('Approval request not found or expired');
  if(found.status==='pending'){found.status=approve?'approved':'denied';found.resolved_at=now;}
  return found;
}
export function pendingApprovals(_s:AccessState,_now=Date.now()):PendingApproval[] {
  return [];
}
