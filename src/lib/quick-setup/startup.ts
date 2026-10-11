import type { RuntimeState } from '$lib/types';

/** Decide once after startup discovery; unknown or transitioning runtimes stay in place. */
export function startupSetupTarget(
  path: string,
  workspaceIds: string[],
  states: Partial<Record<string, RuntimeState>>,
  lastWorkspaceId?: string | null,
): string | null {
  const match = path.match(/^\/workspace\/([^/]+)\/?$/);
  if (path !== '/' && !match) return null;
  const requested = match ? decodeURIComponent(match[1]) : null;
  if (requested && !workspaceIds.includes(requested)) return null;
  const candidates = requested ? [requested] : workspaceIds;
  if (candidates.some(id => states[id] !== 'stopped' && states[id] !== 'error')) return null;
  const id = requested ?? (lastWorkspaceId && workspaceIds.includes(lastWorkspaceId) ? lastWorkspaceId : workspaceIds[0]);
  return id ? `/quick-setup?workspace=${encodeURIComponent(id)}` : '/quick-setup';
}
