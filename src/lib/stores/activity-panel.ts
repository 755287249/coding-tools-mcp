import { browser } from "$app/environment";
import { writable, type Writable } from "svelte/store";

/** Task panel (right drawer) preferences, remembered across restarts. */

function persisted(key: string, fallback: () => boolean): Writable<boolean> {
  let initial = false;
  if (browser) {
    try {
      const saved = localStorage.getItem(key);
      initial = saved === null ? fallback() : saved === "1";
    } catch {
      initial = fallback();
    }
  }
  const store = writable<boolean>(initial);
  if (browser) {
    store.subscribe((value) => {
      try {
        localStorage.setItem(key, value ? "1" : "0");
      } catch {
        // Storage may be unavailable; the value still applies for this session.
      }
    });
  }
  return store;
}

/** Open by default when the window is wide enough to show it next to the card. */
export const activityPanelOpen = persisted(
  "coding-tools.activity-panel-open",
  () => typeof window !== "undefined" && window.innerWidth >= 1000,
);

export const activityPanelExpanded = persisted("coding-tools.activity-panel-expanded", () => false);
