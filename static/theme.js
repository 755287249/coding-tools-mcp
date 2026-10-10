// Runs before the first paint, including the login and startup screens.
(() => {
  const root = document.documentElement;
  const system = matchMedia('(prefers-color-scheme: dark)');
  const normalize = value => value === 'light' || value === 'dark' ? value : 'system';
  let preference = 'system';
  try { preference = normalize(localStorage.getItem('theme')); } catch {}
  function apply() {
    const resolved = preference === 'system' ? (system.matches ? 'dark' : 'light') : preference;
    root.dataset.themePreference = preference;
    root.dataset.theme = resolved;
    root.classList.toggle('dark', resolved === 'dark');
    root.style.colorScheme = resolved;
    window.dispatchEvent(new Event('ctmcp-theme-change'));
  }
  window.addEventListener('ctmcp-theme-preference', event => {
    preference = normalize(event.detail);
    try { localStorage.setItem('theme', preference); } catch {}
    apply();
  });
  window.addEventListener('storage', event => {
    if (event.key !== 'theme' && event.key !== null) return;
    preference = normalize(event.newValue);
    apply();
  });
  system.addEventListener('change', () => { if (preference === 'system') apply(); });
  apply();
})();
