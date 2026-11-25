(function () {
  const { $, h, baht, api, toast, listen } = PM;
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const TZ = 'Asia/Bangkok';
  const PERIODS = [['day', 'รายวัน'], ['month', 'รายเดือน'], ['year', 'รายปี']];
  const PREVIOUS = { day: 'วันก่อนหน้า', month: 'เดือนก่อนหน้า', year: 'ปีก่อนหน้า' };
  const TREND_TITLE = { day: 'ยอดขายรายชั่วโมง', month: 'ยอดขายรายวัน', year: 'ยอดขายรายเดือน' };
  const SOURCES = { pos: 'หน้าร้าน (แคชเชียร์)', self: 'ลูกค้าสั่งเอง (สแกน QR)' };
  const BANKS = { kbank: 'กสิกรไทย (KBank)', scb: 'ไทยพาณิชย์ (SCB)', ktb: 'กรุงไทย (KTB)', manual: 'พนักงานกดยืนยันเอง' };
  const REFRESH_DEBOUNCE_MS = 800;

  const today = () => new Intl.DateTimeFormat('en-CA', { timeZone: TZ }).format(new Date());
  const state = { period: 'day', date: today(), data: null, error: null, seq: 0, closes: [] };
  let refreshTimer;

  // ---- dates: `state.date` is always a full YYYY-MM-DD; the API gets the part the period needs
  const keyOf = (period, ymd) => ({ day: ymd, month: ymd.slice(0, 7), year: ymd.slice(0, 4) }[period]);
  const parts = (ymd) => ymd.split('-').map(Number);
  const iso = (d) => d.toISOString().slice(0, 10);

  function shift(period, ymd, delta) {
    const [y, m, d] = parts(ymd);
    if (period === 'day') return iso(new Date(Date.UTC(y, m - 1, d + delta)));
    if (period === 'month') return iso(new Date(Date.UTC(y, m - 1 + delta, 1)));
    return iso(new Date(Date.UTC(y + delta, 0, 1)));
  }

  function label(period, ymd) {
    const [y, m, d] = parts(ymd);
    const at = new Date(Date.UTC(y, m - 1, d));
    if (period === 'day') return at.toLocaleDateString('th-TH', { timeZone: 'UTC', weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
    if (period === 'month') return at.toLocaleDateString('th-TH', { timeZone: 'UTC', month: 'long', year: 'numeric' });
    return 'พ.ศ. ' + (y + 543);
  }

  const isCurrent = () => keyOf(state.period, state.date) === keyOf(state.period, today());
  const canGoForward = () => keyOf(state.period, shift(state.period, state.date, 1)) <= keyOf(state.period, today());

  // ---- formatting
  const pct = (value) => (Math.abs(value) >= 100 ? value.toFixed(0) : value.toFixed(1)) + '%';
  const share = (part, whole) => (whole > 0 ? (part * 100) / whole : 0);

  function compact(satang) {
    const value = satang / 100;
    if (value >= 1000) return '฿' + (value / 1000).toLocaleString('th-TH', { maximumFractionDigits: 1 }) + 'k';
    return '฿' + value.toLocaleString('th-TH', { maximumFractionDigits: 0 });
  }

  function niceCeil(value) {
    if (value <= 0) return 1;
    const magnitude = 10 ** Math.floor(Math.log10(value));
    const f = value / magnitude;
    const step = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10;
    return step * magnitude;
  }

  function svg(tag, attrs, ...kids) {
    const el = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs || {})) if (v != null) el.setAttribute(k, v);
    for (const kid of kids) if (kid) el.append(kid);
    return el;
  }

  // ---- data
  async function load() {
    const seq = ++state.seq;
    try {
      const data = await api('GET', '/api/dashboard?period=' + state.period + '&date=' + keyOf(state.period, state.date));
      if (seq !== state.seq) return;
      state.data = data;
      state.error = null;
      state.closes = await api('GET', '/api/day/closes').catch(() => state.closes);
    } catch (err) {
      if (seq !== state.seq) return;
      state.error = err.message;
      toast(err.message);
    }
    render();
  }

  function go(change) {
    Object.assign(state, change);
    state.data = null;
    render();
    load();
  }

  // ---- controls
  function renderControls() {
    const chips = h('div', { class: 'chips', role: 'tablist' }, ...PERIODS.map(([key, text]) =>
      h('button', { class: 'chip' + (key === state.period ? ' on' : ''), type: 'button', role: 'tab', 'aria-selected': key === state.period, onclick: () => go({ period: key }) }, text)));

    const picker = h('input', {
      class: 'datepick', 'aria-label': 'เลือกช่วงเวลา',
      type: state.period === 'day' ? 'date' : state.period === 'month' ? 'month' : 'number',
      value: keyOf(state.period, state.date), max: keyOf(state.period, today()), min: state.period === 'year' ? 2000 : null,
    });
    picker.addEventListener('change', () => {
      const raw = picker.value.trim();
      const ymd = state.period === 'day' ? raw : state.period === 'month' ? raw + '-01' : raw + '-01-01';
      if (/^\d{4}-\d{2}-\d{2}$/.test(ymd) && ymd.slice(0, 4) >= '2000') go({ date: ymd });
    });

    const nav = h('div', { class: 'dnav' },
      h('button', { class: 'btn small', type: 'button', 'aria-label': 'ก่อนหน้า', onclick: () => go({ date: shift(state.period, state.date, -1) }) }, '‹'),
      h('div', { class: 'dlabel' }, h('b', null, label(state.period, state.date)), picker),
      h('button', { class: 'btn small', type: 'button', 'aria-label': 'ถัดไป', disabled: !canGoForward(), onclick: () => go({ date: shift(state.period, state.date, 1) }) }, '›'),
      h('button', { class: 'btn small', type: 'button', disabled: isCurrent(), onclick: () => go({ date: today() }) }, 'ปัจจุบัน'),
      h('a', { class: 'btn small', href: '/api/dashboard/export?period=' + state.period + '&date=' + keyOf(state.period, state.date), download: '' }, 'ดาวน์โหลด CSV'),
      state.period === 'day' ? h('button', { class: 'btn small primary', type: 'button', onclick: openClose }, 'ปิดยอดวัน') : null);

    $('#controls').replaceChildren(chips, nav);
  }

  // ---- cards
  function delta(change, baseline, sameElapsed) {
    if (change == null) return h('span', { class: 'muted' }, baseline > 0 ? '' : 'ยังไม่มีข้อมูลให้เทียบ');
    const up = change >= 0;
    const basis = PREVIOUS[state.period] + (sameElapsed ? ' (ช่วงเวลาเดียวกัน)' : '');
    return h('span', { class: 'delta ' + (up ? 'up' : 'down') }, (up ? '▲ ' : '▼ ') + pct(Math.abs(change)), h('span', { class: 'muted' }, ' จาก' + basis));
  }

  function kpis(d) {
    const t = d.totals;
    return h('div', { class: 'kpis' },
      h('div', { class: 'card kpi hero-kpi' },
        h('div', { class: 'klabel' }, 'ยอดขายรวม'),
        h('div', { class: 'kval num' }, baht(t.revenue)),
        delta(d.previous.revenue_change_pct, d.previous.revenue, d.previous.same_period),
        d.previous.revenue > 0 ? h('div', { class: 'muted ksub' }, PREVIOUS[state.period] + (d.previous.same_period ? ' (ช่วงเวลาเดียวกัน) ' : ' ') + baht(d.previous.revenue)) : null),
      h('div', { class: 'card kpi' },
        h('div', { class: 'klabel' }, 'จำนวนออเดอร์'),
        h('div', { class: 'kval num' }, t.orders.toLocaleString('th-TH')),
        delta(d.previous.orders_change_pct, d.previous.orders, d.previous.same_period)),
      h('div', { class: 'card kpi' },
        h('div', { class: 'klabel' }, 'เฉลี่ยต่อออเดอร์'),
        h('div', { class: 'kval num' }, baht(t.average))),
      h('div', { class: 'card kpi' },
        h('div', { class: 'klabel' }, 'จำนวนชิ้นที่ขาย'),
        h('div', { class: 'kval num' }, t.items.toLocaleString('th-TH'))));
  }

  function methodSplit(d) {
    const find = (m) => d.methods.find((r) => r.method === m) || { orders: 0, revenue: 0 };
    const qr = find('promptpay');
    const cash = find('cash');
    const total = qr.revenue + cash.revenue;
    const row = (name, cls, r) => h('div', { class: 'mrow' },
      h('span', { class: 'dot ' + cls, 'aria-hidden': 'true' }),
      h('div', { class: 'mname' }, h('b', null, name), h('span', { class: 'muted' }, r.orders.toLocaleString('th-TH') + ' ออเดอร์ · ' + pct(share(r.revenue, total)))),
      h('b', { class: 'num' }, baht(r.revenue)));
    return h('section', { class: 'card' },
      h('h2', null, 'ช่องทางชำระเงิน'),
      h('div', { class: 'splitbar', role: 'img', 'aria-label': 'QR ' + pct(share(qr.revenue, total)) + ' เงินสด ' + pct(share(cash.revenue, total)) },
        total > 0 ? [h('span', { class: 'seg qr', style: 'flex:' + qr.revenue }), h('span', { class: 'seg cash', style: 'flex:' + cash.revenue })] : h('span', { class: 'seg none' })),
      row('QR พร้อมเพย์', 'qr', qr),
      row('เงินสด', 'cash', cash));
  }

  function trendChart(d) {
    const trend = d.trend;
    const W = 720; const H = 250; const L = 46; const R = 6; const T = 10; const B = 26;
    const peakRevenue = Math.max(0, ...trend.map((b) => b.qr_revenue + b.cash_revenue));
    if (peakRevenue === 0) return h('p', { class: 'muted empty' }, 'ยังไม่มียอดขายในช่วงนี้');
    const max = niceCeil(peakRevenue / 100) * 100;
    const n = trend.length;
    const slot = (W - L - R) / n;
    const bw = Math.max(3, Math.min(30, slot * 0.68));
    const y = (v) => T + (H - T - B) * (1 - v / max);
    const everyLabel = n > 24 ? 3 : n > 12 ? 2 : 1;

    const grid = [0, 0.25, 0.5, 0.75, 1].flatMap((f) => [
      svg('line', { x1: L, x2: W - R, y1: y(max * f), y2: y(max * f), class: f === 0 ? 'axis' : 'grid' }),
      svg('text', { x: L - 6, y: y(max * f) + 4, class: 'ytick', 'text-anchor': 'end' }, document.createTextNode(compact(max * f))),
    ]);
    const bars = trend.map((b, i) => {
      const x = L + slot * i + (slot - bw) / 2;
      const qr = b.qr_revenue; const cash = b.cash_revenue;
      const tip = b.label + ' · รวม ' + baht(qr + cash) + ' (QR ' + baht(qr) + ' · เงินสด ' + baht(cash) + ') · ' + (b.qr_orders + b.cash_orders) + ' ออเดอร์';
      return svg('g', { class: 'bar' },
        svg('title', null, document.createTextNode(tip)),
        svg('rect', { x: L + slot * i, y: T, width: slot, height: H - T - B, class: 'hit' }),
        qr > 0 ? svg('rect', { x, y: y(qr), width: bw, height: y(0) - y(qr), class: 'qr', rx: 1.5 }) : null,
        cash > 0 ? svg('rect', { x, y: y(qr + cash), width: bw, height: y(qr) - y(qr + cash), class: 'cash', rx: 1.5 }) : null,
        i % everyLabel === 0 ? svg('text', { x: x + bw / 2, y: H - 8, class: 'xtick', 'text-anchor': 'middle' }, document.createTextNode(b.label)) : null);
    });
    return svg('svg', { viewBox: '0 0 ' + W + ' ' + H, class: 'chart', role: 'img', 'aria-label': TREND_TITLE[d.period] + ' แยก QR และเงินสด' }, ...grid, ...bars);
  }

  function trendCard(d) {
    const rows = d.trend.filter((b) => b.qr_orders + b.cash_orders > 0);
    return h('section', { class: 'card' },
      h('div', { class: 'cardhead' }, h('h2', null, TREND_TITLE[d.period]),
        h('div', { class: 'legend' }, h('span', null, h('i', { class: 'dot qr' }), 'QR พร้อมเพย์'), h('span', null, h('i', { class: 'dot cash' }), 'เงินสด'))),
      trendChart(d),
      d.peak ? h('p', { class: 'muted peak' }, 'ช่วงที่ขายดีที่สุด: ' + d.peak.label + ' (' + baht(d.peak.revenue) + ')') : null,
      rows.length ? h('details', { class: 'tablewrap' }, h('summary', null, 'ดูเป็นตาราง'),
        h('table', { class: 'dtable' },
          h('thead', null, h('tr', null, ...['ช่วง', 'QR', 'เงินสด', 'รวม'].map((c, i) => h('th', { class: i ? 'r' : '' }, c)))),
          h('tbody', null, ...rows.map((b) => h('tr', null,
            h('td', null, b.label),
            h('td', { class: 'r num' }, baht(b.qr_revenue) + ' (' + b.qr_orders + ')'),
            h('td', { class: 'r num' }, baht(b.cash_revenue) + ' (' + b.cash_orders + ')'),
            h('td', { class: 'r num' }, baht(b.qr_revenue + b.cash_revenue))))))) : null);
  }

  function topItems(d) {
    const max = Math.max(1, ...d.top_items.map((i) => i.qty));
    return h('section', { class: 'card' },
      h('h2', null, 'สินค้าขายดี'),
      d.top_items.length ? d.top_items.map((item, i) => h('div', { class: 'irow' },
        h('span', { class: 'rank' }, i + 1),
        h('div', { class: 'iname' },
          h('div', { class: 'itop' }, h('span', null, item.name), h('b', { class: 'num' }, item.qty.toLocaleString('th-TH') + ' ชิ้น')),
          h('div', { class: 'ibar' }, h('span', { style: 'width:' + (item.qty * 100) / max + '%' })),
          h('div', { class: 'muted isub num' }, baht(item.revenue))))) : h('p', { class: 'muted empty' }, 'ยังไม่มียอดขายในช่วงนี้'));
  }

  function breakdown(title, rows, names, keyName, note) {
    const total = rows.reduce((s, r) => s + r.revenue, 0);
    return h('section', { class: 'card' },
      h('h2', null, title),
      note ? h('p', { class: 'muted note' }, note) : null,
      rows.length ? rows.map((r) => h('div', { class: 'brow' },
        h('div', null, h('b', null, names[r[keyName]] || String(r[keyName]).toUpperCase()), h('div', { class: 'muted' }, r.orders.toLocaleString('th-TH') + ' ออเดอร์ · ' + pct(share(r.revenue, total)))),
        h('b', { class: 'num' }, baht(r.revenue)))) : h('p', { class: 'muted empty' }, 'ไม่มีข้อมูล'));
  }

  function cancelled(d) {
    const a = d.cancelled.after_payment; const u = d.cancelled.unpaid;
    return h('section', { class: 'card' },
      h('h2', null, 'ออเดอร์ที่ยกเลิก'),
      h('div', { class: 'brow' }, h('div', null, h('b', null, 'ยกเลิกหลังชำระเงินแล้ว'), h('div', { class: 'muted' }, 'ไม่นับในยอดขาย ตรวจว่าคืนเงินแล้วหรือยัง')),
        h('b', { class: 'num' + (a.orders ? ' warn' : '') }, a.orders + ' ออเดอร์ · ' + baht(a.revenue))),
      h('div', { class: 'brow' }, h('div', null, h('b', null, 'ยกเลิกก่อนชำระเงิน'), h('div', { class: 'muted' }, 'ยังไม่ได้รับเงิน')),
        h('b', { class: 'num' }, u.orders + ' ออเดอร์')),
      ...(d.cancelled.reasons || []).map((r) => h('div', { class: 'brow' }, h('div', null, h('b', null, 'เหตุผล: ' + r.reason)),
        h('b', { class: 'num' }, r.orders + ' ออเดอร์ · ' + baht(r.revenue)))),
      d.totals.discounts ? h('div', { class: 'brow' }, h('div', null, h('b', null, 'ส่วนลดที่ให้ลูกค้า'), h('div', { class: 'muted' }, 'หักออกจากยอดขายแล้ว')),
        h('b', { class: 'num' }, baht(d.totals.discounts))) : null);
  }

  // ---- slips: every PromptPay payment of the period, kept for good (who paid, bank, reference)
  function slipTime(at) {
    const [date, time] = at.split(' ');
    if (state.period === 'day') return time;
    const [, m, day] = date.split('-');
    return day + '/' + m + ' ' + time.slice(0, 5);
  }

  function slipRow(s) {
    const items = (s.items || []).map((i) => i.name + ' ×' + i.qty).join(', ');
    return h('li', { class: 'slip' },
      h('div', { class: 'slip-top' },
        h('span', { class: 'slip-time num' }, slipTime(s.paid_at)),
        h('b', { class: 'slip-q num' }, s.queue_label || '—'),
        h('span', { class: 'badge ' + (s.verified ? 'ok' : 'warn') }, s.verified ? '✓ ตรวจยอดแล้ว' : 'พนักงานกดยืนยัน'),
        h('b', { class: 'slip-amt num' }, baht(s.amount_satang))),
      h('div', { class: 'slip-meta muted' },
        [s.bank ? (BANKS[s.bank] || String(s.bank).toUpperCase()) : null, s.payer_name ? 'ผู้โอน ' + s.payer_name : null, s.reference ? 'อ้างอิง ' + s.reference : null, 'ออเดอร์ #' + s.order_id]
          .filter(Boolean).join(' · ')),
      items ? h('div', { class: 'slip-items' }, items) : null);
  }

  function slipsCard(d) {
    const list = d.slips || [];
    const total = list.reduce((sum, s) => sum + s.amount_satang, 0);
    return h('section', { class: 'card' },
      h('div', { class: 'cardhead' }, h('h2', null, 'สลิปการโอนเงิน (พร้อมเพย์)'),
        h('span', { class: 'muted' }, list.length ? list.length.toLocaleString('th-TH') + ' รายการ · ' + baht(total) : '')),
      h('p', { class: 'muted note' }, 'บันทึกทุกครั้งที่ระบบตรวจยอดเงินเข้าสำเร็จ เก็บถาวร ย้อนดูวันอื่นได้ด้วยตัวเลือกวันที่ด้านบน แม้กดล้างยอดขายแล้วก็ยังอยู่'),
      list.length ? h('ul', { class: 'slips' }, ...list.map(slipRow)) : h('p', { class: 'muted empty' }, 'ยังไม่มีสลิปในช่วงนี้'),
      list.length >= 500 ? h('p', { class: 'muted note' }, 'แสดง 500 รายการล่าสุดของช่วงนี้ เลือกช่วงที่สั้นลงเพื่อดูรายการเก่ากว่า') : null);
  }

  // ---- audit: who confirmed, cancelled or changed what (the money-risky actions first)
  const AUDIT_LABELS = {
    confirm_manual: 'กดยืนยันรับเงินเอง', cancel_paid: 'ยกเลิกหลังชำระเงิน', cancel: 'ยกเลิกก่อนชำระเงิน',
    call_skip: 'เรียกคิวข้ามลำดับ', reset_sales: 'ล้างยอดขาย', staff_added: 'เพิ่มพนักงาน',
    staff_changed: 'แก้บัญชีพนักงาน', staff_removed: 'ลบพนักงาน',
    discount: 'ให้ส่วนลด', sold_out: 'สลับเมนูหมด', close_day: 'ปิดยอดวัน', backup: 'โหลดสำรองข้อมูล',
  };
  const AUDIT_WARN = new Set(['confirm_manual', 'cancel_paid', 'reset_sales']);

  function auditCard(d) {
    const a = d.audit || { rows: [], counts: {} };
    const chips = Object.entries(a.counts).sort((x, y) => y[1] - x[1])
      .map(([k, n]) => h('span', { class: 'badge ' + (AUDIT_WARN.has(k) ? 'warn' : '') }, (AUDIT_LABELS[k] || k) + ' ' + n));
    return h('section', { class: 'card' },
      h('div', { class: 'cardhead' }, h('h2', null, 'บันทึกการทำงาน')),
      h('p', { class: 'muted note' }, 'ใครกดยืนยันรับเงินเอง ยกเลิกออเดอร์ หรือล้างยอดขาย บันทึกชื่อและเวลาไว้ถาวร แก้หรือลบไม่ได้'),
      chips.length ? h('div', { class: 'audit-chips' }, ...chips) : null,
      a.rows.length ? h('ul', { class: 'slips' }, ...a.rows.map((r) => h('li', { class: 'slip' },
        h('div', { class: 'slip-top' },
          h('span', { class: 'slip-time num' }, slipTime(r.at)),
          h('b', null, r.actor + (r.role === 'cashier' ? ' (แคชเชียร์)' : '')),
          h('span', { class: 'badge ' + (AUDIT_WARN.has(r.action) ? 'warn' : '') }, AUDIT_LABELS[r.action] || r.action),
          r.order_id ? h('span', { class: 'muted num' }, 'ออเดอร์ #' + r.order_id) : null),
        r.detail ? h('div', { class: 'slip-meta muted' }, r.detail) : null)))
        : h('p', { class: 'muted empty' }, 'ยังไม่มีรายการในช่วงนี้'));
  }

  // ---- close the day: the takings frozen into a report, with the cash counted in the drawer
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const satangOf = (text) => {
    const v = Number.parseFloat(String(text).replace(/,/g, '').trim());
    return Number.isFinite(v) && v >= 0 ? Math.round(v * 100) : null;
  };

  function reportLines(rep, counted, note) {
    const lines = [
      ['ออเดอร์ที่ขายได้', rep.orders.toLocaleString('th-TH')],
      ['ยอดขายรวม', baht(rep.revenue_satang)],
      ...(rep.discount_satang ? [['ส่วนลดที่ให้ (หักแล้ว)', baht(rep.discount_satang)]] : []),
      ['เงินสด (ต้องมีในลิ้นชัก)', baht(rep.cash_satang)],
      ['โอน QR พร้อมเพย์', baht(rep.qr_satang)],
      ['ตรวจยอดโดยบอท', rep.verified_payments + ' รายการ ' + baht(rep.verified_satang)],
      ['กดยืนยันรับเงินเอง', rep.manual_confirms + ' รายการ'],
      ['ยกเลิกหลังชำระเงิน', rep.cancelled_paid.orders + ' ออเดอร์ ' + baht(rep.cancelled_paid.revenue)],
    ];
    if (counted != null) {
      lines.push(['เงินสดที่นับได้', baht(counted)], ['ส่วนต่าง', (counted - rep.cash_satang >= 0 ? '+' : '') + baht(counted - rep.cash_satang)]);
    }
    if (note) lines.push(['หมายเหตุ', note]);
    return lines;
  }

  function printReport(rep, counted, note) {
    const w = window.open('', '_blank');
    if (!w) { toast('เบราว์เซอร์บล็อกหน้าต่างพิมพ์ ให้อนุญาตป๊อปอัปแล้วลองใหม่'); return; }
    const rows = reportLines(rep, counted, note).map(([k, v]) => '<tr><td>' + esc(k) + '</td><td style="text-align:right">' + esc(v) + '</td></tr>').join('');
    const items = (rep.top_items || []).slice(0, 10).map((i) => '<tr><td>' + esc(i.name) + '</td><td style="text-align:right">' + esc(i.qty) + ' ชิ้น</td></tr>').join('');
    w.document.write('<!doctype html><meta charset="utf-8"><title>รายงานปิดยอด ' + esc(rep.day) + '</title><body style="font:16px sans-serif;max-width:420px;margin:20px auto"><h2>' + esc(window.SHOP || '') + '</h2><h3>รายงานปิดยอด ' + esc(label('day', rep.day)) + '</h3><table style="width:100%;border-collapse:collapse" cellpadding="4">' + rows + '</table>' + (items ? '<h4>สินค้าขายดี</h4><table style="width:100%" cellpadding="3">' + items + '</table>' : '') + '<p style="margin-top:24px">ลงชื่อ ____________________</p></body>');
    w.document.close();
    w.focus();
    w.print();
  }

  async function openClose() {
    let rep;
    try { rep = await api('GET', '/api/day/preview?day=' + state.date); } catch (err) { toast(err.message); return; }
    const counted = h('input', { inputmode: 'decimal', autocomplete: 'off', placeholder: 'เช่น 1250 (เว้นว่างถ้าไม่ได้นับ)' });
    const note = h('input', { maxlength: 200, autocomplete: 'off', placeholder: 'หมายเหตุ (ไม่บังคับ)' });
    const diff = h('p', { class: 'muted', style: 'margin:6px 0 0' });
    const paintDiff = () => {
      const c = satangOf(counted.value);
      diff.textContent = c == null ? '' : 'ส่วนต่าง ' + (c - rep.cash_satang >= 0 ? '+' : '') + baht(c - rep.cash_satang) + (c === rep.cash_satang ? ' (ตรงพอดี)' : '');
    };
    counted.addEventListener('input', paintDiff);
    const close = () => overlay.remove();
    const closeBtn = h('button', { class: 'btn primary block', type: 'button' }, 'ปิดยอดวัน');
    closeBtn.addEventListener('click', async () => {
      closeBtn.disabled = true;
      try {
        const c = counted.value.trim() ? satangOf(counted.value) : null;
        if (counted.value.trim() && c == null) throw new Error('ใส่จำนวนเงินสดที่นับเป็นตัวเลข');
        await api('POST', '/api/day/close', { day: rep.day, cash_counted_satang: c, note: note.value });
        toast('ปิดยอดแล้ว เก็บรายงานไว้ให้');
        close();
        load();
      } catch (err) { toast(err.message); closeBtn.disabled = false; }
    });
    const overlay = h('div', { class: 'modal open close-modal' },
      h('div', { class: 'sheet left', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'ปิดยอดวัน' },
        h('h3', { style: 'margin-bottom:4px' }, 'ปิดยอด ' + label('day', rep.day)),
        rep.closed ? h('p', { class: 'warnbox' }, 'วันนี้ปิดยอดไปแล้วเมื่อ ' + rep.closed.closed_at + ' โดย ' + rep.closed.closed_by + ' ปิดซ้ำได้ รายงานล่าสุดจะใช้แทน') : null,
        h('div', null, ...reportLines(rep, null, '').map(([k, v]) => h('div', { class: 'brow' }, h('span', null, k), h('b', { class: 'num' }, v)))),
        h('label', { class: 'field', style: 'margin-top:12px' }, h('span', null, 'นับเงินสดในลิ้นชักได้เท่าไร (บาท)'), counted),
        diff,
        h('label', { class: 'field' }, h('span', null, 'หมายเหตุ'), note),
        h('div', { class: 'btns' }, closeBtn,
          h('button', { class: 'btn block', type: 'button', onclick: () => printReport(rep, counted.value.trim() ? satangOf(counted.value) : null, note.value) }, 'พิมพ์รายงาน'),
          h('button', { class: 'btn block', type: 'button', onclick: close }, 'ปิดหน้านี้'))));
    overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
    document.body.append(overlay);
    counted.focus();
  }

  function closesCard() {
    const list = state.closes || [];
    return h('section', { class: 'card' },
      h('div', { class: 'cardhead' }, h('h2', null, 'ประวัติการปิดยอด')),
      list.length ? h('ul', { class: 'slips' }, ...list.slice(0, 15).map((c) => h('li', { class: 'slip' },
        h('div', { class: 'slip-top' },
          h('b', null, label('day', c.day)),
          h('span', { class: 'muted' }, 'ปิดโดย ' + c.closed_by + ' · ' + c.closed_at),
          h('b', { class: 'slip-amt num' }, baht(c.revenue_satang))),
        h('div', { class: 'slip-meta muted' }, ['เงินสด ' + baht(c.cash_satang), 'โอน ' + baht(c.qr_satang), c.cash_counted_satang == null ? 'ไม่ได้นับเงินสด' : 'นับได้ ' + baht(c.cash_counted_satang) + ' ต่าง ' + (c.difference_satang >= 0 ? '+' : '') + baht(c.difference_satang), c.manual_confirms ? 'ยืนยันเอง ' + c.manual_confirms : null, c.note || null].filter(Boolean).join(' · ')))))
        : h('p', { class: 'muted empty' }, 'ยังไม่เคยปิดยอด กดปุ่ม “ปิดยอดวัน” ด้านบนเมื่อจบวัน'));
  }

  function render() {
    renderControls();
    const view = $('#view');
    if (state.error && !state.data) { view.replaceChildren(h('p', { class: 'err' }, state.error)); return; }
    if (!state.data) { view.replaceChildren(h('p', { class: 'muted empty' }, 'กำลังโหลด…')); return; }
    const d = state.data;
    view.replaceChildren(...[
      kpis(d),
      h('div', { class: 'dgrid' }, trendCard(d), methodSplit(d)),
      h('div', { class: 'dgrid even' }, topItems(d), h('div', { class: 'dstack' },
        breakdown('ที่มาของออเดอร์', d.sources, SOURCES, 'source'),
        breakdown('QR แยกตามธนาคาร', d.banks, BANKS, 'bank', 'ธนาคารที่ API ตรวจพบ ส่วนพนักงานกดยืนยันเองไม่มีข้อมูลธนาคาร'),
        cancelled(d))),
      slipsCard(d),
      auditCard(d),
      closesCard(),
    ]);
  }

  function refreshSoon() {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => { if (isCurrent()) load(); }, REFRESH_DEBOUNCE_MS);
  }

  render();
  load();
  listen('/events/pos', { orders: refreshSoon }, load);
})();
