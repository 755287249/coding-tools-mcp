import { browser } from "$app/environment";
import { writable } from "svelte/store";

/**
 * "auto" is the simplified, one-click experience (default).
 * "advanced" exposes every tab, tunnel option and policy setting.
 */
export type UiMode = "auto" | "advanced";

const STORAGE_KEY = "coding-tools.ui-mode";

function initialMode(): UiMode {
  if (!browser) return "auto";
  try {
    return localStorage.getItem(STORAGE_KEY) === "advanced" ? "advanced" : "auto";
  } catch {
    return "auto";
  }
}

export const uiMode = writable<UiMode>(initialMode());

if (browser) {
  uiMode.subscribe((mode) => {
    try {
      localStorage.setItem(STORAGE_KEY, mode);
    } catch {
      // Storage can be unavailable; the mode still applies for this session.
    }
  });
}

/**
 * Registered by the root layout so pages (e.g. the empty home screen) can
 * trigger the same "new connection" flow as the sidebar button.
 */
export const connectionActions: { newConnection: (() => void) | null } = {
  newConnection: null,
};
