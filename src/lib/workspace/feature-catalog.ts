import type { SkillInventoryPayload, ExtensionInventoryPayload } from '../backend/types';
export type FeatureRow={key:string;kind:'skill'|'hook'|'mcp';name:string;description:string;provider:string;scope:string;path:string;version:string|null;selected:boolean;enabled:boolean;supported:boolean;sourceEnabled:boolean;connected:boolean;transport:string;toolCount:number;error:string|null};
export function featureRows(tab:'skills'|'hooks'|'mcp',skills:SkillInventoryPayload|null,extensions:ExtensionInventoryPayload|null):FeatureRow[]{
  if(tab==='skills')return (skills?.skills??[]).map(s=>({key:s.key,kind:'skill',name:s.name,description:s.description,provider:s.source,scope:s.scope,path:s.relativePath,version:s.version,selected:s.selected,enabled:s.enabled,supported:true,sourceEnabled:true,connected:false,transport:'',toolCount:0,error:null}));
  if(tab==='hooks')return (extensions?.hooks??[]).map(h=>({key:h.key,kind:'hook',name:h.event,description:h.matcher??'',provider:h.provider,scope:h.scope,path:h.sourcePath,version:null,selected:h.selected,enabled:h.enabled,supported:h.supported,sourceEnabled:h.sourceEnabled,connected:false,transport:h.handlerType,toolCount:0,error:null}));
  return (extensions?.mcpServers??[]).map(s=>({key:s.key,kind:'mcp',name:s.name,description:'',provider:s.provider,scope:s.scope,path:s.sourcePath,version:null,selected:s.selected,enabled:s.enabled,supported:s.supported,sourceEnabled:s.sourceEnabled,connected:s.connected,transport:s.transport,toolCount:s.toolCount,error:s.error}));
}
export function filterFeatureRows(rows:FeatureRow[],query:string,scope:'all'|'workspace'|'user'){
  const words=query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return rows.filter(row=>(scope==='all'||(scope==='user'?row.scope==='user':row.scope!=='user'))&&words.every(word=>`${row.name} ${row.description} ${row.provider}`.toLocaleLowerCase().includes(word)));
}
