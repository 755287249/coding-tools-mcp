import { desktopWindowApi } from '$lib/backend/host';
import { browserInvoke } from '$lib/backend/browser-session';
import { getBackend } from '$lib/backend';
export const localDesktop = () => !!desktopWindowApi?.available();
export interface ShareStatus { enabled: boolean; origins: string[]; lanIp: string | null; password?: string | null }
export interface AppRelease { currentVersion: string; version: string; available: boolean; page: string; notes: string; assetUrl: string | null; sha256: string | null }
export async function distributionCall<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (localDesktop()) return desktopWindowApi!.invoke<T>(command, args);
  if (getBackend().capabilities.host === 'desktop') return browserInvoke<T>(command, args);
  throw new Error('This feature manages the native desktop application. Open it on the host computer.');
}
