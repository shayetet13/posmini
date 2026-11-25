(function () {
  const { $, h, baht, api, qrCard, listen, Voice, keepAwake } = PM;
  const stage = $('#stage');
  const FLASH_MS = 9000;
  const PAID_MS = 7000;
  const CLOCK_MS = 10000;
  let cart = null;
  let payView = null;
  let paidTimer;
  let flashTimer;

  function tickClock() {
    $('#clock').textContent = new Date().toLocaleTimeString('th-TH', { hour: '2-digit', minute: '2-digit', timeZone: 'Asia/Bangkok' }) + ' น.';
  }

  function idle() {
    stage.replaceChildren(
      h('div', { class: 'mid muted' }, 'ยินดีต้อนรับสู่'),
      h('div', { class: 'big' }, window.DISPLAY_NAME || window.SHOP),
      h('div', { class: 'steps' }, h('span', null, '1 เลือกสินค้ากับพนักงาน'), h('span', null, '2 สแกน QR ชำระเงิน'), h('span', null, '3 รอรับคิว')));
  }

  function showCart() {
    if (!cart || !cart.items.length) { idle(); return; }
    stage.replaceChildren(
      h('div', { class: 'list card' },
        ...cart.items.map((i) => h('div', { class: 'row' },
          h('span', { class: 'nm' }, i.name + ' ×' + i.qty),
          h('span', { class: 'num' }, baht(i.qty * i.price_satang))))),
      cart.discount_satang ? h('div', { class: 'mid muted' }, 'ส่วนลด −' + baht(cart.discount_satang)) : null,
      h('div', { class: 'mid muted' }, 'รวมทั้งหมด'),
      h('div', { class: 'total-chip num' }, baht(cart.total_satang)));
  }

  function showPay(d) {
    payView = d;
    const head = [h('div', { class: 'mid' }, 'สแกนเพื่อชำระเงิน'), h('div', { class: 'total-chip num' }, baht(d.total_satang))];
    if (d.state === 'pending' || d.state === 'manual') {
      stage.replaceChildren(...head,
        qrCard(d.qr_svg || '', { expiresIn: d.expires_in }),
        h('div', { class: 'steps' }, h('span', null, '1 เปิดแอปธนาคาร'), h('span', null, '2 สแกน QR นี้'), h('span', null, '3 ยืนยันการโอน')));
      return;
    }
    const message = {
      queued: 'กรุณารอสักครู่ ระบบกำลังเตรียม QR',
      expired: 'QR หมดอายุ กรุณาแจ้งพนักงาน',
      cancelled: 'รายการถูกยกเลิก',
      ambiguous: 'กรุณารอพนักงานตรวจสอบยอด',
    }[d.state] || 'กำลังเตรียม QR…';
    stage.replaceChildren(...head, h('div', { class: 'mid muted' }, message));
  }

  function showPaid(d) {
    payView = null;
    cart = null;
    stage.replaceChildren(
      h('div', { class: 'okmark okmark-xl', 'aria-hidden': 'true' }, '✓'),
      h('div', { class: 'big', style: 'color:var(--jade)' }, 'ชำระเงินแล้ว'),
      h('div', { class: 'mid' }, 'ขอบคุณครับ · ' + baht(d.total_satang)),
      d.change_satang != null ? h('div', { class: 'mid', style: 'color:var(--jade)' }, 'เงินทอน ', h('b', { class: 'num' }, baht(d.change_satang))) : null,
      h('div', { class: 'mid muted' }, 'หมายเลขคิวของคุณ'),
      h('div', { class: 'total-chip num' }, d.queue_label));
    clearTimeout(paidTimer);
    paidTimer = setTimeout(() => { if (!payView) showCart(); }, PAID_MS);
  }

  function showCalled(d) {
    $('#called').textContent = 'กำลังเรียกคิว ' + d.queue_label;
    const flash = $('#flash');
    flash.replaceChildren(
      h('div', { class: 'mid' }, 'คิว'),
      h('div', { class: 'big pulse num', style: 'font-size:clamp(72px,22vw,240px)' }, d.queue_label),
      h('div', { class: 'big' }, 'ถึงคิวแล้ว'));
    flash.classList.add('open');
    Voice.speak(d.say);
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => flash.classList.remove('open'), FLASH_MS);
  }

  $('#flash').addEventListener('click', () => $('#flash').classList.remove('open'));
  Voice.bindButton($('#snd'));
  if (!Voice.enabled) {
    // Browsers (iPad especially) only allow sound after a tap, so ask for one up front.
    const hint = h('button', { class: 'soundhint', type: 'button' }, 'แตะที่นี่เพื่อเปิดเสียงประกาศคิว');
    hint.addEventListener('click', () => $('#snd').click());
    $('#snd').addEventListener('click', () => hint.remove());
    document.body.append(hint);
  }
  keepAwake();
  tickClock();
  setInterval(tickClock, CLOCK_MS);
  idle();

  // Catch up with what the cashier is doing right now (page opened late, or the link dropped).
  async function sync() {
    try {
      const s = await api('GET', '/api/display/' + window.SLUG + '/state');
      if (s.pay) { cart = null; showPay(s.pay); return; }
      payView = null;
      cart = s.cart;
      showCart();
    } catch (_) { /* the next event will bring the screen up to date */ }
  }

  listen('/events/display/' + window.SLUG, {
    // The cashier clears the cart right after starting a payment; that must not wipe the QR.
    cart: (d) => {
      if (payView && !d.items.length) return;
      cart = d; payView = null; showCart();
    },
    pay: showPay,
    paid: showPaid,
    called: showCalled,
    idle: (d) => {
      if (payView && d.order_id && payView.order_id !== d.order_id) return;
      cart = null; payView = null; idle();
    },
  }, sync);

  sync();
})();
