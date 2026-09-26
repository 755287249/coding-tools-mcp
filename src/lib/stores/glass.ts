import { browser } from "$app/environment";
import { writable } from "svelte/store";

/**
 * Translucent "glass" window support. The desktop window is frameless and
 * transparent with a native acrylic backdrop; the app paints a tint layer on
 * top whose opacity the user controls (bottom-right slider).
 */

const STORAGE_KEY = "coding-tools.glass-opacity";
export const GLASS_MIN = 15;
export const GLASS_MAX = 100;
const GLASS_DEFAULT = 55;

/** True inside the Tauri desktop window (not the Node Agent web UI). */
export function isDesktopWindow(): boolean {
  return browser && typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function clamp(value: number): number {
  if (!Number.isFinite(value)) return GLASS_DEFAULT;
  return Math.min(GLASS_MAX, Math.max(GLASS_MIN, Math.round(value)));
}

function initialOpacity(): number {
  if (!browser) return GLASS_DEFAULT;
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return saved === null ? GLASS_DEFAULT : clamp(Number(saved));
  } catch {
    return GLASS_DEFAULT;
  }
}

/** Tint opacity in percent (15–100). 100 = solid. */
export const glassOpacity = writable<number>(initialOpacity());

if (browser) {
  glassOpacity.subscribe((value) => {
    const next = clamp(value);
    // CSSOM custom properties are allowed under the strict CSP (no inline style attribute).
    document.documentElement.style.setProperty("--glass-alpha", String(next / 100));
    try {
      localStorage.setItem(STORAGE_KEY, String(next));
    } catch {
      // Storage may be unavailable; the value still applies for this session.
    }
  });
  if (isDesktopWindow()) document.documentElement.classList.add("is-glass");
}

const BLUR_KEY = "coding-tools.glass-blur";

function initialBlur(): boolean {
  if (!browser) return true;
  try {
    return localStorage.getItem(BLUR_KEY) !== "0";
  } catch {
    return true;
  }
}

/**
 * Native acrylic blur behind the window. Acrylic can stutter while dragging on
 * some Windows 10 / early Windows 11 builds, so the user can switch it off.
 */
export const glassBlur = writable<boolean>(initialBlur());

if (browser && isDesktopWindow()) {
  let first = true;
  glassBlur.subscribe((enabled) => {
    try {
      localStorage.setItem(BLUR_KEY, enabled ? "1" : "0");
    } catch {
      // Ignore storage failures.
    }
    // The window starts with acrylic from tauri.conf.json; skip the redundant first apply.
    if (first && enabled) {
      first = false;
      return;
    }
    first = false;
    void import("@tauri-apps/api/window")
      .then(({ getCurrentWindow, Effect }) =>
        enabled ? getCurrentWindow().setEffects({ effects: [Effect.Acrylic] }) : getCurrentWindow().clearEffects(),
      )
      .catch(() => undefined);
  });
}
