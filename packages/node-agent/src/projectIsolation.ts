import {projectScopedToolNames} from './rustCatalog.generated.js';
import type {AgentConfig,ToolContext} from './types.js';

export const GLOBAL_MCP_ID='00000000000000000000000000000001';
const allowed=new Set<string>(projectScopedToolNames);
export function projectToolAllowed(name:string):boolean {return allowed.has(name);}
export function enforceProjectConfiguration(config:AgentConfig):void {
  config.sandbox={...config.sandbox,enabled:true,externalPaths:[]};
  config.securityPolicy={...config.securityPolicy,enforceWorkspaceBoundary:true,blockSymlinkEscape:true};
  config.extensions={hooks:{...config.extensions.hooks,active:false},mcp:{...config.extensions.mcp,active:false}};
}
export function enforceProjectContext(context:ToolContext):void {
  if(context.projectIsolation)enforceProjectConfiguration(context.config);
}
