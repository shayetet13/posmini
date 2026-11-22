(function () {
  const { $, h, baht, api, qrCard, toast, listen, Voice, keepAwake } = PM;
  const VIBRATE = [300, 150, 300, 150, 600];
  const SUCCESS_MS = 6000;
  let order = null;
  let printed = false;
  let successUntil = 0;
  let qrExpired = false;

  function when(utc) {
    const d = new Date(String(utc).replace(' ', 'T') + 'Z');
    return d.toLocaleString('th-TH', { timeZone: 'Asia/Bangkok', dateStyle: 'medium', timeStyle: 'short' });
  }

  async function refreshQr() {
    try {
      await api('POST', '/api/q/' + window.TOKEN + '/refresh');
      qrExpired = false;
      await load();
    } catch (err) { toast(err.message); }
  }

  function payHero(o) {
    const nodes = [h('div', { class: 'lbl' }, 'รอชำระเงิน'), h('div', { class: 'num disp', style: 'font-size:44px;font-weight:700;line-height:1.2' }, baht(o.total_satang))];
    const expired = qrExpired || o.pay_state === 'expired' || o.pay_state === 'cancelled';
    if (expired) {
      nodes.push(h('p', { style: 'margin:10px 0 4px;font-weight:600' }, 'QR หมดอายุแล้ว'),
        h('p', { class: 'muted', style: 'margin-bottom:12px' }, 'QR ใช้ได้ 2 นาที กดสร้างใหม่เพื่อสแกนจ่าย'),
        h('button', { class: 'btn primary block lg', style: 'flex-direction:row', type: 'button', onclick: refreshQr }, 'สร้าง QR ใหม่'));
    } else if (o.qr_svg) {
      nodes.push(h('div', { style: 'margin:14px 0 6px' }, qrCard(o.qr_svg, { expiresIn: o.qr_expires_in, onExpire: () => { qrExpired = true; render(); } })),
        h('p', { class: 'muted' }, 'สแกนด้วยแอปธนาคาร ชำระแล้วระบบจะออกคิวให้อัตโนมัติ'));
    } else if (o.pay_state === 'queued') {
      nodes.push(h('p', null, 'กรุณารอสักครู่ ระบบกำลังเตรียม QR'));
    } else {
      nodes.push(h('p', null, 'QR ไม่พร้อมใช้งาน กรุณาแจ้งพนักงานที่เคาน์เตอร์'));
    }
    return h('div', { class: 'hero wait' }, nodes);
  }

  const STEPS = ['ชำระเงินแล้ว', 'กำลังเตรียม', 'ถึงคิวแล้ว'];

  function progress(at) {
    return h('div', { class: 'progress', role: 'list', 'aria-label': 'ขั้นตอน' },
      ...STEPS.map((text, i) => h('div', { class: 'st' + (i < at ? ' done' : i === at ? ' now' : ''), role: 'listitem' }, text)));
  }

  function hero(o) {
    switch (o.status) {
      case 'pending_payment': return payHero(o);
      case 'paid':
        if (Date.now() < successUntil) {
          return h('div', { class: 'hero ready' },
            h('div', { class: 'okmark', 'aria-hidden': 'true' }, '✓'),
            h('div', { class: 'ql', style: 'font-size:34px' }, 'ชำระเงินสำเร็จ'),
            h('div', { class: 'num', style: 'font-size:30px;font-weight:500' }, baht(o.total_satang)),
            h('div', { style: 'font-size:22px' }, 'คิวของคุณ ' + o.queue_label));
        }
        return h('div', { class: 'hero wait' },
          h('div', { class: 'lbl' }, 'คิวของคุณ'),
          h('div', { class: 'ql num' }, o.queue_label),
          h('div', { style: 'font-weight:500' }, o.ahead > 0 ? 'รออีก ' + o.ahead + ' คิว' : 'คิวถัดไป เตรียมตัวได้เลย'),
          progress(1),
          h('p', { class: 'muted noprint', style: 'margin:10px 0 0' }, 'เปิดหน้านี้ค้างไว้ แล้วกด “เปิดเสียง” เพื่อรับแจ้งเตือนเมื่อถึงคิว'));
      case 'called':
        return h('div', { class: 'hero ready pulse' },
          h('div', { class: 'ql' }, 'ถึงคิวแล้ว'),
          h('div', { style: 'font-size:26px;font-weight:500' }, 'คิว ' + o.queue_label + ' มารับสินค้าได้เลย'),
          progress(2));
      case 'done':
        return h('div', { class: 'hero wait' }, h('div', { class: 'ql', style: 'font-size:36px' }, 'รับสินค้าแล้ว'), h('div', null, 'ขอบคุณที่อุดหนุนครับ'));
      default:
        return h('div', { class: 'hero wait' }, h('div', { class: 'ql', style: 'font-size:36px' }, 'ออเดอร์ถูกยกเลิก'));
    }
  }

  const shopName = (o) => (o.bill && o.bill.shop) || window.SHOP;

  function render() {
    const o = order;
    if (o.qr_expires_in > 5) qrExpired = false;
    $('#hero').replaceChildren(hero(o));
    $('#receipt').replaceChildren(
      h('h3', { style: 'text-align:center' }, shopName(o)),
      ...((o.bill && o.bill.header) || []).map((line) => h('p', { class: 'muted', style: 'text-align:center;margin:0' }, line)),
      o.bill && o.bill.title ? h('p', { style: 'text-align:center;font-weight:600;margin:8px 0 0' }, o.bill.title) : null,
      h('p', { class: 'muted', style: 'text-align:center;margin:2px 0 8px' }, [o.queue_label ? (o.queue_label.startsWith('คิว') ? '' : 'คิว ') + o.queue_label : '', 'ออเดอร์ #' + o.id, !o.bill || o.bill.show_time ? when(o.created_at) : ''].filter(Boolean).join(' · ')),
      ...o.items.map((i) => h('div', { class: 'line' }, h('span', null, i.name + ' ×' + i.qty), h('span', { class: 'num' }, baht(i.unit_price_satang * i.qty)))),
      h('div', { class: 'sum' }, h('span', null, 'รวมทั้งหมด'), h('span', { class: 'num' }, baht(o.total_satang))),
      o.cash_received_satang != null ? h('div', { class: 'line' }, h('span', null, 'รับเงินสด'), h('span', { class: 'num' }, baht(o.cash_received_satang))) : null,
      o.change_satang != null ? h('div', { class: 'line' }, h('b', null, 'เงินทอน'), h('b', { class: 'num' }, baht(o.change_satang))) : null,
      h('p', { class: 'muted', style: 'text-align:center;margin:10px 0 0' },
        o.status === 'pending_payment' ? 'ยังไม่ได้ชำระเงิน' : 'ชำระแล้ว'),
      ...(o.status === 'pending_payment' ? [] : ((o.bill && o.bill.footer) || ['ขอบคุณครับ']).map((line) => h('p', { class: 'muted', style: 'text-align:center;margin:2px 0 0' }, line))));
    document.title = o.status === 'called' ? 'ถึงคิวแล้ว! · ' + shopName(o) : 'คิวและใบเสร็จ · ' + shopName(o);
    if (!printed && location.search.includes('print=1') && o.status !== 'pending_payment') {
      printed = true;
      setTimeout(() => window.print(), 300);
    }
  }

  async function load() {
    try { order = await api('GET', '/api/q/' + window.TOKEN); render(); } catch (err) { toast(err.message); }
  }

  function alertCalled(d) {
    if (navigator.vibrate) navigator.vibrate(VIBRATE);
    Voice.speak(d.say);
  }

  $('#print').addEventListener('click', () => window.print());
  Voice.bindButton($('#snd'));
  keepAwake();

  listen('/events/q/' + window.TOKEN, {
    order: load,
    pay: load,
    paid: () => {
      successUntil = Date.now() + SUCCESS_MS;
      load();
      setTimeout(() => { if (order) render(); }, SUCCESS_MS + 100);
    },
    refresh: load,
    called: (d) => { alertCalled(d); load(); },
  }, load);

  load();
})();
