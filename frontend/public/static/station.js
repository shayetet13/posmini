// Print station: lets the browser on the computer that has the printer attached do the printing,
// so the server can live in the cloud. The server keeps the queue and draws each sheet; this
// asks for the next sheet, writes it to the printer (Web Serial for Bluetooth/COM printers, or
// WebUSB), and reports back. One tab per browser does the work (Web Locks); other tabs of the
// same browser pass test prints to it over a BroadcastChannel.
(function () {
  const { api } = PM;
  const KEY = 'posmini-station';
  const LOCK = 'posmini-station-lock';
  const RETRY_MS = 5000;
  const IDLE_MS = 3000;
  const OFF_MS = 1500;
  const SERIAL_CHUNK = 1024;
  const USB_CHUNK = 16384;
  const REMOTE_TIMEOUT_MS = 12000;
  const BAUDS = [9600, 19200, 38400, 57600, 115200];

  const supported = { serial: 'serial' in navigator, usb: 'usb' in navigator };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const newId = () => (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + Math.random().toString(16).slice(2));

  function loadCfg() {
    let saved = {};
    try { saved = JSON.parse(localStorage.getItem(KEY) || '{}'); } catch (_) { /* private mode */ }
    return { id: newId(), kind: 'serial', baud: 115200, on: false, info: null, ...saved };
  }
  function saveCfg(next) {
    try { localStorage.setItem(KEY, JSON.stringify(next)); } catch (_) { /* private mode */ }
  }
  let cfg = loadCfg();
  saveCfg(cfg); // keeps the station id stable

  // ------------------------------------------------------------ transports
  let handle = null; // open port / device
  let writer = null;
  let endpoint = null;

  const transports = {
    serial: {
      async request() {
        const port = await navigator.serial.requestPort();
        const { usbVendorId, usbProductId } = port.getInfo();
        return { vendor: usbVendorId ?? null, product: usbProductId ?? null };
      },
      async find() {
        const ports = await navigator.serial.getPorts();
        const want = cfg.info;
        return ports.find((p) => want && p.getInfo().usbVendorId === want.vendor && p.getInfo().usbProductId === want.product) || ports[0] || null;
      },
      async open(port) {
        try { await port.open({ baudRate: cfg.baud }); } catch (err) { if (err.name !== 'InvalidStateError') throw err; }
        writer = port.writable.getWriter();
        handle = port;
      },
      async write(bytes) {
        for (let i = 0; i < bytes.length; i += SERIAL_CHUNK) await writer.write(bytes.subarray(i, i + SERIAL_CHUNK));
      },
      async close() {
        try { writer?.releaseLock(); } catch (_) { /* already released */ }
        try { await handle?.close(); } catch (_) { /* already closed */ }
      },
    },
    usb: {
      async request() {
        const dev = await navigator.usb.requestDevice({ filters: [] });
        return { vendor: dev.vendorId, product: dev.productId };
      },
      async find() {
        const devs = await navigator.usb.getDevices();
        const want = cfg.info;
        return devs.find((d) => want && d.vendorId === want.vendor && d.productId === want.product) || devs[0] || null;
      },
      async open(dev) {
        await dev.open();
        if (dev.configuration === null) await dev.selectConfiguration(1);
        let pick = null;
        for (const iface of dev.configuration.interfaces) {
          for (const alt of iface.alternates) {
            const out = alt.endpoints.find((e) => e.direction === 'out' && e.type === 'bulk');
            if (out && (!pick || alt.interfaceClass === 7)) pick = { iface, alt, out };
          }
        }
        if (!pick) throw new Error('ไม่พบช่องส่งข้อมูลของเครื่องพิมพ์ USB นี้');
        await dev.claimInterface(pick.iface.interfaceNumber);
        if (pick.alt.alternateSetting) await dev.selectAlternateInterface(pick.iface.interfaceNumber, pick.alt.alternateSetting);
        endpoint = pick.out.endpointNumber;
        handle = dev;
      },
      async write(bytes) {
        for (let i = 0; i < bytes.length; i += USB_CHUNK) await handle.transferOut(endpoint, bytes.subarray(i, i + USB_CHUNK));
      },
      async close() {
        try { await handle?.close(); } catch (_) { /* already closed */ }
      },
    },
  };

  const transport = () => transports[cfg.kind] || transports.serial;

  async function ensureOpen() {
    if (handle) return true;
    const found = await transport().find();
    if (!found) return false;
    await transport().open(found);
    return true;
  }

  async function closePort() {
    await transport().close();
    handle = null;
    writer = null;
    endpoint = null;
  }

  // ----------------------------------------------------------------- state
  const listeners = new Set();
  const Station = {
    supported,
    bauds: BAUDS,
    status: 'off', // off | noport | ready | printing | error
    message: '',
    label: '',
    leader: false,
    cfg: () => ({ ...cfg }),
    onChange(fn) { listeners.add(fn); return () => listeners.delete(fn); },
  };
  const channel = 'BroadcastChannel' in window ? new BroadcastChannel('posmini-station') : null;

  function setStatus(status, message = '', label = '') {
    if (Station.status === status && Station.message === message && Station.label === label) return;
    Object.assign(Station, { status, message, label });
    listeners.forEach((fn) => fn(Station));
    channel?.postMessage({ type: 'status', status, message, label });
  }

  channel?.addEventListener('message', async (ev) => {
    const msg = ev.data || {};
    if (msg.type === 'status' && !Station.leader) {
      Object.assign(Station, { status: msg.status, message: msg.message, label: msg.label });
      listeners.forEach((fn) => fn(Station));
    } else if (msg.type === 'config') {
      cfg = loadCfg();
      if (Station.leader) await closePort(); // reopen with the new settings
    } else if (msg.type === 'print' && Station.leader) {
      let reply = { type: 'ack', id: msg.id, ok: true };
      try { await writeNow(msg.bytes); } catch (err) { reply = { type: 'ack', id: msg.id, ok: false, error: err.message }; }
      channel.postMessage(reply);
    }
  });

  // ------------------------------------------------------------- printing
  let busy = Promise.resolve();
  function writeNow(bytes) {
    const run = busy.then(async () => {
      cfg = loadCfg();
      if (!(await ensureOpen())) throw new Error('ยังไม่ได้เชื่อมต่อเครื่องพิมพ์ในเครื่องนี้');
      try { await transport().write(bytes); } catch (err) { await closePort(); throw err; }
    });
    busy = run.catch(() => {});
    return run;
  }

  /** Writes printer bytes from any tab of this browser (used for test pages). */
  Station.printBytes = function printBytes(bytes) {
    if (Station.leader) return writeNow(bytes);
    if (!channel) return Promise.reject(new Error('เบราว์เซอร์นี้ส่งข้อมูลระหว่างแท็บไม่ได้'));
    const id = newId();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { channel.removeEventListener('message', onAck); reject(new Error('ไม่มีแท็บสถานีพิมพ์ตอบกลับ ลองเปิดหน้าขายบนเครื่องนี้')); }, REMOTE_TIMEOUT_MS);
      function onAck(ev) {
        if (!ev.data || ev.data.type !== 'ack' || ev.data.id !== id) return;
        clearTimeout(timer);
        channel.removeEventListener('message', onAck);
        if (ev.data.ok) resolve(); else reject(new Error(ev.data.error || 'ส่งไปเครื่องพิมพ์ไม่สำเร็จ'));
      }
      channel.addEventListener('message', onAck);
      channel.postMessage({ type: 'print', id, bytes });
    });
  };

  Station.fromBase64 = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

  // ----------------------------------------------------------------- loop
  let wakeUp = null;
  Station.wake = () => { if (wakeUp) wakeUp(); };
  const waitWake = (ms) => new Promise((resolve) => {
    const timer = setTimeout(() => { wakeUp = null; resolve(); }, ms);
    wakeUp = () => { clearTimeout(timer); wakeUp = null; resolve(); };
  });

  async function lead() {
    Station.leader = true;
    for (;;) {
      cfg = loadCfg();
      if (!cfg.on) { setStatus('off'); await sleep(OFF_MS); continue; }
      let job = null;
      try {
        if (!(await ensureOpen())) { setStatus('noport'); await sleep(RETRY_MS / 2); continue; }
        const res = await fetch('/api/print/station/claim', {
          method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ station: cfg.id }),
        });
        if (res.status === 401) { location.href = '/login'; return; }
        if (res.status === 204) { setStatus('ready'); await waitWake(IDLE_MS); continue; }
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || 'ขอคิวพิมพ์ไม่สำเร็จ');
        job = data;
        setStatus('printing', '', job.queue_label || '');
        await writeNow(Station.fromBase64(job.bytes));
        await api('POST', '/api/print/station/done', { job_id: job.job_id });
        job = null;
        setStatus('ready');
        if (data.pause_ms) await sleep(data.pause_ms);
      } catch (err) {
        setStatus('error', err.message || String(err));
        if (job) await api('POST', '/api/print/station/fail', { job_id: job.job_id, message: err.message || 'พิมพ์ไม่สำเร็จ' }).catch(() => {});
        await closePort();
        await sleep(RETRY_MS);
      }
    }
  }

  // Exactly one tab per browser leads; the rest wait their turn in the lock queue.
  if (navigator.locks) navigator.locks.request(LOCK, () => lead());
  else lead();

  // ---------------------------------------------------------- user actions
  /** Must run from a click: asks the user to pick the printer, then starts the station. */
  Station.connect = async function connect(kind) {
    if (!supported[kind]) throw new Error('เบราว์เซอร์นี้ไม่รองรับการเชื่อมต่อแบบนี้ ใช้ Chrome หรือ Edge');
    let info;
    try { info = await transports[kind].request(); } catch (err) { if (err.name === 'NotFoundError') return; throw err; }
    await closePort();
    cfg = { ...loadCfg(), kind, info, on: true };
    saveCfg(cfg);
    channel?.postMessage({ type: 'config' });
    Station.wake();
  };
  Station.setOn = function setOn(on) {
    cfg = { ...loadCfg(), on };
    saveCfg(cfg);
    channel?.postMessage({ type: 'config' });
    Station.wake();
  };
  Station.setBaud = function setBaud(baud) {
    cfg = { ...loadCfg(), baud };
    saveCfg(cfg);
    channel?.postMessage({ type: 'config' });
  };

  window.Station = Station;
})();
