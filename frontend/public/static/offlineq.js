(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.OfflineQ = api;
})(typeof self !== 'undefined' ? self : this, function () {
  // Cash sales made while the till had no connection. They wait here, in the browser's storage,
  // and are sent one by one, oldest first, when the connection is back. Each carries its own
  // `client_id`, so the server turns a repeated send (answer lost, tab reloaded) into one order.
  const KEY = 'pm-offline-sales';
  const MAX_SALES = 200;

  function newId(random) {
    const bytes = random || ((n) => Array.from({ length: n }, () => Math.floor(Math.random() * 256)));
    return 'till-' + bytes(12).map((b) => b.toString(16).padStart(2, '0')).join('');
  }

  // A failed fetch has no HTTP status: that is "no connection". A server answer has one.
  function isNoConnection(err) {
    return !!err && err.status == null;
  }

  // Answers that will never change by trying again (bad item, sold out, cash not enough).
  function isRefusal(err) {
    return !!err && typeof err.status === 'number' && err.status >= 400 && err.status < 500 && err.status !== 408 && err.status !== 429;
  }

  function create(storage, opts = {}) {
    const now = opts.now || (() => new Date());
    const read = () => {
      try {
        const list = JSON.parse(storage.getItem(KEY) || '[]');
        return Array.isArray(list) ? list : [];
      } catch (_) { return []; }
    };
    const write = (list) => {
      try { storage.setItem(KEY, JSON.stringify(list)); return true; } catch (_) { return false; }
    };

    return {
      list: read,
      waiting: () => read().filter((s) => !s.failed),
      failed: () => read().filter((s) => s.failed),

      // Saves a sale; false when the browser's storage is full or blocked (the sale is then NOT kept).
      add(sale) {
        const list = read();
        if (list.length >= MAX_SALES) return null;
        const entry = {
          client_id: newId(opts.random),
          at: now().toISOString(),
          ...sale,
        };
        return write([...list, entry]) ? entry : null;
      },

      remove(clientId) {
        write(read().filter((s) => s.client_id !== clientId));
      },

      // Sends every waiting sale in order. Stops at the first lost connection (the rest would fail
      // the same way); a sale the server refuses is kept, marked with the reason, for the cashier.
      // Returns { sent, failed, stopped }.
      async flush(send) {
        let sent = 0;
        let failed = 0;
        let stopped = false;
        for (const sale of read().filter((s) => !s.failed)) {
          try {
            await send(sale);
            this.remove(sale.client_id);
            sent += 1;
          } catch (err) {
            if (isNoConnection(err) || !isRefusal(err)) { stopped = true; break; }
            write(read().map((s) => (s.client_id === sale.client_id ? { ...s, failed: err.message || 'ถูกปฏิเสธ' } : s)));
            failed += 1;
          }
        }
        return { sent, failed, stopped };
      },
    };
  }

  return { create, newId, isNoConnection, isRefusal, KEY, MAX_SALES };
});
