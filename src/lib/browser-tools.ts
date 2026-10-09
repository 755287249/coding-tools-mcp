/** getRandomValues also works on private-network HTTP origins. */
export function randomId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6]! & 15) | 64; bytes[8] = (bytes[8]! & 63) | 128;
  const hex = Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
}
export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) { try { await navigator.clipboard.writeText(text); return; } catch { /* Try the browser's local copy action. */ } }
  const active = document.activeElement as HTMLElement | null;
  const input = document.createElement('textarea'); input.value = text; input.style.cssText = 'position:fixed;left:-9999px;top:0'; document.body.append(input); input.select();
  try { if (!document.execCommand('copy')) throw new Error('Select and copy the displayed text manually.'); }
  finally { input.remove(); active?.focus({ preventScroll: true }); }
}
