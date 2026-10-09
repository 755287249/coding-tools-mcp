import { getBackend } from "$lib/backend";
import type { AlertOptions, ConfirmOptions, PickDirectoryOptions } from "$lib/backend";

export type { AlertOptions, ConfirmOptions, PickDirectoryOptions };

export function pickDirectory(
  options?: PickDirectoryOptions,
): Promise<string | string[] | null> {
  return getBackend().native.pickDirectory(options);
}

export function confirm(message: string, options?: ConfirmOptions): Promise<boolean> {
  return getBackend().native.confirm(message, options);
}

export function alert(message: string, options?: AlertOptions): Promise<void> {
  return getBackend().native.alert(message, options);
}

export function openExternal(url: string): Promise<void> {
  return getBackend().native.openExternal(url);
}

/** @deprecated use alert() */
export const message = alert;
