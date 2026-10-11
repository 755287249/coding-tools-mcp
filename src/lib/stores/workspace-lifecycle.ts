/** Tombstones fence late results from requests started before removal. */
const blocked = new Set<string>();
const epochs = new Map<string, number>();
export const workspaceBlocked = (id: string) => blocked.has(id);
export const workspaceEpoch = (id: string) => epochs.get(id) ?? 0;
export function blockWorkspace(id: string) {
  blocked.add(id);
  epochs.set(id, workspaceEpoch(id) + 1);
}
export function restoreWorkspace(id: string) {
  blocked.delete(id);
  epochs.set(id, workspaceEpoch(id) + 1);
}
export function assertWorkspaceAvailable(id: string) {
  if (workspaceBlocked(id)) throw new Error('Workspace is being removed');
}
