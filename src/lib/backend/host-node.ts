import { setBackend } from "./index";
import { createNodeBackend } from "./node";

export function installHostBackend(): void {
  setBackend(createNodeBackend());
}

export const desktopWindowApi: typeof import("./desktop").desktopWindowApi | undefined = undefined;
