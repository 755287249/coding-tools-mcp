export interface ChatOperation {
  id:string; reply_to:string; agent_id?:string; agent_name:string; tool:string;
  kind:'read'|'search'|'edit'|'exec'|'other'; status:'running'|'completed'|'failed'|'interrupted';
  started_at:number; duration_ms?:number; paths:string[]; input:string; output?:string;
  diff?:string; truncated?:boolean; dry_run:boolean;
}
export function operationsMarkdown(events:ChatOperation[]):string {
  return '# MCP operations\n\nBounded recent operation history; running is not proof of completion.\n\n'+events.map(e=>`## ${e.agent_name} · ${e.tool} · ${e.status}\n\nRequest: ${e.reply_to}\nTime: ${e.started_at}\n${e.paths.join('\n')}\n${e.dry_run?'Dry run\n':''}${e.input}\n${e.output??''}\n${e.diff??''}\n${e.truncated?'Details truncated\n':''}`).join('\n');
}
