import type { OAuthRuntime } from '../oauth.js';
import type { JsonObject, ToolContext } from '../types.js';
import type { ConfigStore } from './configStore.js';
import type { RuntimeHotApplyTarget } from './runtimeContract.js';

export type { RuntimeHotApplyTarget, TunnelRuntimeController } from './runtimeContract.js';

export interface WorkspaceManagementStore {
  readonly primaryWorkspaceId: string;
  snapshot(): JsonObject;
  addWorkspace(folderPath: string, name?: string): Promise<JsonObject>;
  deleteWorkspace(id: string): Promise<JsonObject>;
  saveWorkspace(id: string, value: unknown, runtime?: RuntimeHotApplyTarget): Promise<JsonObject>;
  setSkillActive(id: string, active: boolean, runtime?: RuntimeHotApplyTarget): Promise<JsonObject>;
  setSkillEnabled(id: string, key: string, enabled: boolean, runtime?: RuntimeHotApplyTarget): Promise<JsonObject>;
  setExtensionActive(id: string, kind: 'hook' | 'mcp', active: boolean, runtime?: RuntimeHotApplyTarget): Promise<JsonObject>;
  setExtensionEnabled(id: string, kind: 'hook' | 'mcp', key: string, enabled: boolean, runtime?: RuntimeHotApplyTarget): Promise<JsonObject>;
  secret(id: string, key: 'oauthPassword'): string;
  replaceSecret(
    id: string,
    key: 'oauthPassword' | 'tunnelEnrollmentUrl',
    value: string,
    runtime?: RuntimeHotApplyTarget
  ): Promise<JsonObject>;
  regenerateSecret(id: string, key: 'oauthPassword', runtime?: RuntimeHotApplyTarget): Promise<JsonObject>;
  replaceNamedSecret(id: string, reference: string, value: string): Promise<JsonObject>;
}

export interface WorkspaceRuntimeRecord extends RuntimeHotApplyTarget {
  startedAt: number;
}

export interface ManagementOptions {
  configStore: ConfigStore;
  context: ToolContext;
  oauth: OAuthRuntime;
  startedAt: number;
  adminToken: string;
  requestRestart?: () => void;
  workspaceStore?: WorkspaceManagementStore;
  runtimeRegistry?: Map<string, WorkspaceRuntimeRecord>;
}
