(function () {
  const { $, h, baht, api, productPic, qrCard, toast, listen, Voice, keepAwake } = PM;
  const ALL = 'ทั้งหมด';
  const state = { printMode: 'off', printq: null, products: [], path: [], cat: ALL, cart: new Map(), orders: [], payId: null, printReady: false, verifyActive: false, expired: new Set() };
  let cartTimer;

  const productById = (id) => state.products.find((p) => p.id === id);
  const cartLines = () => [...state.cart].map(([id, qty]) => ({ p: productById(id), qty })).filter((l) => l.p);
  const cartTotal = () => cartLines().reduce((s, l) => s + l.p.price_satang * l.qty, 0);
  PM.cartTotal = () => cartTotal();
  const cartCount = () => cartLines().reduce((s, l) => s + l.qty, 0);

  function setTab(tab) {
    document.body.dataset.tab = tab;
    document.querySelectorAll('.tab').forEach((b) => b.classList.toggle('on', b.dataset.tab === tab));
  }

  // Groups open like folders: `state.path` is the chain of groups we are inside (empty = top).
  function goPath(path) {
    state.path = path;
    renderChips();
    renderGrid();
  }

  function renderChips() {
    if (state.path.length) { $('#chips').replaceChildren(...PM.menuCrumbs(state.products, state.path, goPath)); return; }
    const cats = [ALL, ...new Set(state.products.filter((p) => p.parent_id == null).map((p) => p.category).filter(Boolean))];
    $('#chips').replaceChildren(...cats.map((c) =>
      h('button', { class: 'chip' + (c === state.cat ? ' on' : ''), type: 'button', onclick: () => { state.cat = c; renderChips(); renderGrid(); } }, c)));
  }

  function renderGrid() {
    const list = PM.menuLevel(state.products, state.path, state.cat === ALL ? null : state.cat);
    const tile = (p) => PM.menuTile(p, { products: state.products, qty: state.cart.get(p.id), onAdd: (id) => addToCart(id, 1), onOpen: (id) => goPath([...state.path, id]) });
    $('#grid').replaceChildren(...(list.length ? list.map(tile)
      : [h('p', { class: 'muted empty' }, state.path.length ? 'กลุ่มนี้ยังไม่มีรายการ เพิ่มได้ที่ จัดการ › เมนูสินค้า' : 'ยังไม่มีสินค้าในหมวดนี้ เพิ่มได้ที่ จัดการ › เมนูสินค้า')]));
  }

  function renderCart() {
    const lines = cartLines();
    $('#lines').replaceChildren(...(lines.length
      ? lines.map(({ p, qty }) => h('div', { class: 'row' },
          h('span', { class: 'nm' }, p.name),
          h('span', { class: 'price num' }, baht(p.price_satang * qty)),
          h('div', { class: 'stepper' },
            h('button', { class: 'qb', type: 'button', 'aria-label': 'ลด ' + p.name, onclick: () => addToCart(p.id, -1) }, '−'),
            h('span', { class: 'qty num' }, qty),
            h('button', { class: 'qb', type: 'button', 'aria-label': 'เพิ่ม ' + p.name, onclick: () => addToCart(p.id, 1) }, '+'))))
      : [h('div', { class: 'cart-empty' }, PM.icon('bag', 22), h('span', null, 'ยังไม่มีสินค้า แตะเมนูเพื่อเริ่มขาย'))]));
    $('#cart').classList.toggle('empty', !lines.length);
    const total = baht(cartTotal());
    $('#sum').textContent = total;
    $('#barSum').textContent = total;
    $('#barCount').textContent = cartCount() + ' รายการ';
    $('#tabCart').textContent = cartCount() ? '(' + cartCount() + ')' : '';
    $('#clear').hidden = !lines.length;
  }

  function addToCart(id, delta) {
    if (delta > 0 && productById(id) && productById(id).sold_out) { toast('“' + productById(id).name + '” หมดแล้ว'); return; }
    const next = (state.cart.get(id) || 0) + delta;
    if (next <= 0) state.cart.delete(id);
    else state.cart.set(id, Math.min(next, 99));
    renderGrid();
    renderCart();
    clearTimeout(cartTimer);
    cartTimer = setTimeout(pushCart, 150);
  }

  function pushCart() {
    const items = cartLines().map((l) => ({ name: l.p.name, qty: l.qty, price_satang: l.p.price_satang }));
    api('POST', '/api/cart', { items }).catch(() => {});
  }

  function clearCart() {
    state.cart.clear();
    renderGrid();
    renderCart();
    pushCart();
  }

  async function pay(method, cashReceivedSatang) {
    if (!state.cart.size) { toast('ตะกร้าว่าง เลือกสินค้าก่อน'); return; }
    const items = [...state.cart].map(([product_id, qty]) => ({ product_id, qty }));
    const body = { method, items };
    if (cashReceivedSatang != null) body.cash_received_satang = cashReceivedSatang;
    try {
      const order = await api('POST', '/api/orders', body);
      clearCart();
      setTab('menu');
      await loadOrders();
      if (method === 'promptpay') openPay(order.id);
    } catch (err) { toast(err.message); }
  }

  const CANCEL_ASK = { title: 'ยกเลิกออเดอร์', okText: 'ยกเลิกออเดอร์', cancelText: 'ไม่ยกเลิก' };
  const STATUS = {
    pending_payment: { label: 'รอชำระ', tone: 'warn', cls: 's-pending' },
    paid: { label: 'ชำระแล้ว', tone: 'ok', cls: 's-paid' },
    called: { label: 'เรียกแล้ว', tone: 'ok', cls: 'called' },
  };
  const GROUPS = [['called', 'เรียกแล้ว · รอรับของ'], ['paid', 'ชำระแล้ว · รอเรียกคิว'], ['pending_payment', 'รอชำระเงิน']];

  async function printOrder(id, kind) {
    try {
      await api('POST', '/api/orders/' + id + '/print', { kind });
      toast('ส่งเข้าคิวพิมพ์แล้ว');
    } catch (err) { toast(err.message); }
  }

  function itemsText(o) { return o.items.map((i) => i.name + ' ×' + i.qty).join(', '); }

  const SKIP_ASK = { title: 'เรียกข้ามคิว', okText: 'เรียกข้าม', cancelText: 'ไม่เรียก' };

  // The server refuses to call a queue ahead of an earlier paid one until the cashier agrees.
  async function callOrder(id, force) {
    try {
      await api('POST', '/api/orders/' + id + '/call' + (force ? '?force=true' : ''));
    } catch (err) {
      if (err.status !== 409) { toast(err.message); return; }
      const ask = err.message + String.fromCharCode(10) + 'เรียกคิวนี้ข้ามไปเลยไหม?';
      if (await PM.confirm(ask, SKIP_ASK)) await callOrder(id, true);
    }
  }

  async function callNext() {
    try { await api('POST', '/api/orders/call-next'); } catch (err) { toast(err.message); }
  }

  async function act(id, action, confirmText, opts) {
    if (confirmText && !(await PM.confirm(confirmText, opts))) return;
    try {
      await api('POST', '/api/orders/' + id + '/' + action);
      if (action === 'repay') toast('สร้าง QR ใหม่แล้ว');
    } catch (err) { toast(err.message); }
  }

  function orderCard(o) {
    const meta = STATUS[o.status] || { label: o.status, tone: '', cls: '' };
    const btn = (cls, text, fn) => h('button', { class: 'btn small ' + cls, type: 'button', onclick: fn }, text);
    const cancel = (ask) => btn('danger', 'ยกเลิก', () => act(o.id, 'cancel', ask, CANCEL_ASK));
    let main;
    let more;
    if (o.status === 'pending_payment') {
      main = [btn('primary', 'เปิด QR / ยืนยัน', () => openPay(o.id))];
      more = [cancel('ยกเลิกออเดอร์นี้?')];
    } else {
      main = o.status === 'called'
        ? [btn('primary', 'รับของแล้ว', () => act(o.id, 'done')), btn('', 'เรียกซ้ำ', () => callOrder(o.id))]
        : [btn('ok', 'เรียกคิว', () => callOrder(o.id)), btn('', 'รับของแล้ว', () => act(o.id, 'done'))];
      more = state.printReady
        ? [btn('', 'พิมพ์บิล', () => printOrder(o.id, 'ticket')), btn('', 'พิมพ์ใบออเดอร์', () => printOrder(o.id, 'slip'))]
        : [btn('', 'ใบเสร็จ', () => window.open('/q/' + o.token + '?print=1', '_blank'))];
      more.push(cancel('ยกเลิกออเดอร์ที่ชำระแล้ว?'));
    }
    // Tapping the queue number or the items opens the whole order in a big box (no printer needed).
    const open = { role: 'button', tabindex: '0', 'aria-label': 'ดูออเดอร์ ' + (o.queue_label || ''), onclick: () => openOrderView(o.id),
      onkeydown: (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openOrderView(o.id); } } };
    return h('div', { class: 'qcard ' + meta.cls },
      h('div', { class: 'qh tap', ...open },
        h('span', { class: 'ql num' }, o.queue_label || '—'),
        h('span', { class: 'badge ' + meta.tone }, meta.label),
        h('span', { style: 'flex:1' }),
        h('b', { class: 'num disp' }, baht(o.total_satang))),
      h('div', { class: 'items tap', ...open }, itemsText(o)),
      h('div', { class: 'acts' }, main),
      h('details', { class: 'more' }, h('summary', null, o.status === 'pending_payment' ? 'ยกเลิก' : 'พิมพ์ซ้ำ · ยกเลิก'), h('div', { class: 'acts2' }, more)));
  }

  // ---- one order in a big centred box: queue number, every item, total; closes with the button
  const PAY_LABEL = { cash: 'เงินสด', promptpay: 'พร้อมเพย์' };
  let orderView = null;

  function closeOrderView() {
    if (!orderView) return;
    document.removeEventListener('keydown', onOrderViewKey, true);
    orderView.remove();
    orderView = null;
  }
  function onOrderViewKey(e) { if (e.key === 'Escape') closeOrderView(); }

  function openOrderView(id) {
    const o = state.orders.find((x) => x.id === id);
    if (!o) return;
    closeOrderView();
    const meta = STATUS[o.status] || { label: o.status, tone: '' };
    const then = (fn) => async () => { closeOrderView(); await fn(); };
    const actions = o.status === 'paid'
      ? [h('button', { class: 'btn ok lg block', type: 'button', onclick: then(() => callOrder(o.id)) }, 'เรียกคิว ' + (o.queue_label || ''))]
      : o.status === 'called'
        ? [h('button', { class: 'btn primary lg block', type: 'button', onclick: then(() => act(o.id, 'done')) }, 'รับของแล้ว')]
        : [];
    const how = [PAY_LABEL[o.method] || o.method, o.source === 'self' ? 'ลูกค้าสั่งเอง' : 'สั่งหน้าร้าน'].join(' · ');
    orderView = h('div', { class: 'modal open order-modal' },
      h('div', { class: 'sheet orderbox', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'รายการออเดอร์' },
        h('div', { class: 'ob-head' },
          h('b', { class: 'ob-q num' }, o.queue_label || '—'),
          h('span', { class: 'badge ' + meta.tone }, meta.label)),
        h('div', { class: 'ob-no muted num' }, 'ออเดอร์ #' + o.id + ' · ' + how),
        h('div', { class: 'ob-items' }, ...o.items.map((i) => h('div', { class: 'ob-line' },
          h('span', { class: 'ob-qty num' }, i.qty + ' ×'),
          h('span', { class: 'ob-name' }, i.name),
          h('span', { class: 'ob-price num' }, baht(i.unit_price_satang * i.qty))))),
        h('div', { class: 'ob-total' }, h('span', null, 'รวม'), h('b', { class: 'num' }, baht(o.total_satang))),
        h('div', { class: 'btns' }, ...actions, h('button', { class: 'btn lg block', type: 'button', onclick: closeOrderView }, 'ปิด'))));
    orderView.addEventListener('click', (e) => { if (e.target === orderView) closeOrderView(); });
    document.addEventListener('keydown', onOrderViewKey, true);
    document.body.append(orderView);
  }

  function renderOrders() {
    const nodes = GROUPS.flatMap(([status, title]) => {
      // Waiting queues are listed by queue number (the order they paid), not by when the order was made.
      const list = state.orders.filter((o) => o.status === status).sort((a, b) => (a.queue_no ?? Infinity) - (b.queue_no ?? Infinity) || a.id - b.id);
      return list.length ? [h('div', { class: 'grp-title' }, title + ' (' + list.length + ')'), ...list.map(orderCard)] : [];
    });
    $('#qlist').replaceChildren(...(nodes.length
      ? nodes
      : [h('div', { class: 'cart-empty' }, PM.icon('ticket', 34), h('p', { class: 'muted' }, 'ยังไม่มีคิว'))]));
    const waiting = state.orders.filter((o) => o.status !== 'pending_payment').length;
    const nextUp = state.orders.filter((o) => o.status === 'paid').sort((a, b) => a.queue_no - b.queue_no)[0];
    $('#qnext').replaceChildren(...(nextUp
      ? [h('button', { class: 'btn small primary qnext', type: 'button', onclick: callNext }, 'เรียกถัดไป ', h('b', { class: 'num' }, nextUp.queue_label))]
      : []));
    $('#qcount').textContent = waiting ? '· ' + waiting + ' คิว' : '';
    $('#tabQueue').textContent = waiting ? '(' + waiting + ')' : '';
    renderPay();
  }

  const SHEET = { ticket: 'บัตรคิว', slip: 'ใบออเดอร์', receipt: 'ใบเสร็จ' };

  function renderPrintq() {
    const q = state.printq;
    const box = $('#printq');
    if (!q || (!q.queued && !q.failed)) { box.replaceChildren(); return; }
    const nodes = [];
    if (q.web && q.queued && !q.station_online) {
      nodes.push(h('div', { class: 'pq pq-bad', role: 'alert' },
        h('div', { class: 'pq-msg' }, h('b', null, 'ยังไม่มีสถานีพิมพ์'), h('span', null, 'เปิดหน้าขายบนคอมพิวเตอร์ที่ต่อเครื่องพิมพ์ แล้วกดเชื่อมต่อ'))));
    }
    if (q.stuck) {
      const what = [q.stuck.queue_label, SHEET[q.stuck.kind]].filter(Boolean).join(' · ');
      nodes.push(h('div', { class: 'pq pq-bad', role: 'alert' },
        h('div', { class: 'pq-msg' }, h('b', null, 'พิมพ์ไม่ออก ' + what), h('span', null, q.stuck.message + ' · ลองใหม่เองทุก 5 วินาที')),
        h('button', { class: 'btn small', type: 'button', onclick: skipPrint }, 'ข้ามใบนี้')));
    } else if (q.queued) {
      nodes.push(h('div', { class: 'pq' }, PM.icon('ticket', 16), 'รอพิมพ์ ' + q.queued + ' ใบ'));
    }
    if (q.failed) {
      nodes.push(h('div', { class: 'pq pq-warn' },
        h('span', null, 'มี ' + q.failed + ' ใบที่ข้ามไปและยังไม่ได้พิมพ์'),
        h('button', { class: 'btn small', type: 'button', onclick: retryPrint }, 'พิมพ์ที่ค้างอีกครั้ง')));
    }
    box.replaceChildren(...nodes);
  }

  async function skipPrint() {
    try { state.printq = await api('POST', '/api/print/queue/skip'); renderPrintq(); } catch (err) { toast(err.message); }
  }

  async function retryPrint() {
    try { state.printq = await api('POST', '/api/print/queue/retry'); renderPrintq(); } catch (err) { toast(err.message); }
  }

  // Printing through the browser (cloud server): this computer's browser drives the printer.
  const STATION_TEXT = {
    off: ['สถานีพิมพ์: ปิด', 'bad'],
    noport: ['ต่อเครื่องพิมพ์', 'bad'],
    ready: ['พร้อมพิมพ์', 'ok'],
    printing: ['กำลังพิมพ์', 'ok'],
    error: ['พิมพ์ไม่ได้', 'bad'],
    nomode: ['ตั้งโหมดพิมพ์', 'bad'],
  };

  function paintStation() {
    const pill = $('#stationPill');
    if (!pill) return;
    pill.hidden = state.printMode !== 'web' || !window.Station;
    if (pill.hidden) return;
    const [text, tone] = STATION_TEXT[Station.status] || STATION_TEXT.off;
    pill.className = 'pill station ' + tone;
    pill.textContent = '🖨 ' + text + (Station.label ? ' ' + Station.label : '');
    pill.title = Station.message || 'แตะเพื่อตั้งค่าเครื่องพิมพ์ของเครื่องนี้';
  }

  if (window.Station) {
    Station.onChange(paintStation);
    $('#stationPill').addEventListener('click', async () => {
      try {
        if (Station.status === 'off' || Station.status === 'noport') await Station.connect(Station.cfg().kind);
        else location.href = '/settings#print';
      } catch (err) { toast(err.message); }
    });
  }

  async function loadPrintq() {
    try { state.printq = await api('GET', '/api/print/queue'); renderPrintq(); } catch (_) { /* printing is optional */ }
  }

  async function loadOrders() {
    try { state.orders = await api('GET', '/api/orders/active'); } catch (err) { toast(err.message); return; }
    renderOrders();
  }

  // Settings can change on another screen, so ask again each time the payment box opens.
  async function refreshVerify() {
    try { state.verifyActive = (await api('GET', '/api/settings')).verify_active; } catch (_) { state.verifyActive = false; }
  }

  function openPay(id) {
    state.payId = id;
    $('#paymodal').classList.add('open');
    renderPay();
    refreshVerify().then(renderPay);
  }
  function closePay() { state.payId = null; $('#paymodal').classList.remove('open'); }

  function expiredView(o) {
    return [h('p', null, 'QR หมดอายุแล้ว'),
      h('p', { class: 'muted' }, 'QR ใช้ได้ 2 นาที ต้องสร้างใหม่ก่อนให้ลูกค้าสแกน'),
      h('button', { class: 'btn primary block', type: 'button', onclick: () => act(o.id, 'repay') }, 'สร้าง QR ใหม่')];
  }

  function payBody(o) {
    switch (o.pay_state) {
      case 'pending':
        if (state.expired.has(o.id) || o.qr_expires_in === 0) return expiredView(o);
        return [qrCard(o.qr_svg || '', { expiresIn: o.qr_expires_in, onExpire: () => { state.expired.add(o.id); renderPay(); } }),
          o.pay_note ? h('p', { class: 'warnbox' }, 'ยังตรวจยอดไม่ผ่าน: ' + o.pay_note) : null,
          h('p', { class: 'muted' }, 'ให้ลูกค้าสแกนจ่าย ระบบจะตรวจยอดเงินเข้าให้อัตโนมัติ…'),
          h('p', { class: 'okhint' }, 'ระหว่างรอ กด “ปิด แล้วขายคิวอื่นต่อ” ได้เลย ระบบตรวจยอดต่อเบื้องหลัง พอเงินเข้าจะเด้งหน้าต่างแจ้งเอง')];
      case 'manual':
        if (!o.qr_svg) {
          return [h('p', null, 'ยังไม่ได้ตั้งเลขพร้อมเพย์ จึงยังสร้าง QR ไม่ได้'),
            h('a', { class: 'btn primary block', href: '/settings#pay' }, 'ไปหน้าตั้งค่า'),
            h('p', { class: 'muted' }, 'หรือรับเงินแล้วกดยืนยันด้านล่าง')];
        }
        return [qrCard(o.qr_svg),
          o.pay_note ? h('p', { class: 'warnbox' }, 'ตรวจยอดอัตโนมัติใช้ไม่ได้ครั้งนี้: ' + o.pay_note) : null,
          h('p', { class: 'muted' }, 'ให้ลูกค้าสแกนจ่าย แล้วตรวจยอดในแอปธนาคาร จากนั้นกดยืนยัน (ตั้งค่าระบบตรวจยอดอัตโนมัติได้ที่หน้า ตั้งค่า)'),
          state.verifyActive ? h('button', { class: 'btn primary block', type: 'button', onclick: () => act(o.id, 'repay') }, 'สร้าง QR ใหม่ (ตรวจยอดอัตโนมัติ)') : null,
          state.verifyActive ? h('p', { class: 'muted', style: 'font-size:14px;margin:6px 0 0' }, 'ถ้าลูกค้าสแกน QR เดิมจ่ายไปแล้ว ให้กด “ยืนยันรับเงินแล้ว” แทน เพื่อไม่ให้จ่ายซ้ำ') : null];
      case 'queued':
        return [h('p', null, 'มีรายการยอดเท่ากันกำลังรอจ่ายอยู่ก่อน ระบบจะแสดง QR เมื่อถึงคิว'),
          h('p', { class: 'muted' }, 'ยอดเท่ากันและบัญชีเดียวกันตรวจพร้อมกันไม่ได้ จึงต้องต่อคิว (รายการละไม่เกิน 2 นาที) ถ้าไม่อยากรอ ให้เลือกยอดที่ต่างกัน หรือกดยืนยันเองเมื่อรับเงินแล้ว')];
      case 'expired':
      case 'cancelled':
        return expiredView(o);
      case 'ambiguous':
        return [h('p', null, 'ระบบยืนยันยอดเองไม่ได้'),
          o.pay_note ? h('p', { class: 'warnbox' }, o.pay_note) : null,
          h('p', { class: 'muted' }, 'ตรวจในแอปธนาคาร แล้วกดยืนยันถ้าเงินเข้าแล้ว')];
      default:
        return [h('p', { class: 'muted' }, 'กำลังสร้าง QR…')];
    }
  }

  function renderPay() {
    if (state.payId == null) return;
    const o = state.orders.find((x) => x.id === state.payId);
    if (!o || o.status !== 'pending_payment') { closePay(); return; }
    if (o.qr_expires_in > 5) state.expired.delete(o.id);
    $('#paybox').replaceChildren(
      h('h3', null, 'รับชำระผ่าน QR พร้อมเพย์'),
      h('div', { class: 'amt num' }, baht(o.total_satang)),
      ...payBody(o).filter(Boolean),
      h('div', { class: 'btns' },
        h('button', { class: 'btn ok block', type: 'button', onclick: () => act(o.id, 'confirm', 'ยืนยันว่าได้รับเงิน ' + baht(o.total_satang) + ' แล้ว?', { title: 'ยืนยันรับเงิน', okText: 'ได้รับเงินแล้ว', cancelText: 'ยังไม่ได้รับ', tone: 'primary' }) }, 'ยืนยันรับเงินแล้ว'),
        h('button', { class: 'btn block', type: 'button', onclick: closePay }, o.pay_state === 'pending' ? 'ปิด แล้วขายคิวอื่นต่อ' : 'ปิดหน้านี้'),
        h('button', { class: 'btn danger block', type: 'button', onclick: () => act(o.id, 'cancel', 'ยกเลิกออเดอร์นี้?', CANCEL_ASK) }, 'ยกเลิกออเดอร์')));
  }

  // ---- payment success page (PromptPay): shown when the money is verified or confirmed by hand
  const SUCCESS_MS = 5000;
  const CASH_SUCCESS_MS = 8000;
  let successTimer;

  function closeSuccess() {
    clearTimeout(successTimer);
    document.querySelectorAll('.success-modal').forEach((el) => el.remove());
    document.removeEventListener('keydown', onSuccessKey, true);
  }
  function onSuccessKey(e) { if (e.key === 'Escape') closeSuccess(); }

  async function showSuccess(d) {
    const cash = d.method === 'cash';
    // A cash sale without an amount typed (older orders) has nothing extra to show.
    if (!cash && d.method !== 'promptpay') return;
    if (cash && d.cash_received_satang == null) return;
    await loadOrders();
    const order = state.orders.find((o) => o.id === d.order_id);
    const meta = (cash ? [
      ['ช่องทาง', 'เงินสด'],
      ['รับเงินมา', baht(d.cash_received_satang)],
    ] : [
      ['ช่องทาง', 'พร้อมเพย์'],
      d.bank ? ['ธนาคาร', String(d.bank).toUpperCase()] : null,
      d.payer_name ? ['ผู้โอน', d.payer_name] : null,
      ['การตรวจสอบ', d.verified ? 'ตรวจยอดอัตโนมัติแล้ว' : 'พนักงานกดยืนยัน'],
    ]).filter(Boolean);
    const done = h('button', { class: 'btn ok block', type: 'button', onclick: closeSuccess }, 'เสร็จสิ้น');
    const extra = state.printReady
      ? [h('button', { class: 'btn block', type: 'button', onclick: () => printOrder(d.order_id, 'ticket') }, 'พิมพ์บิลซ้ำ')]
      : [];
    closeSuccess();
    const overlay = h('div', { class: 'modal open success-modal' },
      h('div', { class: 'sheet okbox', role: 'alertdialog', 'aria-modal': 'true', 'aria-label': 'ชำระเงินสำเร็จ' },
        h('div', { class: 'okmark', 'aria-hidden': 'true' }, '✓'),
        h('h2', { style: 'font-size:22px;margin-bottom:4px' }, cash ? 'รับเงินสดแล้ว' : 'ชำระเงินสำเร็จ'),
        cash ? h('div', { class: 'okchange' }, h('span', null, 'เงินทอน'), h('b', { class: 'num disp' }, baht(d.change_satang || 0))) : null,
        h('div', { class: cash ? 'okpaid num' : 'amt num' }, (cash ? 'ยอดสุทธิ ' : '') + baht(d.total_satang)),
        h('div', { class: 'okq' }, h('span', { class: 'muted' }, 'คิว'), h('b', { class: 'num' }, d.queue_label), h('span', { class: 'okno num' }, 'ออเดอร์ #' + d.order_id)),
        order ? h('div', { class: 'okitems' }, ...order.items.map((i) => h('div', { class: 'line' },
          h('span', null, i.name + ' ×' + i.qty), h('span', { class: 'num' }, baht(i.unit_price_satang * i.qty))))) : null,
        h('dl', { class: 'okmeta' }, ...meta.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])),
        h('div', { class: 'btns' }, done, ...extra)));
    overlay.addEventListener('click', (e) => { if (e.target === overlay) closeSuccess(); });
    document.addEventListener('keydown', onSuccessKey, true);
    document.body.append(overlay);
    done.focus();
    successTimer = setTimeout(closeSuccess, cash ? CASH_SUCCESS_MS : SUCCESS_MS);
  }

  // ---- cash: keypad for the amount handed over, live change
  const NOTES = [20, 50, 100, 500, 1000];
  const SUGGESTIONS = 4;
  const MAX_DIGITS = 6;
  let cashBox = null;

  function closeCash() {
    if (!cashBox) return;
    document.removeEventListener('keydown', onCashKey, true);
    cashBox.remove();
    cashBox = null;
  }

  // Likely amounts a customer hands over: round-ups of the total plus the notes that cover it.
  function cashSuggestions(totalBaht) {
    const up = (step) => Math.ceil(totalBaht / step) * step;
    const picks = new Set([up(10), up(20), up(50), up(100), ...NOTES.filter((n) => n >= totalBaht)]);
    picks.delete(totalBaht);
    return [...picks].sort((a, b) => a - b).slice(0, SUGGESTIONS);
  }

  function onCashKey(e) {
    if (!cashBox || e.ctrlKey || e.metaKey || e.altKey) return;
    if (/^\d$/.test(e.key)) { e.preventDefault(); cashBox.press(e.key); }
    else if (e.key === 'Backspace') { e.preventDefault(); cashBox.press('back'); }
    else if (e.key === 'Enter') { e.preventDefault(); cashBox.submit(); }
    else if (e.key === 'Escape') { e.preventDefault(); closeCash(); }
  }

  function openCash() {
    if (!state.cart.size) { toast('ตะกร้าว่าง เลือกสินค้าก่อน'); return; }
    closeCash();
    const total = cartTotal();
    const totalBaht = total / 100;
    let digits = '';
    const receivedSatang = () => Number(digits || 0) * 100;

    const shown = h('b', { class: 'num disp cash-in' });
    const change = h('div', { class: 'cash-change' });
    const okBtn = h('button', { class: 'btn ok lg', type: 'button' }, 'รับเงินและออกคิว');
    const quick = h('div', { class: 'cash-quick' });

    function paint() {
      const got = receivedSatang();
      shown.textContent = digits ? baht(got) : '฿0';
      shown.classList.toggle('empty', !digits);
      const enough = got >= total;
      change.className = 'cash-change ' + (!digits ? 'idle' : enough ? 'ok' : 'short');
      change.replaceChildren(...(!digits
        ? [h('span', null, 'กดตัวเลขจำนวนเงินที่ลูกค้าจ่าย หรือเลือกปุ่มลัด')]
        : enough
          ? [h('span', null, 'เงินทอน'), h('b', { class: 'num disp' }, baht(got - total))]
          : [h('span', null, 'ยังขาดอีก'), h('b', { class: 'num disp' }, baht(total - got))]));
      okBtn.disabled = !enough;
    }

    function set(baht_) { digits = String(Math.round(baht_)); paint(); }
    function press(key) {
      if (key === 'back') digits = digits.slice(0, -1);
      else if (key === 'clear') digits = '';
      else if (digits.length + key.length <= MAX_DIGITS && !(digits === '' && /^0+$/.test(key))) digits += key;
      paint();
    }

    async function submit() {
      if (okBtn.disabled) return;
      okBtn.disabled = true;
      const got = receivedSatang();
      try {
        closeCash();
        await pay('cash', got);
      } catch (err) { toast(err.message); }
    }

    const exact = h('button', { class: 'chip', type: 'button', onclick: () => { digits = String(Math.round(totalBaht)); if (total % 100) { toast('ยอดมีเศษสตางค์ ใส่จำนวนเต็มที่สูงกว่ายอด'); digits = String(Math.ceil(totalBaht)); } paint(); } }, 'พอดี');
    quick.append(exact, ...cashSuggestions(Math.ceil(totalBaht)).map((v) => h('button', { class: 'chip', type: 'button', onclick: () => set(v) }, '฿' + v.toLocaleString('th-TH'))));

    const keys = ['7', '8', '9', '4', '5', '6', '1', '2', '3', '00', '0', 'back'].map((k) =>
      h('button', { class: 'ck' + (k === 'back' ? ' fn' : ''), type: 'button', 'aria-label': k === 'back' ? 'ลบ' : k, onclick: () => press(k) }, k === 'back' ? '⌫' : k));

    okBtn.addEventListener('click', submit);
    cashBox = h('div', { class: 'modal open cash-modal', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'รับเงินสด' },
      h('div', { class: 'sheet cash' },
        h('div', { class: 'cash-head' }, h('h3', null, 'รับเงินสด'), h('button', { class: 'btn ghost small', type: 'button', onclick: closeCash }, 'ปิด')),
        h('div', { class: 'cash-total' }, h('span', { class: 'muted' }, 'ยอดที่ต้องชำระ'), h('b', { class: 'num disp' }, baht(total))),
        h('div', { class: 'cash-recv' }, h('span', { class: 'muted' }, 'ลูกค้าจ่ายมา'), shown,
          h('button', { class: 'btn ghost small', type: 'button', onclick: () => press('clear') }, 'ล้าง')),
        change,
        quick,
        h('div', { class: 'calc-keys cash-keys' }, ...keys),
        okBtn));
    cashBox.press = press;
    cashBox.submit = submit;
    cashBox.addEventListener('click', (e) => { if (e.target === cashBox) closeCash(); });
    document.addEventListener('keydown', onCashKey, true);
    document.body.append(cashBox);
    paint();
  }

  function paintDisplays(count) {
    const el = $('#dstat');
    el.textContent = count > 0 ? '● ' + count : '○';
    el.className = 'dstat ' + (count > 0 ? 'on' : 'off');
    el.title = count > 0 ? 'จอลูกค้าเชื่อมต่ออยู่ ' + count + ' เครื่อง' : 'ยังไม่มีจอลูกค้าเชื่อมต่อ';
  }

  async function refreshDisplays() {
    try { paintDisplays((await api('GET', '/api/displays')).count); } catch (_) { /* indicator only */ }
  }

  function openScreens() {
    const saved = (() => { try { return localStorage.getItem('pm:origin'); } catch (_) { return null; } })();
    const input = h('input', { value: saved || location.origin, 'aria-label': 'ที่อยู่เซิร์ฟเวอร์', style: 'width:100%;height:44px;border:1px solid var(--ln);border-radius:10px;padding:0 10px;font:inherit' });
    const out = h('div');
    const draw = () => {
      const origin = input.value.trim().replace(/\/$/, '');
      try { localStorage.setItem('pm:origin', origin); } catch (_) { /* ignore */ }
      const slug = window.SLUG;
      const make = (title, path) => h('div', { style: 'margin-top:14px' },
        h('b', null, title),
        h('div', { class: 'qr' }, h('img', { src: '/api/qr?text=' + encodeURIComponent(origin + path), alt: 'QR ' + title, style: 'width:100%' })),
        h('a', { href: origin + path, target: '_blank', rel: 'noopener', class: 'muted', style: 'word-break:break-all' }, origin + path));
      out.replaceChildren(make('จอลูกค้า (สแกนด้วยมือถือ/iPad)', '/display/' + slug), make('สั่งเองจากมือถือลูกค้า', '/m/' + slug));
    };
    input.addEventListener('change', draw);
    $('#scrbox').replaceChildren(
      h('h3', null, 'จอลูกค้า และ QR สั่งเอง'),
      h('p', { class: 'muted', style: 'margin:6px 0' }, 'สแกน QR “จอลูกค้า” ด้วยมือถือหรือ iPad เครื่องที่ 2 หน้านั้นจะแสดงตะกร้า และ QR ชำระเงินให้ลูกค้าสแกนเมื่อกด “QR พร้อมเพย์”'),
      h('p', { class: 'muted', style: 'margin:6px 0' }, 'ถ้าเปิดจากเครื่องอื่น ให้ใส่ที่อยู่ IP ของเครื่องนี้ เช่น http://192.168.1.20:8080'),
      input, out,
      h('div', { class: 'btns' }, h('button', { class: 'btn block', type: 'button', onclick: () => $('#scrmodal').classList.remove('open') }, 'ปิด')));
    draw();
    $('#scrmodal').classList.add('open');
  }

  async function loadProducts() {
    try { state.products = await api('GET', '/api/products'); } catch (err) { toast(err.message); return; }
    for (const id of [...state.cart.keys()]) {
      const p = productById(id);
      if (!p) state.cart.delete(id);
      else if (p.sold_out) { state.cart.delete(id); toast('“' + p.name + '” หมดแล้ว เอาออกจากตะกร้าให้'); }
    }
    if (state.cat !== ALL && !state.products.some((p) => p.parent_id == null && p.category === state.cat)) state.cat = ALL;
    // A group that was deleted (or emptied elsewhere) must not leave us standing inside it.
    if (state.path.some((id) => !productById(id))) state.path = [];
    renderChips(); renderGrid(); renderCart();
  }

  // ---- payment-check warning: a red bar (and three beeps) when the bank check cannot work
  const PAY_CHECK_MS = 30000;
  const ALERT_AGAIN_MS = 300000;
  let payBad = false;
  let lastAlert = 0;

  async function checkPay() {
    let s;
    try { s = await api('GET', '/api/pay/health'); } catch (_) { return; }
    const bad = s.mode === 'auto' && !s.ok;
    const box = $('#paybanner');
    box.hidden = !bad;
    if (bad) {
      box.replaceChildren(h('b', null, 'ตรวจยอดอัตโนมัติใช้ไม่ได้'),
        h('span', null, ' ' + s.message + ' · ลูกค้าที่โอนแล้วต้องกด “ยืนยันรับเงินแล้ว” เอง และลูกค้าที่สแกนสั่งเองจะสั่งไม่ได้จนกว่าจะแก้'));
      if (!payBad || Date.now() - lastAlert > ALERT_AGAIN_MS) { PM.alertBeep(); lastAlert = Date.now(); }
    } else if (payBad) toast('ตรวจยอดอัตโนมัติกลับมาใช้ได้แล้ว');
    payBad = bad;
  }

  // ---- sold out: one switch per menu item, for the cashier in the middle of a rush
  function openSoldOut() {
    const items = state.products.filter((p) => !p.is_group);
    const rows = () => items.map((p) => {
      const box = h('input', { type: 'checkbox', class: 'pcheck', checked: !!p.sold_out, 'aria-label': p.name + ' หมด' });
      box.addEventListener('change', async () => {
        try {
          await api('PUT', '/api/products/' + p.id + '/sold-out', { sold_out: box.checked });
          p.sold_out = box.checked;
          if (box.checked) state.cart.delete(p.id);
          renderGrid(); renderCart();
        } catch (err) { box.checked = !box.checked; toast(err.message); }
      });
      return h('label', { class: 'prow selectable' }, box, h('div', { class: 'pinfo' }, h('div', { class: 'pname' }, p.name)), h('span', { class: 'muted' }, 'หมด'));
    });
    const overlay = h('div', { class: 'modal open' },
      h('div', { class: 'sheet left', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'เมนูที่หมด' },
        h('h3', { style: 'margin-bottom:4px' }, 'เมนูที่หมดวันนี้'),
        h('p', { class: 'muted', style: 'margin:0 0 8px' }, 'ติ๊กช่อง “หมด” แล้วเมนูนั้นสั่งไม่ได้ทั้งหน้าขายและหน้าลูกค้า เอาติ๊กออกเมื่อมีของ'),
        h('div', { style: 'max-height:55vh;overflow:auto' }, ...(items.length ? rows() : [h('p', { class: 'muted' }, 'ยังไม่มีสินค้า')])),
        h('div', { class: 'btns' }, h('button', { class: 'btn primary block', type: 'button', onclick: () => overlay.remove() }, 'เสร็จ'))));
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
    document.body.append(overlay);
  }

  async function init() {
    $('#today').textContent = new Date().toLocaleDateString('th-TH', { timeZone: 'Asia/Bangkok', weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
    await loadProducts();
    await loadOrders();
    pushCart();
    try {
      const ps = await api('GET', '/api/print/settings');
      state.printReady = ps.ready;
      state.printMode = ps.mode;
      paintStation();
      renderOrders();
    } catch (_) { /* printing is optional */ }
    loadPrintq();
    refreshDisplays();
  }

  document.querySelectorAll('.tab').forEach((b) => b.addEventListener('click', () => setTab(b.dataset.tab)));
  $('#toCart').addEventListener('click', () => setTab('cart'));
  $('#clear').addEventListener('click', clearCart);
  $('#payCash').addEventListener('click', openCash);
  $('#payQr').addEventListener('click', () => pay('promptpay'));
  $('#screens').addEventListener('click', openScreens);
  $('#soldout').addEventListener('click', openSoldOut);
  ['paymodal', 'scrmodal'].forEach((id) => $('#' + id).addEventListener('click', (e) => {
    if (e.target.id === id) { if (id === 'paymodal') closePay(); else e.target.classList.remove('open'); }
  }));
  Voice.bindButton($('#snd'));
  keepAwake();

  listen('/events/pos', {
    orders: loadOrders,
    pay: loadOrders,
    refresh: loadOrders,
    products: () => { loadProducts().then(pushCart); },
    paid: (d) => {
      toast('รับเงินแล้ว ' + baht(d.total_satang) + ' · คิว ' + d.queue_label);
      Voice.speak(d.say);
    },
    paid_detail: showSuccess,
    displays: (d) => paintDisplays(d.count),
    called: (d) => {
      if (Voice.enabled) Voice.speak(d.say);
      else toast('เรียกคิว ' + d.queue_label + ' แล้ว แต่เสียงปิดอยู่ กดปุ่ม “เปิดเสียง” ด้านบนเพื่อให้เครื่องนี้พูด');
    },
    print_error: (d) => toast('พิมพ์ไม่สำเร็จ: ' + d.message),
    print_state: (d) => { state.printq = d; renderPrintq(); if (window.Station) Station.wake(); },
  }, () => { loadOrders(); loadProducts(); refreshDisplays(); loadPrintq(); });

  init();
  checkPay();
  setInterval(() => { if (document.visibilityState === 'visible') checkPay(); }, PAY_CHECK_MS);
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') checkPay(); });
})();
