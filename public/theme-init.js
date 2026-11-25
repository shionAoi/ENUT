// Blocking local script applies a valid saved preference before the first paint.
(() => {
  let saved = null;
  try { saved = localStorage.getItem('ai-monitor-theme'); } catch { /* Optional storage. */ }
  const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  document.documentElement.dataset.theme = saved === 'dark' || saved === 'light' ? saved : dark ? 'dark' : 'light';
})();
