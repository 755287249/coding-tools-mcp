import {projectToolAllowed} from '../projectIsolation.js';
import { createHash } from 'node:crypto';
import type { ServerResponse } from 'node:http';
import {
  toolNamesForProfile,
  toolsForProfile,
  toolsetRevisionForProfile
} from '../catalog.js';
import type { AgentConfig, ToolContext } from '../types.js';
import { AGENT_VERSION } from '../version.js';

export interface ToolCatalogSnapshot {
  profile: AgentConfig['activeToolProfile'];
  tools: ReturnType<typeof toolsForProfile>;
  names: ReturnType<typeof toolNamesForProfile>;
  revision: string;
}

const toolCatalogCache = new Map<AgentConfig['activeToolProfile'], ToolCatalogSnapshot>();

export function currentToolCatalog(context: ToolContext): ToolCatalogSnapshot {
  const profile = context.config.activeToolProfile;
  let base = toolCatalogCache.get(profile);
  if (!base) {
    base = {
      profile,
      tools: toolsForProfile(profile),
      names: toolNamesForProfile(profile),
      revision: toolsetRevisionForProfile(profile)
    };
    toolCatalogCache.set(profile, base);
  }
  if(context.projectIsolation){
    const tools=base.tools.filter(tool=>projectToolAllowed(tool.name));
    return {profile,tools,names:tools.map(tool=>tool.name),revision:createHash('sha256').update(JSON.stringify(tools)).digest('hex').slice(0,16)};
  }
  const extensionTools = context.extensions.toolDefinitions();
  if (!extensionTools.length) return base;
  return {
    profile,
    tools: [...base.tools, ...extensionTools],
    names: [...base.names, ...extensionTools.map(tool => tool.name)],
    revision: createHash('sha256')
      .update(`${base.revision}:${context.extensions.revision}`)
      .digest('hex')
      .slice(0, 16)
  };
}

export function setRuntimeRevisionHeaders(
  res: ServerResponse,
  catalog: ToolCatalogSnapshot,
  startedAt: number
): void {
  res.setHeader('x-coding-tools-toolset-revision', catalog.revision);
  res.setHeader('x-coding-tools-runtime-started-at', String(startedAt));
  res.setHeader('x-coding-tools-agent-version', AGENT_VERSION);
}
