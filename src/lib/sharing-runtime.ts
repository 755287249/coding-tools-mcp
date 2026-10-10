import type { RuntimeStatus, WorkspaceProfile } from './types';

export interface SharingRuntimeActions {
  getStatus(id: string): Promise<RuntimeStatus>;
  start(id: string): Promise<RuntimeStatus>;
  restart(id: string): Promise<RuntimeStatus>;
  save(profile: WorkspaceProfile): Promise<void>;
  confirmLan(): Promise<boolean>;
}

export async function prepareSharingRuntime(
  profile: WorkspaceProfile,
  allowLan: boolean,
  actions: SharingRuntimeActions,
): Promise<WorkspaceProfile | null> {
  const status = await actions.getStatus(profile.id);
  if (status.state === 'starting' || status.state === 'stopping') {
    throw new Error(status.localMessage || 'Service is changing state; try again shortly.');
  }
  let selected = profile;
  if (allowLan) {
    if (!await actions.confirmLan()) return null;
    if (profile.runtime.bind_address !== '0.0.0.0') {
      selected = { ...profile, runtime: { ...profile.runtime, bind_address: '0.0.0.0' } };
      await actions.save(selected);
    }
  }
  // Rebind LAN even when the saved profile already says 0.0.0.0: an earlier
  // failed enable may have saved that value while the listener stayed loopback.
  const ready = allowLan && status.state === 'running'
    ? await actions.restart(profile.id)
    : status.state === 'running' ? status : await actions.start(profile.id);
  if (ready.state !== 'running') {
    throw new Error(ready.localMessage || 'The sharing listener did not start.');
  }
  return selected;
}
