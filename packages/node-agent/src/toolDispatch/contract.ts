import type { ConversationIdentity } from '../conversation/contract.js';
import type { ProcessRequestLifecycle } from '../processes.js';
import type { KnowledgeCanaryDecision } from '../knowledge/canary.js';
import type { JsonObject, ToolContext } from '../types.js';

export interface ResumeToolRequest {
  readonly name: string;
  readonly args: JsonObject;
  readonly meta: unknown;
  readonly folderId: string;
  readonly defaultCwd: string;
}

export interface ToolDispatchRequest {
  readonly ctx: ToolContext;
  readonly key: string;
  readonly identity: ConversationIdentity;
  readonly args: JsonObject;
  readonly historyArgs: JsonObject;
  readonly processLifecycle?: ProcessRequestLifecycle;
  readonly knowledgeCanary?: KnowledgeCanaryDecision;
  readonly resumeTool?: (request: ResumeToolRequest) => Promise<JsonObject>;
}

export type ToolHandler = (request: ToolDispatchRequest) => JsonObject | Promise<JsonObject>;
export type ToolHandlerMap = Readonly<Record<string, ToolHandler>>;
