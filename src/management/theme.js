'use strict';
(() => {
  let theme = 'system';
  try { theme = localStorage.getItem('lllmmm-theme') || 'system'; } catch {}
  if (['light', 'dark'].includes(theme)) document.documentElement.dataset.theme = theme;
})();
