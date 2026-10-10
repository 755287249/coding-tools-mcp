import { clearChatCaches } from '../chat/cache-lifecycle.js';
import type { FrontendBackend } from "./types";

export type {
  AgentBackend,
  AgentRestartResult,
  AlertOptions,
  BooleanCapability,
  ConfirmOptions,
  DirectoryBackend,
  DirectoryBrowseResult,
  ExtensionInventoryPayload,
  ExtensionKind,
  ExtensionMasterToggleResult,
  ExtensionProvider,
  ExtensionScope,
  ExtensionToggleResult,
  FrontendBackend,
  FrontendCapabilities,
  FrontendHost,
  HealthBackend,
  HistoryBackend,
  HookInventoryItem,
  InvokeFn,
  LogsBackend,
  NativeUi,
  OperationLogPayload,
  OperationLogQuery,
  OperationsBackend,
  PickDirectoryOptions,
  McpServerInventoryItem,
  SecretsBackend,
  SettingsBackend,
  SoftwareBackend,
  SkillInventoryItem,
  SkillInventoryPayload,
  SkillMasterToggleResult,
  SkillScope,
  SkillSource,
  SkillToggleResult,
  TelemetryBackend,
  TelemetryQueryOptions,
  TunnelBackend,
  WorkspaceBackend,
  WorkspaceFeatureBackend,
} from "./types";
export { DESKTOP_CAPABILITIES, NODE_CAPABILITIES } from "./capabilities";
export { CapabilityError, UnimplementedError, requireCapability } from "./errors";
export {
  isUnavailableBackendError,
  loadMcpAuthSecrets,
  readSecretIfAvailable,
  workspaceAuthSecretKeys,
} from "./secret-read";
export type { McpAuthSecrets } from "./secret-read";
export { createTauriBackend } from "./tauri";
export { createNodeBackend } from "./node";

let current: FrontendBackend | null = null;

export function setBackend(backend: FrontendBackend): void {
  clearChatCaches();
  current = backend;
}

export function getBackend(): FrontendBackend {
  if (!current) {
    throw new Error("Frontend backend was not initialized. Call setBackend() at app startup.");
  }
  return current;
}

export function resetBackend(): void {
  clearChatCaches();
  current = null;
}
