import { browserInvoke } from "./browser-session";
import { getCurrentWindow, Effect } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { invoke } from "@tauri-apps/api/core";
import { confirm, message, open } from "@tauri-apps/plugin-dialog";
import { setBackend } from "./index";
import { createTauriBackend } from "./tauri";
import type { InvokeFn } from "./types";

export function installDesktopBackend(): void {
  const remote = typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);
  const backend = createTauriBackend({
      invoke: (remote ? browserInvoke : (cmd, args) => invoke(cmd, args)) as InvokeFn,
      dialog: {
        open: remote ? async () => null : open,
        confirm: remote ? async (text) => window.confirm(text) : confirm,
        async message(text, options) {
          if (remote) window.alert(text);
          else await message(text, options);
        },
      },
    });
  if (remote) {
    backend.native.openExternal = async url => {
      const target = new URL(url);
      if (!["https:", "http:"].includes(target.protocol)) throw new Error("HTTP(S) links only");
      window.open(target.href, "_blank", "noopener,noreferrer");
    };
  }
  setBackend(remote ? { ...backend, capabilities: { ...backend.capabilities, nativeDirectoryPicker: false, openNativePath: false, workspaceLifecycle: false } } : backend);
}

/** Native APIs are exported only by the desktop host adapter. */
export const desktopWindowApi = {
  getCurrentWindow, Effect, WebviewWindow, invoke,
  available: () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window,
};
