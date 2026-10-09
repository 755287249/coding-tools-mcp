import type {ChatOperation,ChatSession} from '../api/chat';
export type OperationRow=ChatOperation&{source:'mcp'|'reported';task:string};
export function chatOperations(session:ChatSession|null,request?:string):OperationRow[]{
 const messages=session?.messages??[];
 const related=new Set(request?[request]:[]);
 if(request)for(const m of messages)if(m.kind==='assignment'&&m.reply_to&&related.has(m.reply_to))related.add(m.id);
 const title=(id:string)=>{const m=messages.find(m=>m.id===id);return m?.text??id;};
 const actual=(session?.operations??[]).filter(e=>!request||related.has(e.reply_to)).map(e=>({...e,source:'mcp' as const,task:title(e.reply_to)}));
 const reports:OperationRow[]=messages.filter(m=>m.role==='assistant'&&m.tool_event&&m.reply_to&&(!request||related.has(m.reply_to))).map(m=>({
  id:'report:'+m.id,reply_to:m.reply_to!,agent_id:m.agent_id,agent_name:m.agent_name??session?.agent_name??'AI',tool:m.tool_event!.name,kind:'other',status:m.tool_event!.status,started_at:m.created_at,paths:[],input:m.tool_event!.input??'',output:m.tool_event!.output??m.text,dry_run:false,truncated:m.tool_event!.output_truncated,source:'reported',task:title(m.reply_to!)
 }));
 return [...actual,...reports].sort((a,b)=>b.started_at-a.started_at);
}

export function operationSummary(rows:OperationRow[]) {
 const actual=rows.filter(row=>row.source==='mcp');
 const completed=actual.filter(row=>row.status==='completed').length;
 const failed=actual.filter(row=>row.status==='failed').length;
 const durations=actual.filter(row=>row.status==='completed'||row.status==='failed').map(row=>row.duration_ms).filter((ms):ms is number=>typeof ms==='number'&&Number.isFinite(ms)&&ms>=0).sort((a,b)=>a-b);
 const changed=new Set(actual.filter(row=>row.kind==='edit'&&row.status==='completed'&&!row.dry_run).flatMap(row=>row.paths));
 return {total:actual.length,completed,failed,running:actual.filter(row=>row.status==='running').length,unknown:actual.filter(row=>row.status==='interrupted').length,
  successRate:completed+failed?Math.round(completed/(completed+failed)*1000)/10:null,
  average:durations.length?durations.reduce((a,b)=>a+b,0)/durations.length:null,p95:durations.length?durations[Math.ceil(durations.length*.95)-1]:null,
  reads:actual.filter(row=>row.kind==='read').length,searches:actual.filter(row=>row.kind==='search').length,edits:actual.filter(row=>row.kind==='edit').length,execs:actual.filter(row=>row.kind==='exec').length,
  files:changed.size,reported:rows.length-actual.length};
}
