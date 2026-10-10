import type { ShareStatus } from './api/distribution';
import type { WorkspaceProfile } from './types';

/** Restore the form from the active sharing origins without changing the listener. */
export function restoreSharingSettings(status: ShareStatus, profiles: WorkspaceProfile[]) {
  const origins = status.enabled ? status.origins.flatMap(value => {
    try { return [new URL(value)]; } catch { return []; }
  }) : [];
  const local = origins.find(url => url.hostname === '127.0.0.1' || url.hostname === 'localhost');
  const lan = origins.find(url => url.hostname === status.lanIp);
  const profile = local && profiles.find(p => String(p.runtime.local_port) === local.port);
  const publicOrigin = origins.find(url => url !== local && url !== lan);
  return { id: (profile || profiles[0])?.id ?? '', lan: !!lan, publicUrl: publicOrigin?.origin ?? '' };
}
