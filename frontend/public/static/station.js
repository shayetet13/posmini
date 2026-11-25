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
  const NOMODE_MS = 10000; // server not in browser-print mode: check again now and then
  const OFF_MS = 1500;
  // Cheap Bluetooth printers have a small buffer and no flow control: too much, too fast and they
  // lose bytes, then print the leftover picture data as random symbols. So send in small pieces
  // with a short pause, and let a sheet finish printing before the next one is sent.
  const SERIAL_CHUNK = 256;
  // How fast data goes to a serial / Bluetooth printer (bytes per second). A printer prints about
  // 20-30 KB/s and cannot say "stop" when its buffer is full, so the slow setting stays well under.
  const SPEEDS = { slow: 8000, normal: 15000, fast: 40000 };
  const DEFAULT_SPEED = 'slow';
  const WAKE_AFTER_IDLE_MS = 15000; // a printer that has dozed off loses the first bytes it gets
  const WAKE_BYTES = 12;
  const WAKE_SETTLE_MS = 250;
  const ROW_BYTES = 48; // 58 mm paper: 384 dots per row
  const ROWS_PER_SEC = 300; // slower than most printers, so we never run ahead of them
  const MAX_SETTLE_MS = 8000;
  const USB_CHUNK = 16384;
  const REMOTE_TIMEOUT_MS = 12000;
  const BAUDS = [9600, 19200, 38400, 57600, 115200];

  // Web Bluetooth (Bluetooth LE) is what a phone or tablet running Chrome on Android has; Web Serial
  // (a paired Bluetooth / COM port) is for computers.
  const supported = { serial: 'serial' in navigator, usb: 'usb' in navigator, ble: 'bluetooth' in navigator };

  // Bluetooth LE services the usual receipt printers use for receiving data.
  const BLE_SERVICES = [
    '000018f0-0000-1000-8000-00805f9b34fb', '0000ff00-0000-1000-8000-00805f9b34fb',
    '0000ffe0-0000-1000-8000-00805f9b34fb', '0000fff0-0000-1000-8000-00805f9b34fb',
    '49535343-fe7d-4ae5-8fa9-9fafd205e455', 'e7810a71-73ae-499d-8c15-faa9aef0c3f2',
    '0000fee7-0000-1000-8000-00805f9b34fb', '0000ae30-0000-1000-8000-00805f9b34fb',
  ];
  // Bytes per Bluetooth LE write: big when the link allows it, falling back to the 20 that always fit.
  const BLE_CHUNKS = [180, 100, 20];
  let bleDevice = null; // the printer picked in this session (a page reload needs a new pick, or a saved permission)
  let bleChar = null;
  let bleChunk = BLE_CHUNKS[0];
  // Timers run in a worker: a browser slows page timers to one per second (or minute) once the tab
  // is in the background, which would stretch every pause and stall printing while the cashier
  // works in another window. Worker timers are not slowed.
  const sleep = (() => {
    try {
      const url = URL.createObjectURL(new Blob(['onmessage=(e)=>setTimeout(()=>postMessage(e.data.id),e.data.ms)'], { type: 'text/javascript' }));
      const worker = new Worker(url);
      const waiting = new Map();
      let next = 0;
      worker.onmessage = (e) => { const done = waiting.get(e.data); waiting.delete(e.data); if (done) done(); };
      return (ms) => new Promise((resolve) => { const id = ++next; waiting.set(id, resolve); worker.postMessage({ id, ms }); });
    } catch (_) {
      return (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    }
  })();
  const newId = () => (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + Math.random().toString(16).slice(2));

  function loadCfg() {
    let saved = {};
    try { saved = JSON.parse(localStorage.getItem(KEY) || '{}'); } catch (_) { /* private mode */ }
    return { id: newId(), kind: 'serial' in navigator || !('bluetooth' in navigator) ? 'serial' : 'ble', baud: 115200, speed: DEFAULT_SPEED, on: false, info: null, ...saved };
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
        const pause = Math.round((SERIAL_CHUNK * 1000) / (SPEEDS[cfg.speed] || SPEEDS[DEFAULT_SPEED]));
        for (let i = 0; i < bytes.length; i += SERIAL_CHUNK) {
          await writer.write(bytes.subarray(i, i + SERIAL_CHUNK));
          await sleep(pause);
        }
      },
      async close() {
        try { writer?.releaseLock(); } catch (_) { /* already released */ }
        try { await handle?.close(); } catch (_) { /* already closed */ }
      },
    },
    ble: {
      async request() {
        const dev = await navigator.bluetooth.requestDevice({ acceptAllDevices: true, optionalServices: BLE_SERVICES });
        bleDevice = dev;
        return { id: dev.id, name: dev.name || '' };
      },
      async find() {
        if (bleDevice && (!cfg.info || bleDevice.id === cfg.info.id)) return bleDevice;
        // A printer the browser was allowed to use before (Chrome keeps the permission on Android).
        if (!navigator.bluetooth.getDevices) return null;
        const devs = await navigator.bluetooth.getDevices();
        return devs.find((d) => cfg.info && d.id === cfg.info.id) || (devs.length === 1 ? devs[0] : null);
      },
      async open(dev) {
        bleDevice = dev;
        dev.removeEventListener('gattserverdisconnected', onBleLost);
        dev.addEventListener('gattserverdisconnected', onBleLost);
        const server = await dev.gatt.connect();
        const seen = [];
        let best = null;
        for (const service of await server.getPrimaryServices()) {
          for (const ch of await service.getCharacteristics()) {
            seen.push(service.uuid.slice(4, 8) + '/' + ch.uuid.slice(4, 8));
            const canWrite = ch.properties.writeWithoutResponse || ch.properties.write;
            if (canWrite && (!best || (!best.properties.writeWithoutResponse && ch.properties.writeWithoutResponse))) best = ch;
          }
        }
        if (!best) {
          try { dev.gatt.disconnect(); } catch (_) { /* already gone */ }
          throw new Error('เจอ “' + (dev.name || 'อุปกรณ์') + '” แต่ไม่มีช่องรับข้อมูลพิมพ์ (บริการที่เจอ: ' + (seen.join(', ') || 'ไม่มี') + ') เครื่องพิมพ์รุ่นนี้อาจเป็น Bluetooth แบบคลาสสิก ซึ่งเบราว์เซอร์มือถือเชื่อมต่อไม่ได้');
        }
        bleChar = best;
        handle = dev;
      },
      async write(bytes) {
        const speed = SPEEDS[cfg.speed] || SPEEDS[DEFAULT_SPEED];
        for (let i = 0; i < bytes.length;) {
          const piece = bytes.subarray(i, i + bleChunk);
          try {
            if (bleChar.properties.writeWithoutResponse) await bleChar.writeValueWithoutResponse(piece);
            else await bleChar.writeValue(piece);
          } catch (err) {
            const smaller = BLE_CHUNKS.find((n) => n < bleChunk);
            if (!smaller || !bleDevice.gatt.connected) throw err; // not a size problem
            bleChunk = smaller; // too long for this link: try shorter pieces
            continue;
          }
          i += piece.length;
          await sleep(Math.round((piece.length * 1000) / speed));
        }
      },
      async close() {
        bleChar = null;
        try { bleDevice?.gatt?.disconnect(); } catch (_) { /* already gone */ }
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

  // The printer went out of range or switched off: forget the open link so the next sheet reconnects.
  function onBleLost() {
    bleChar = null;
    if (cfg.kind === 'ble') handle = null;
  }

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
  let lastWrite = 0;
  function writeNow(bytes) {
    const run = busy.then(async () => {
      cfg = loadCfg();
      if (!(await ensureOpen())) throw new Error('ยังไม่ได้เชื่อมต่อเครื่องพิมพ์ในเครื่องนี้');
      try {
        // After a quiet spell, first send a few harmless bytes: if the printer is asleep it eats those.
        if (Date.now() - lastWrite > WAKE_AFTER_IDLE_MS) {
          await transport().write(new Uint8Array(WAKE_BYTES));
          await sleep(WAKE_SETTLE_MS);
        }
        await transport().write(bytes);
        lastWrite = Date.now();
      } catch (err) { await closePort(); throw err; }
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
    let finished = false;
    const finish = () => { if (!finished) { finished = true; wakeUp = null; resolve(); } };
    wakeUp = finish;
    sleep(ms).then(finish);
  });
  // Coming back to this window: look for work right away.
  document.addEventListener('visibilitychange', () => { if (!document.hidden) Station.wake(); });

  // Does the server hand sheets to browsers? Asked quietly (a plain 200) so a server set to
  // another print mode is not hit with a refused request every few seconds.
  async function serverIsWeb() {
    const res = await fetch('/api/print/queue', { credentials: 'same-origin' });
    if (res.status === 401) { location.href = '/login'; return false; }
    const info = await res.json().catch(() => ({}));
    return info.web === true;
  }

  async function lead() {
    Station.leader = true;
    let serverWeb = null; // unknown until asked
    for (;;) {
      cfg = loadCfg();
      if (!cfg.on) { setStatus('off'); await sleep(OFF_MS); continue; }
      let job = null;
      try {
        if (serverWeb !== true) {
          serverWeb = await serverIsWeb();
          if (!serverWeb) { setStatus('nomode', 'ตั้งโหมดพิมพ์เป็น “พิมพ์ผ่านเบราว์เซอร์” ในหน้าตั้งค่า > เครื่องพิมพ์ ก่อน'); await waitWake(NOMODE_MS); continue; }
        }
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
        const sheet = Station.fromBase64(job.bytes);
        await writeNow(sheet);
        // The printer is still working through its buffer; do not start the next sheet on top of it.
        await sleep(Math.min(MAX_SETTLE_MS, (sheet.length / ROW_BYTES) * 1000 / ROWS_PER_SEC));
        await api('POST', '/api/print/station/done', { job_id: job.job_id });
        job = null;
        setStatus('ready');
        if (data.pause_ms) await sleep(data.pause_ms);
      } catch (err) {
        serverWeb = null; // the print mode may have changed: look again before the next claim
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
    if (!supported[kind]) throw new Error(kind === 'ble' ? 'เบราว์เซอร์นี้ไม่รองรับ Bluetooth LE ใช้ Chrome บน Android' : 'เบราว์เซอร์นี้ไม่รองรับการเชื่อมต่อแบบนี้ ใช้ Chrome หรือ Edge');
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
  Station.setSpeed = function setSpeed(speed) {
    if (!SPEEDS[speed]) return;
    cfg = { ...loadCfg(), speed };
    saveCfg(cfg);
    channel?.postMessage({ type: 'config' });
  };
  Station.speeds = { slow: 'ช้า (แนะนำสำหรับ Bluetooth)', normal: 'ปกติ', fast: 'เร็ว (สาย USB-Serial)' };
  Station.setBaud = function setBaud(baud) {
    cfg = { ...loadCfg(), baud };
    saveCfg(cfg);
    channel?.postMessage({ type: 'config' });
  };

  window.Station = Station;
})();
