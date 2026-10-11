import { workspaceBlocked, assertWorkspaceAvailable } from "$lib/stores/workspace-lifecycle";
import { getBackend } from "$lib/backend";
import type { RuntimeStatus, SandboxBackendDescriptor, WorkspaceProfile } from "$lib/types";

export async function listWorkspaces(): Promise<WorkspaceProfile[]> {
  return (await getBackend().workspaces.list()).filter(item => !workspaceBlocked(item.id));
}

export async function createWorkspace(
  path: string,
  name?: string,
): Promise<WorkspaceProfile> {
  return getBackend().workspaces.create(path, name);
}

export async function listWslDistributions(): Promise<string[]> {
  return getBackend().workspaces.listWslDistributions();
}

export async function listSandboxBackends(): Promise<SandboxBackendDescriptor[]> {
  return getBackend().workspaces.listSandboxBackends();
}

export async function updateWorkspace(profile: WorkspaceProfile): Promise<void> {
  return getBackend().workspaces.update(profile);
}

export async function addWorkspaceFolder(
  id: string,
  path: string,
  name?: string,
): Promise<WorkspaceProfile> {
  return getBackend().workspaces.addFolder(id, path, name);
}

export async function addWslWorkspaceFolder(
  id: string,
  distro: string,
  linuxPath: string,
  name?: string,
): Promise<WorkspaceProfile> {
  return getBackend().workspaces.addWslFolder(id, distro, linuxPath, name);
}

export async function removeWorkspaceFolder(
  id: string,
  folderId: string,
): Promise<WorkspaceProfile> {
  return getBackend().workspaces.removeFolder(id, folderId);
}

export async function openWorkspaceDirectory(path: string): Promise<void> {
  return getBackend().workspaces.openDirectory(path);
}

export async function deleteWorkspace(id: string): Promise<void> {
  return getBackend().workspaces.delete(id);
}

export async function startRuntime(id: string): Promise<RuntimeStatus> {
  assertWorkspaceAvailable(id);
  return getBackend().workspaces.startRuntime(id);
}

export async function stopRuntime(id: string): Promise<RuntimeStatus> {
  return getBackend().workspaces.stopRuntime(id);
}

export async function getRuntimeStatus(id: string): Promise<RuntimeStatus> {
  assertWorkspaceAvailable(id);
  return getBackend().workspaces.getRuntimeStatus(id);
}

export async function startActionsRuntime(id: string): Promise<RuntimeStatus> {
  assertWorkspaceAvailable(id);
  return getBackend().workspaces.startActionsRuntime(id);
}

export async function stopActionsRuntime(id: string): Promise<RuntimeStatus> {
  return getBackend().workspaces.stopActionsRuntime(id);
}

export async function getActionsRuntimeStatus(id: string): Promise<RuntimeStatus> {
  assertWorkspaceAvailable(id);
  return getBackend().workspaces.getActionsRuntimeStatus(id);
}

export async function restartRuntime(id: string): Promise<RuntimeStatus> {
  assertWorkspaceAvailable(id);
  return getBackend().workspaces.restartRuntime(id);
}

export async function restartActionsRuntime(id: string): Promise<RuntimeStatus> {
  assertWorkspaceAvailable(id);
  return getBackend().workspaces.restartActionsRuntime(id);
}
