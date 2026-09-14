import type { FolderRuntime, ProcessSession, ToolContext } from '../types.js';
import { ProcessToolError } from './errors.js';
import { removeDurableSessionArtifacts } from './durableSession.js';

export const MAX_RETAINED_FINALIZED_SESSIONS = 128;
export const FINALIZED_SESSION_RETENTION_MS = 60 * 60_000;

function processRuntime(value: ToolContext | FolderRuntime): FolderRuntime {
  if ('folderId' in value) return value;
  const runtime = value.folderRuntimes.values().next().value;
  if (!runtime) throw new Error('WORKSPACE_FOLDER_NOT_FOUND');
  return runtime;
}

export function removeProcessSession(value: ToolContext | FolderRuntime, sessionId: string): boolean {
  const runtime = processRuntime(value);
  const session = runtime.sessions.get(sessionId);
  if (!session) return false;
  if (session.timeoutTimer) clearTimeout(session.timeoutTimer);
  if (session.detachedTimer) clearTimeout(session.detachedTimer);
  if (session.durableMonitor) clearInterval(session.durableMonitor);
  session.lockRelease?.();
  runtime.sessions.delete(sessionId);
  for (const [fingerprintValue, indexedSessionId] of runtime.operationsByFingerprint) {
    if (indexedSessionId === sessionId) runtime.operationsByFingerprint.delete(fingerprintValue);
  }
  void removeDurableSessionArtifacts(session).catch(() => { /* best-effort retained artifact cleanup */ });
  return true;
}

export function pruneProcessSessions(value: ToolContext | FolderRuntime, now = Date.now()): void {
  const runtime = processRuntime(value);
  for (const session of [...runtime.sessions.values()]) {
    if (session.finalizedAt && now - session.finalizedAt >= FINALIZED_SESSION_RETENTION_MS) removeProcessSession(runtime, session.id);
  }
  const finalized = [...runtime.sessions.values()]
    .filter(session => session.finalizedAt !== undefined)
    .sort((left, right) => (left.finalizedAt ?? 0) - (right.finalizedAt ?? 0));
  for (const session of finalized.slice(0, Math.max(0, finalized.length - MAX_RETAINED_FINALIZED_SESSIONS))) {
    removeProcessSession(runtime, session.id);
  }
}

export function touchSessionAttachment(session: ProcessSession): void {
  session.attachmentGeneration += 1;
  session.detachedGeneration = 0;
  if (session.detachedTimer) clearTimeout(session.detachedTimer);
  session.detachedTimer = undefined;
}

export function requireProcessSession(value: ToolContext | FolderRuntime, rawSessionId: unknown, touch = true): ProcessSession {
  const runtime = processRuntime(value);
  pruneProcessSessions(runtime);
  const sessionId = String(rawSessionId ?? '').trim();
  const outputReference = /^output:\/\/([^/]+)\/(stdout|stderr)$/.exec(sessionId);
  if (outputReference) {
    throw new ProcessToolError('OUTPUT_REF_USED_AS_SESSION_ID', 'output_ref cannot be used as session_id', 'runtime', true, {
      received: sessionId,
      corrected_session_id: outputReference[1],
      suggestion: 'Use the corrected_session_id with wait_command, send_input, or kill_session.'
    });
  }
  const session = runtime.sessions.get(sessionId);
  if (!session) throw new ProcessToolError('SESSION_NOT_FOUND', `Session not found: ${sessionId}`, 'not_found', false);
  if (touch) touchSessionAttachment(session);
  return session;
}

export function findProcessOperation(
  value: ToolContext | FolderRuntime,
  operationId: string,
  fingerprintValue: string
): { session?: ProcessSession; resolvedBy?: string } {
  const runtime = processRuntime(value);
  pruneProcessSessions(runtime);
  if (operationId) {
    const session = [...runtime.sessions.values()].find(candidate => candidate.operationId === operationId);
    if (session) return { session, resolvedBy: 'operation_id' };
  }
  if (fingerprintValue) {
    const sessionId = runtime.operationsByFingerprint.get(fingerprintValue);
    const session = sessionId ? runtime.sessions.get(sessionId) : undefined;
    if (session) return { session, resolvedBy: 'fingerprint' };
  }
  return {};
}
