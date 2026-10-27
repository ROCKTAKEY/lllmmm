(async () => {
  const autoRefresh = document.getElementById('auto-refresh');
  const unload = document.getElementById('unload');
  if (!unload || unload.disabled) {
    throw new Error('Load a test model before running the management browser regression.');
  }
  const wasRefreshing = autoRefresh.checked;
  autoRefresh.checked = false;
  let requests = 0;
  const interceptUnload = (event) => {
    if (event.detail.requestConfig.path !== '/admin/ui/unload') return;
    requests++;
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  let onSwap;
  let deadline;
  document.body.addEventListener('htmx:beforeRequest', interceptUnload, true);
  try {
    await new Promise((resolve, reject) => {
      deadline = setTimeout(() => reject(new Error('Dashboard refresh timed out')), 5000);
      onSwap = (event) => {
        if (event.detail.target.id !== 'dashboard') return;
        document.body.removeEventListener('htmx:afterSwap', onSwap);
        document.getElementById('unload').click();
        resolve();
      };
      document.body.addEventListener('htmx:afterSwap', onSwap);
      htmx.trigger(document.body, 'refresh');
    });
    if (requests !== 1) {
      throw new Error(`Enabled unload button must issue one request immediately after refresh; got ${requests}`);
    }
    return 'PASS: unload works immediately after dashboard refresh';
  } finally {
    clearTimeout(deadline);
    document.body.removeEventListener('htmx:afterSwap', onSwap);
    document.body.removeEventListener('htmx:beforeRequest', interceptUnload, true);
    autoRefresh.checked = wasRefreshing;
  }
})()
