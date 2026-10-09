/** Desktop-only window operations are isolated from the shared UI. */
import { desktopWindowApi } from '$lib/backend/host';
import { isDesktopWindow } from '$lib/stores/glass';
export async function newAppWindow() {
  if (!isDesktopWindow()) {
    if (!window.open(window.location.href, '_blank', 'noopener')) return;
    return;
  }
  const { WebviewWindow } = desktopWindowApi;
  await new Promise<void>((resolve, reject) => {
    const view = new WebviewWindow(`workspace-${crypto.randomUUID()}`, {url: '/', title:'Coding Tools MCP', width:1200, height:800, decorations:false, transparent:true});
    void view.once('tauri://created', () => resolve()).catch(reject);
    void view.once('tauri://error', event => reject(new Error(String(event.payload)))).catch(reject);
  });
}
export async function closeAppWindow() {
  if (isDesktopWindow()) { await getCurrentWindow().close(); }
  else window.close();
}
export async function quitApp() {
  if (isDesktopWindow()) { await desktopWindowApi.invoke('quit_app'); }
  else window.close();
}
export async function toggleFullscreen() {
  if (isDesktopWindow()) { const win=getCurrentWindow(); await win.setFullscreen(!await win.isFullscreen()); }
  else if (document.fullscreenElement) await document.exitFullscreen();
  else await document.documentElement.requestFullscreen();
}

export function getCurrentWindow() {
  if (!isDesktopWindow()) throw new Error('Desktop window is unavailable');
  return desktopWindowApi.getCurrentWindow();
}
