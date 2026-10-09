/** Desktop-only window operations are isolated from the shared UI. */
import { isDesktopWindow } from '$lib/stores/glass';
export async function newAppWindow() {
  if (!isDesktopWindow()) {
    if (!window.open(window.location.href, '_blank', 'noopener')) return;
    return;
  }
  const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
  await new Promise<void>((resolve, reject) => {
    const view = new WebviewWindow(`workspace-${crypto.randomUUID()}`, {url: '/', title:'Coding Tools MCP', width:1200, height:800, decorations:false, transparent:true});
    void view.once('tauri://created', () => resolve()).catch(reject);
    void view.once('tauri://error', event => reject(new Error(String(event.payload)))).catch(reject);
  });
}
export async function closeAppWindow() {
  if (isDesktopWindow()) { const {getCurrentWindow}=await import('@tauri-apps/api/window'); await getCurrentWindow().close(); }
  else window.close();
}
export async function quitApp() {
  if (isDesktopWindow()) { const {invoke}=await import('@tauri-apps/api/core'); await invoke('quit_app'); }
  else window.close();
}
export async function toggleFullscreen() {
  if (isDesktopWindow()) { const {getCurrentWindow}=await import('@tauri-apps/api/window'); const win=getCurrentWindow(); await win.setFullscreen(!await win.isFullscreen()); }
  else if (document.fullscreenElement) await document.exitFullscreen();
  else await document.documentElement.requestFullscreen();
}
