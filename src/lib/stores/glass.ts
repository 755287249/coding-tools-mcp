import { browser } from "$app/environment";
import { derived, writable } from "svelte/store";

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

export type GlassEffectKind = "acrylic" | "blur";

/**
 * Which native backdrop is in use. window-vibrancy's acrylic goes through the
 * legacy SetWindowCompositionAttribute API on Windows 10 and Windows 11 21H2
 * (build < 22523), which makes window dragging stutter badly. On those builds we
 * use the classic blur instead (smooth there); newer builds get the DWM system
 * backdrop acrylic, which drags smoothly.
 */
export const glassEffect = writable<GlassEffectKind | null>(null);

/** First Windows build where acrylic uses the smooth DWM system backdrop. */
const DWM_ACRYLIC_BUILD = 22523;

export function effectForBuild(build: number | null | undefined): GlassEffectKind {
  if (typeof build !== "number" || !Number.isFinite(build) || build <= 0) return "acrylic";
  return build >= DWM_ACRYLIC_BUILD ? "acrylic" : "blur";
}

let effectKind: Promise<GlassEffectKind> | null = null;

function resolveEffectKind(): Promise<GlassEffectKind> {
  effectKind ??= import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke<number | null>("get_windows_build"))
    .then(effectForBuild)
    .catch(() => "acrylic" as const)
    .then((kind) => {
      glassEffect.set(kind);
      return kind;
    });
  return effectKind;
}

/**
 * Native backdrop blur behind the window. It can be switched off entirely for
 * the smoothest possible dragging on slow machines.
 */
export const glassBlur = writable<boolean>(initialBlur());

if (browser) {
  glassBlur.subscribe((enabled) => {
    try {
      localStorage.setItem(BLUR_KEY, enabled ? "1" : "0");
    } catch {
      // Ignore storage failures.
    }
  });
}

/** The backdrop is only worth its cost when enabled and the tint is see-through. */
const backdropWanted = derived(
  [glassBlur, glassOpacity],
  ([enabled, opacity]) => enabled && clamp(opacity) < GLASS_MAX,
);

if (browser && isDesktopWindow()) {
  let applied: boolean | null = null;
  backdropWanted.subscribe((wanted) => {
    if (wanted === applied) return;
    applied = wanted;
    void Promise.all([import("@tauri-apps/api/window"), resolveEffectKind()])
      .then(([{ getCurrentWindow, Effect }, kind]) => {
        // A newer toggle may have landed while the build lookup was pending.
        if (applied !== wanted) return;
        return wanted
          ? getCurrentWindow().setEffects({ effects: [kind === "acrylic" ? Effect.Acrylic : Effect.Blur] })
          : getCurrentWindow().clearEffects();
      })
      .catch(() => undefined);
  });
}
