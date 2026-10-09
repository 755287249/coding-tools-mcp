export const MAX_PREVIEW_BYTES = 256 * 1024;
export function previewLanguage(label: string): 'html' | 'javascript' | null {
  const language = label.trim().toLowerCase().replace(/^.*\./, '');
  if (['html', 'htm', 'svg'].includes(language)) return 'html';
  if (['javascript', 'js', 'mjs'].includes(language)) return 'javascript';
  return null;
}
export function isTextFile(name: string): boolean {
  return /\.(html?|svg|[cm]?js|jsx|tsx?|json|css|md|txt|log|csv|ya?ml|toml|xml|py|rs|sh|sql)$/i.test(name);
}
export function buildPreviewDocument(code: string, language: 'html' | 'javascript'): string {
  if (new TextEncoder().encode(code).length > MAX_PREVIEW_BYTES) throw new Error('Preview exceeds 256 KiB');
  const policy = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'">`;
  // The policy is placed before user HTML and cannot be relaxed by later meta tags.
  const head = `<!doctype html><html><head><meta charset="utf-8">${policy}<meta name="referrer" content="no-referrer"></head><body>`;
  if (language === 'html') return head + code + '</body></html>';
  const quoted = JSON.stringify(code).replace(/</g, '\\u003c').replace(/\u2028/g, '\\u2028').replace(/\u2029/g, '\\u2029');
  return head + `<pre id="console" style="white-space:pre-wrap;font:13px/1.6 monospace"></pre><script>
const output=document.getElementById('console');
const print=(...values)=>{const line=document.createElement('div');line.textContent=values.map(v=>{try{return typeof v==='string'?v:JSON.stringify(v)}catch{return String(v)}}).join(' ');output.appendChild(line)};
for(const level of ['log','info','warn','error','debug']) console[level]=print;
addEventListener('error',e=>print(e.message));
addEventListener('unhandledrejection',e=>print(String(e.reason)));
try{(0,eval)(${quoted})}catch(e){print(String(e))}
</script></body></html>`;
}
