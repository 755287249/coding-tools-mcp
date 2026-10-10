import { writable } from "svelte/store";

/** Opening a task sidebar is a per-page action, never a startup preference. */
export const activityPanelOpen = writable(false);
export const activityPanelExpanded = writable(false);

export function resetActivityPanel(): void {
  activityPanelOpen.set(false);
  activityPanelExpanded.set(false);
}
