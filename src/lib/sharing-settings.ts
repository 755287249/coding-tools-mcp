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

const GLOBAL_MCP_ID = '00000000000000000000000000000001';
/** Shared MCP connection first: its tunnel is the one the user configures. */
export function preferredSharingProfile(profiles: WorkspaceProfile[], restoredId: string): string {
  return profiles.find(p => p.id === GLOBAL_MCP_ID)?.id ?? (restoredId || profiles[0]?.id || '');
}

/**
 * A temporary tunnel gets a new address on every start. Replace the saved
 * public origin with the tunnel's current one so the share link keeps working.
 */
export function refreshedOrigins(origins: string[], stalePublic: string, currentPublic: string): string[] | null {
  if (!currentPublic || currentPublic === stalePublic) return null;
  const kept = origins.filter(origin => origin !== stalePublic);
  return kept.includes(currentPublic) ? null : [...kept, currentPublic];
}
