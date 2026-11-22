(function () {
  const { $, h, baht, api, upload, productPic, toast } = PM;
  const IMAGE_SIDE = 640;
  const TAB_KEYS = ['menu', 'pay', 'bill', 'print'];
  const tabFromHash = () => (TAB_KEYS.includes(location.hash.slice(1)) ? location.hash.slice(1) : 'menu');
  const state = { tab: tabFromHash(), products: [], settings: null, testMsg: null, selecting: false, selected: new Set(), print: null, bill: null, billKind: 'ticket', targets: null, targetsLoading: false };

  // A test sheet is printed by the server, or (cloud server, web mode) by this browser.
  async function runTest(kind) {
    const res = await api('POST', '/api/print/test', { kind });
    if (res && res.web) {
      if (!window.Station) throw new Error('หน้านี้ยังไม่พร้อมพิมพ์ผ่านเบราว์เซอร์');
      await Station.printBytes(Station.fromBase64(res.bytes));
    }
  }

  const TABS = [['menu', 'เมนูสินค้า'], ['pay', 'การรับเงิน'], ['bill', 'บิลและเลขคิว'], ['print', 'เครื่องพิมพ์']];
  const ID_TYPES = { phone: 'เบอร์โทรศัพท์', national_id: 'เลขบัตรประชาชน', ewallet: 'e-Wallet' };

  function setTab(tab) {
    state.tab = tab;
    history.replaceState(null, '', '#' + tab);
    if (tab === 'print') loadTargets(false);
    render();
  }

  function renderTabs() {
    $('#tabs').replaceChildren(...TABS.map(([key, label]) =>
      h('button', { class: 'chip' + (key === state.tab ? ' on' : ''), type: 'button', role: 'tab', 'aria-selected': key === state.tab, onclick: () => setTab(key) }, label)));
  }

  function render() {
    renderTabs();
    const views = { menu: renderMenu, pay: renderPay, bill: renderBill, print: renderPrint };
    $('#view').replaceChildren(...views[state.tab]().filter(Boolean));
  }

  // ------------------------------------------------------------- menu tab

  async function loadProducts() { state.products = await api('GET', '/api/products'); }

  function setSelecting(on) {
    state.selecting = on;
    state.selected = new Set();
    render();
  }

  function toggleSelected(id, on) {
    if (on) state.selected.add(id); else state.selected.delete(id);
    render();
  }

  async function removeProducts(ask, request, doneText) {
    if (!(await PM.confirm(ask.message, { title: ask.title, okText: ask.okText }))) return;
    try {
      await request();
      state.selecting = false;
      state.selected = new Set();
      await loadProducts();
      render();
      toast(doneText);
    } catch (err) { toast(err.message); }
  }

  function deleteSelected() {
    const ids = [...state.selected];
    if (!ids.length) return;
    return removeProducts(
      { title: 'ลบสินค้าที่เลือก', message: 'ลบสินค้า ' + ids.length + ' รายการที่เลือก?\nออเดอร์เก่ายังอยู่ตามเดิม', okText: 'ลบ ' + ids.length + ' รายการ' },
      () => api('POST', '/api/products/bulk-delete', { ids }),
      'ลบแล้ว ' + ids.length + ' รายการ');
  }

  function deleteAll() {
    const n = state.products.length;
    if (!n) return;
    return removeProducts(
      { title: 'ลบสินค้าทั้งหมด', message: 'ลบสินค้าทั้งหมด ' + n + ' รายการออกจากเมนู?\nออเดอร์เก่ายังอยู่ตามเดิม แต่ต้องเพิ่มสินค้าใหม่เอง', okText: 'ลบทั้งหมด' },
      () => api('DELETE', '/api/products'),
      'ลบสินค้าทั้งหมดแล้ว');
  }

  function productRow(p) {
    const info = [
      h('div', { class: 'pthumb' }, productPic(p, true) || h('span', { class: 'im ph' }, '')),
      h('div', { class: 'pinfo' },
        h('div', { class: 'pname' }, p.name),
        h('div', { class: 'muted' }, p.category || 'ไม่มีหมวด')),
      h('b', { class: 'num' }, baht(p.price_satang)),
    ];
    if (!state.selecting) {
      return h('div', { class: 'prow' }, ...info, h('button', { class: 'btn small', type: 'button', onclick: () => openEditor(p) }, 'แก้ไข'));
    }
    const box = h('input', { type: 'checkbox', class: 'pcheck', 'aria-label': 'เลือก ' + p.name, checked: state.selected.has(p.id) });
    box.addEventListener('change', () => toggleSelected(p.id, box.checked));
    return h('label', { class: 'prow selectable' }, box, ...info);
  }

  function menuToolbar() {
    const total = state.products.length;
    if (!state.selecting) {
      return h('div', { class: 'total', style: 'margin-bottom:10px;flex-wrap:wrap' },
        h('h2', { style: 'font-size:18px' }, 'เมนูสินค้า (' + total + ')'),
        h('div', { class: 'erow', style: 'margin:0' },
          h('button', { class: 'btn small', type: 'button', disabled: !total, onclick: () => setSelecting(true) }, 'เลือกลบ'),
          h('button', { class: 'btn small danger', type: 'button', disabled: !total, onclick: deleteAll }, 'ลบทั้งหมด'),
          h('button', { class: 'btn primary', type: 'button', onclick: () => openEditor(null) }, '+ เพิ่มสินค้า')));
    }
    const allOn = total > 0 && state.selected.size === total;
    return h('div', { class: 'card selbar' },
      h('b', null, 'เลือกแล้ว ' + state.selected.size + ' / ' + total),
      h('div', { class: 'erow', style: 'margin:0' },
        h('button', { class: 'btn small', type: 'button', onclick: () => { state.selected = allOn ? new Set() : new Set(state.products.map((p) => p.id)); render(); } }, allOn ? 'ไม่เลือกทั้งหมด' : 'เลือกทั้งหมด'),
        h('button', { class: 'btn small danger-fill', type: 'button', disabled: !state.selected.size, onclick: deleteSelected }, 'ลบที่เลือก (' + state.selected.size + ')'),
        h('button', { class: 'btn small', type: 'button', onclick: () => setSelecting(false) }, 'ยกเลิก')));
  }

  function renderMenu() {
    const anyPic = state.products.some((p) => p.image_v != null);
    const rows = state.products.map(productRow);
    return [
      menuToolbar(),
      h('div', { class: 'card' }, ...(rows.length ? rows : [h('p', { class: 'muted', style: 'text-align:center;padding:16px 0' }, 'ยังไม่มีสินค้า กด “เพิ่มสินค้า”')])),
      anyPic || !rows.length || state.selecting ? null : h('p', { class: 'muted', style: 'margin-top:10px' }, 'เคล็ดลับ: ใส่รูปสินค้าได้ที่ปุ่ม “แก้ไข” รูปจะแสดงบนหน้าขายและหน้าสั่งของลูกค้า'),
    ];
  }

  function parsePrice(text) {
    const value = Number.parseFloat(String(text).replace(/,/g, '').trim());
    if (!Number.isFinite(value) || value <= 0) return null;
    return Math.round(value * 100);
  }

  function loadImage(file) {
    return new Promise((resolve, reject) => {
      const url = URL.createObjectURL(file);
      const img = new Image();
      img.onload = () => { URL.revokeObjectURL(url); resolve(img); };
      img.onerror = () => { URL.revokeObjectURL(url); reject(new Error('เปิดไฟล์รูปนี้ไม่ได้')); };
      img.src = url;
    });
  }

  async function toJpeg(file) {
    const img = await loadImage(file);
    const scale = Math.min(1, IMAGE_SIDE / Math.max(img.naturalWidth, img.naturalHeight));
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(img.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(img.naturalHeight * scale));
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#fff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return new Promise((resolve, reject) => canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error('แปลงรูปไม่สำเร็จ'))), 'image/jpeg', 0.82));
  }

  function openEditor(p) {
    const edit = { blob: null, previewUrl: null, removeImage: false, busy: false };
    const field = (label, input) => h('label', { class: 'field' }, h('span', null, label), input);
    const name = h('input', { value: p ? p.name : '', maxlength: 60, autocomplete: 'off', placeholder: 'เช่น ลาเต้เย็น' });
    const cats = [...new Set(state.products.map((x) => x.category).filter(Boolean))];
    const category = h('input', { value: p ? p.category : '', maxlength: 30, list: 'catlist', autocomplete: 'off', placeholder: 'เช่น กาแฟ' });
    const price = h('input', { value: p ? String(p.price_satang / 100) : '', inputmode: 'decimal', autocomplete: 'off', placeholder: 'เช่น 45' });
    const preview = h('div', { class: 'epreview' });
    const file = h('input', { type: 'file', accept: 'image/*', hidden: true });
    const removeBtn = h('button', { class: 'btn small danger', type: 'button' }, 'ลบรูป');

    function paint() {
      const savedUrl = p && p.image_v != null && !edit.removeImage ? '/img/' + p.id + '?v=' + p.image_v : null;
      const src = edit.previewUrl || savedUrl;
      preview.replaceChildren(src ? h('img', { src, alt: 'รูปสินค้า' }) : h('span', { class: 'muted' }, 'ยังไม่มีรูป'));
      removeBtn.hidden = !src;
    }

    file.addEventListener('change', async () => {
      const chosen = file.files && file.files[0];
      file.value = '';
      if (!chosen) return;
      try {
        const blob = await toJpeg(chosen);
        if (edit.previewUrl) URL.revokeObjectURL(edit.previewUrl);
        edit.blob = blob;
        edit.previewUrl = URL.createObjectURL(blob);
        edit.removeImage = false;
        paint();
      } catch (err) { toast(err.message); }
    });
    removeBtn.addEventListener('click', () => {
      if (edit.previewUrl) URL.revokeObjectURL(edit.previewUrl);
      edit.blob = null; edit.previewUrl = null; edit.removeImage = true;
      paint();
    });

    function close() {
      if (edit.previewUrl) URL.revokeObjectURL(edit.previewUrl);
      $('#editmodal').classList.remove('open');
    }

    const saveBtn = h('button', { class: 'btn primary block', type: 'button' }, 'บันทึก');
    saveBtn.addEventListener('click', async () => {
      if (edit.busy) return;
      const satang = parsePrice(price.value);
      if (!name.value.trim()) { toast('กรอกชื่อสินค้า'); return; }
      if (satang == null) { toast('กรอกราคาเป็นตัวเลขมากกว่า 0'); return; }
      edit.busy = true; saveBtn.disabled = true;
      try {
        const body = { name: name.value, category: category.value, price_satang: satang };
        let id;
        if (p) { await api('PUT', '/api/products/' + p.id, body); id = p.id; }
        else { id = (await api('POST', '/api/products', body)).id; }
        if (edit.blob) await upload('/api/products/' + id + '/image', edit.blob);
        else if (edit.removeImage && p && p.image_v != null) await api('DELETE', '/api/products/' + id + '/image');
        close();
        await loadProducts();
        render();
        toast('บันทึกแล้ว');
      } catch (err) {
        toast(err.message);
        edit.busy = false; saveBtn.disabled = false;
        await loadProducts().then(render).catch(() => {});
      }
    });

    const actions = [saveBtn];
    if (p) {
      actions.push(h('button', { class: 'btn danger block', type: 'button', onclick: async () => {
        if (!(await PM.confirm('ลบ “' + p.name + '” ออกจากเมนู?\nออเดอร์เก่ายังอยู่ตามเดิม', { title: 'ลบสินค้า', okText: 'ลบสินค้า' }))) return;
        try { await api('DELETE', '/api/products/' + p.id); close(); await loadProducts(); render(); toast('ลบแล้ว'); } catch (err) { toast(err.message); }
      } }, 'ลบสินค้า'));
    }
    actions.push(h('button', { class: 'btn block', type: 'button', onclick: close }, 'ยกเลิก'));

    $('#editbox').replaceChildren(
      h('h3', { style: 'margin-bottom:10px' }, p ? 'แก้ไขสินค้า' : 'เพิ่มสินค้า'),
      preview,
      h('div', { class: 'erow' },
        h('button', { class: 'btn small', type: 'button', onclick: () => file.click() }, 'เลือกรูป / ถ่ายรูป'),
        removeBtn, file),
      h('datalist', { id: 'catlist' }, cats.map((c) => h('option', { value: c }))),
      field('ชื่อสินค้า', name),
      field('หมวด', category),
      field('ราคา (บาท)', price),
      h('div', { class: 'btns' }, ...actions));
    paint();
    $('#editmodal').classList.add('open');
    name.focus();
  }

  // -------------------------------------------------------------- pay tab

  function missingFields(s) {
    const gaps = [];
    if (!s.base_url) gaps.push('ที่อยู่ API');
    if (!s.api_key_set) gaps.push('API key');
    if (!s.id_type) gaps.push('เลขพร้อมเพย์');
    return gaps;
  }

  function renderPay() {
    const s = state.settings;
    if (!s) return [h('p', { class: 'muted' }, 'กำลังโหลด…')];
    const input = (value, attrs) => h('input', { value: value || '', autocomplete: 'off', spellcheck: 'false', ...attrs });
    const ppId = input(s.promptpay_id, { inputmode: 'numeric', maxlength: 20, placeholder: '0812345678' });
    const ppHint = h('span', { class: 'muted' });
    const drawHint = () => {
      const digits = ppId.value.replace(/\D/g, '');
      const kind = digits.length === 10 && digits.startsWith('0') ? 'phone' : digits.length === 13 ? 'national_id' : digits.length === 15 ? 'ewallet' : null;
      ppHint.textContent = digits ? (kind ? 'ประเภท: ' + ID_TYPES[kind] : 'ยังไม่ครบ (เบอร์โทร 10 หลัก / บัตรประชาชน 13 หลัก / e-Wallet 15 หลัก)') : 'เบอร์โทร 10 หลัก, เลขบัตรประชาชน 13 หลัก หรือ e-Wallet 15 หลัก';
    };
    ppId.addEventListener('input', drawHint);
    drawHint();

    const base = input(s.base_url, { inputmode: 'url', placeholder: 'https://api.example.com' });
    const key = input('', { type: 'password', name: 'verify-api-key', placeholder: s.api_key_set ? 'ตั้งไว้แล้ว (เว้นว่างเพื่อใช้ค่าเดิม)' : 'lbk_…' });
    const last4 = input(s.dest_last4, { inputmode: 'numeric', maxlength: 4, placeholder: 'ไม่บังคับ' });
    const bot = input(s.bot_id, { placeholder: 'เว้นว่าง = ให้ระบบหาเอง' });
    const target = input(s.source_target_id, { placeholder: 'เว้นว่าง = ให้ระบบหาเอง' });
    const field = (label, el, note) => h('label', { class: 'field' }, h('span', null, label), el, note || null);

    const gaps = missingFields(s);
    const status = s.verify_active
      ? h('span', { class: 'badge ok' }, 'ตรวจยอดเงินเข้าอัตโนมัติ: เปิดใช้งาน')
      : h('span', { class: 'badge warn' }, 'ตรวจยอดอัตโนมัติ: ปิด · แคชเชียร์กดยืนยันเอง');
    const testBox = h('div', { class: 'testres', role: 'status' });
    if (state.testMsg) {
      testBox.className = 'testres ' + (state.testMsg.ok ? 'ok' : 'bad');
      testBox.textContent = state.testMsg.message;
    }

    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, 'บันทึก');
    const testBtn = h('button', { class: 'btn', type: 'button' }, 'ทดสอบการเชื่อมต่อ');
    saveBtn.addEventListener('click', async () => {
      saveBtn.disabled = true;
      try {
        state.settings = await api('PUT', '/api/settings', {
          base_url: base.value, api_key: key.value, promptpay_id: ppId.value, dest_last4: last4.value,
          bot_id: bot.value, source_target_id: target.value,
        });
        state.testMsg = null;
        toast('บันทึกแล้ว ใช้งานได้ทันที');
        render();
      } catch (err) { toast(err.message); saveBtn.disabled = false; }
    });
    testBtn.addEventListener('click', async () => {
      testBtn.disabled = true;
      try { state.testMsg = await api('POST', '/api/settings/test'); } catch (err) { state.testMsg = { ok: false, message: err.message }; }
      render();
    });

    return [
      h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'เลขพร้อมเพย์ที่รับเงิน'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ใช้สร้าง QR พร้อมเพย์ให้ลูกค้าสแกนจ่าย พร้อมยอดเงินของแต่ละออเดอร์'),
        field('เลขพร้อมเพย์', ppId, ppHint)),
      h('div', { class: 'card' },
        h('div', { class: 'total', style: 'margin-bottom:6px' }, h('h2', { style: 'font-size:18px' }, 'ระบบตรวจยอดเงินเข้า'), status),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'เมื่อกรอกครบ ระบบจะตรวจว่าลูกค้าจ่ายแล้วให้อัตโนมัติ แล้วพูดจำนวนเงินเป็นภาษาไทย ถ้ายังไม่กรอก ให้แคชเชียร์กด “ยืนยันรับเงินแล้ว” เอง'),
        gaps.length && !s.verify_active ? h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ยังขาด: ' + gaps.join(', ')) : null,
        field('ที่อยู่ API', base),
        field('API key', key, h('span', { class: 'muted' }, 'เก็บในฐานข้อมูลของเครื่องนี้ ไม่ส่งกลับมาแสดงในหน้านี้')),
        field('เลข 4 ตัวท้ายบัญชีปลายทาง', last4, h('span', { class: 'muted' }, 'ไม่บังคับ ใส่เมื่อ API ของคุณต้องการ')),
        h('details', { class: 'adv' },
          h('summary', null, 'ขั้นสูง: Bot ID / Source target ID (ปกติเว้นว่างได้)'),
          h('p', { class: 'muted', style: 'margin:6px 0 10px' }, 'API ต้องใช้สองค่านี้ ถ้าเว้นว่าง ระบบจะหาให้เองจาก API key เมื่อมีบอทที่เปิดใช้งาน 1 ตัว และแหล่งแจ้งเงินเข้าที่เปิดใช้งาน 1 อัน ใส่เองเมื่อมีหลายอัน'),
          field('Bot ID', bot),
          field('Source target ID', target)),
        h('div', { class: 'erow', style: 'margin-top:6px' }, saveBtn, testBtn),
        testBox,
        h('p', { class: 'muted', style: 'margin:8px 0 0;font-size:14px' }, 'ปุ่มทดสอบใช้ค่าที่บันทึกแล้ว: ค้นหารายการสมมติ 1 รายการ ทั้งแบบมีและไม่มี key เพื่อดูว่าเซิร์ฟเวอร์ตรวจ key จริง จึงไม่สร้างรายการเก็บเงินจริง และผลที่ตอบ 404 ของรายการสมมติถือว่าปกติ')),
    ];
  }

  // --------------------------------------------------------------- bill tab

  const PREVIEW_DELAY_MS = 350;
  let previewTimer = null;
  let previewSeq = 0;
  let previewUrl = null;

  function billBody(f) {
    return {
      shop_name: f.shop_name.value, header: f.header.value, footer: f.footer.value,
      ticket_title: f.ticket_title.value, slip_title: f.slip_title.value, ticket_note: f.ticket_note.value,
      queue_prefix: f.queue_prefix.value, queue_digits: Number(f.queue_digits.value),
      show_time: f.show_time.checked,
      show_method: f.show_method.checked, show_order_no: f.show_order_no.checked,
    };
  }

  // Shows the bill as it will print: the server draws the same picture it sends to the printer.
  async function drawPreview(f, img, msg) {
    const seq = ++previewSeq;
    try {
      const res = await fetch('/api/bill/preview', {
        method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...billBody(f), kind: state.billKind }),
      });
      if (seq !== previewSeq) return;
      if (!res.ok) {
        let data = null;
        try { data = await res.json(); } catch (_) { /* not json */ }
        throw new Error((data && data.error) || 'สร้างตัวอย่างไม่ได้');
      }
      const url = URL.createObjectURL(await res.blob());
      if (previewUrl) URL.revokeObjectURL(previewUrl);
      previewUrl = url;
      img.src = url;
      img.hidden = false;
      msg.textContent = '';
    } catch (err) {
      if (seq === previewSeq) msg.textContent = err.message;
    }
  }

  function renderBill() {
    const b = state.bill;
    if (!b) return [h('p', { class: 'muted' }, 'กำลังโหลด…')];
    const field = (label, el, note) => h('label', { class: 'field' }, h('span', null, label), el, note || null);
    const text = (value, max, placeholder) => h('input', { value, maxlength: max, placeholder: placeholder || '', autocomplete: 'off' });
    const area = (value, rows, max, placeholder) => { const el = h('textarea', { rows, maxlength: max, placeholder }); el.value = value; return el; };
    const check = (label, value) => {
      const box = h('input', { type: 'checkbox', class: 'pcheck', checked: value });
      return { box, row: h('label', { class: 'checkrow' }, box, h('span', null, label)) };
    };

    const f = {
      shop_name: text(b.shop_name, 40, b.shop_default),
      header: area(b.header, 3, 200, 'ที่อยู่ / เบอร์โทร / เลขผู้เสียภาษี (บรรทัดละเรื่อง สูงสุด 4 บรรทัด)'),
      footer: area(b.footer, 2, 100, 'เช่น ขอบคุณที่อุดหนุน (สูงสุด 2 บรรทัด)'),
      ticket_title: text(b.ticket_title, 24, 'เช่น บัตรคิว / ใบเสร็จรับเงิน'),
      slip_title: text(b.slip_title, 24, 'เช่น ใบออเดอร์'),
      ticket_note: text(b.ticket_note, 48, 'เช่น กรุณารอเรียกคิว'),
      queue_prefix: text(b.queue_prefix, 3, 'เช่น A'),
      queue_digits: h('select', null, [1, 2, 3, 4, 5].map((n) => h('option', { value: String(n), selected: n === b.queue_digits }, n + ' หลัก'))),
    };
    const flags = {
      show_time: check('แสดงวันและเวลา', b.show_time),
      show_method: check('แสดงวิธีชำระเงิน', b.show_method),
      show_order_no: check('แสดงเลขที่ออเดอร์ (ตัวใหญ่ใต้เลขคิว)', b.show_order_no),
    };
    for (const [key, flag] of Object.entries(flags)) f[key] = flag.box;

    const example = h('b', { class: 'num' });
    const paintExample = () => {
      const digits = Number(f.queue_digits.value);
      example.textContent = f.queue_prefix.value.trim() + '7'.padStart(digits, '0');
    };
    paintExample();

    const img = h('img', { class: 'bill-img', alt: 'ตัวอย่างบิล', hidden: true });
    const msg = h('p', { class: 'bill-msg' });
    const refresh = () => { clearTimeout(previewTimer); previewTimer = setTimeout(() => drawPreview(f, img, msg), PREVIEW_DELAY_MS); };
    for (const el of Object.values(f)) { el.addEventListener('input', () => { paintExample(); refresh(); }); el.addEventListener('change', () => { paintExample(); refresh(); }); }

    const kinds = [['ticket', 'บิลลูกค้า'], ['slip', 'ใบออเดอร์ร้าน']];
    const kindTabs = h('div', { class: 'chips', role: 'tablist' }, kinds.map(([key, label]) =>
      h('button', { class: 'chip' + (state.billKind === key ? ' on' : ''), type: 'button', role: 'tab', 'aria-selected': state.billKind === key,
        onclick: () => { state.billKind = key; render(); } }, label)));

    const run = (btn, work) => async () => {
      btn.disabled = true;
      try { await work(); } catch (err) { toast(err.message); }
      btn.disabled = false;
    };
    async function save() { state.bill = await api('PUT', '/api/bill/settings', billBody(f)); }
    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, 'บันทึก');
    saveBtn.addEventListener('click', run(saveBtn, async () => { await save(); toast('บันทึกแล้ว ใช้ได้ทันที'); render(); }));
    const testBtn = (label, kind, doneText) => {
      const btn = h('button', { class: 'btn', type: 'button' }, label);
      btn.addEventListener('click', run(btn, async () => { await save(); await runTest(kind); toast(doneText); }));
      return btn;
    };

    drawPreview(f, img, msg);

    return [h('div', { class: 'bill-layout' },
      h('div', { class: 'bill-form' },
        h('div', { class: 'card', style: 'margin-bottom:12px' },
          h('h2', { style: 'font-size:18px;margin-bottom:8px' }, 'ข้อความบนบิล'),
          field('ชื่อร้านบนบิล', f.shop_name, h('span', { class: 'muted' }, 'เว้นว่างจะใช้ “' + b.shop_default + '”')),
          field('หัวบิล', f.header),
          field('ท้ายบิล', f.footer),
          field('หัวข้อบิลลูกค้า (บัตรคิวและใบเสร็จรวมกัน)', f.ticket_title),
          field('ข้อความใต้รายการ', f.ticket_note),
          field('หัวข้อใบออเดอร์สำหรับร้าน', f.slip_title)),
        h('div', { class: 'card', style: 'margin-bottom:12px' },
          h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'เลขคิว'),
          h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ตัวอย่างคิวที่ 7: ', example),
          h('div', { class: 'two' }, field('ตัวอักษรหน้าเลข', f.queue_prefix), field('จำนวนหลัก', f.queue_digits)),
          h('p', { class: 'muted', style: 'margin:0;font-size:14px' }, 'เปลี่ยนแล้วมีผลกับทุกหน้าจอและบิล รวมถึงคิวของวันนี้ที่ยังไม่รับของ เลขคิวยังเริ่มนับใหม่ทุกวัน')),
        h('div', { class: 'card', style: 'margin-bottom:12px' },
          h('h2', { style: 'font-size:18px;margin-bottom:8px' }, 'แสดงอะไรบ้าง'),
          ...Object.values(flags).map((flag) => flag.row)),
        h('div', { class: 'erow' }, saveBtn, testBtn('ทดสอบพิมพ์บิลลูกค้า', 'ticket', 'ส่งบิลตัวอย่างแล้ว'), testBtn('ทดสอบใบออเดอร์ร้าน', 'slip', 'ส่งใบออเดอร์ตัวอย่างแล้ว')),
        h('p', { class: 'muted', style: 'font-size:14px' }, 'ปุ่มทดสอบจะบันทึกก่อนแล้วสั่งพิมพ์ ต้องตั้งค่าเครื่องพิมพ์ในแท็บ “เครื่องพิมพ์” ก่อน แก้รายการสินค้าในออเดอร์ที่ชำระแล้วไม่ได้ หน้านี้แก้ได้เฉพาะข้อความและรูปแบบของบิล')),
      h('aside', { class: 'bill-side' },
        kindTabs,
        h('div', { class: 'bill-paper' }, img, msg),
        h('p', { class: 'muted', style: 'font-size:14px;margin:8px 0 0;text-align:center' }, 'ตัวอย่างตรงกับที่พิมพ์จริงเมื่อเลือกพิมพ์แบบรูปภาพ (ขนาดกระดาษตามที่ตั้งในแท็บเครื่องพิมพ์)')))];
  }

  // ------------------------------------------------------------ printer tab

  async function loadTargets(force) {
    if (state.targetsLoading || (state.targets && !force)) return;
    state.targetsLoading = true;
    render();
    try { state.targets = await api('GET', '/api/print/targets'); } catch (err) { state.targets = { printers: [], ports: [] }; toast(err.message); }
    state.targetsLoading = false;
    render();
  }

  const PRINT_MODES = [
    ['off', 'ไม่ใช้เครื่องพิมพ์'],
    ['printer', 'เครื่องพิมพ์ที่ติดตั้งใน Windows (USB หรือ Bluetooth ที่ติดตั้งเป็นเครื่องพิมพ์แล้ว)'],
    ['com', 'พอร์ต COM (Bluetooth ที่จับคู่แล้ว หรือ USB แบบ Serial)'],
    ['web', 'ผ่านเบราว์เซอร์ (เซิร์ฟเวอร์อยู่บนคลาวด์ เช่น Railway) — Chrome/Edge บนเครื่องที่ต่อเครื่องพิมพ์'],
  ];

  let stopStationWatch = null;

  // Connect the printer attached to THIS computer (Chrome/Edge: Web Serial for Bluetooth/COM, WebUSB).
  function buildStationBox(field) {
    const S = window.Station;
    if (!S) return h('p', { class: 'warnbox' }, 'โหลดสถานีพิมพ์ไม่สำเร็จ ลองรีเฟรชหน้านี้');
    const c = S.cfg();
    const status = h('b', null);
    const names = { off: 'ปิดอยู่', noport: 'ยังไม่ได้เชื่อมต่อเครื่องพิมพ์', ready: 'พร้อมพิมพ์', printing: 'กำลังพิมพ์', error: 'ผิดพลาด' };
    const paint = () => { status.textContent = (names[S.status] || S.status) + (S.label ? ' ' + S.label : '') + (S.message ? ' · ' + S.message : ''); };
    paint();
    if (stopStationWatch) stopStationWatch();
    stopStationWatch = S.onChange(paint);

    const baud = h('select', null, ...S.bauds.map((b) => h('option', { value: String(b), selected: c.baud === b }, String(b))));
    baud.addEventListener('change', () => S.setBaud(Number(baud.value)));
    const on = h('input', { type: 'checkbox', class: 'pcheck', checked: c.on });
    on.addEventListener('change', () => S.setOn(on.checked));
    const connect = (kind, label, ok) => {
      const btn = h('button', { class: 'btn', type: 'button', disabled: !ok }, label);
      btn.addEventListener('click', async () => {
        try { await S.connect(kind); toast('เลือกเครื่องพิมพ์แล้ว'); } catch (err) { toast(err.message); }
      });
      return btn;
    };
    const unsupported = !S.supported.serial && !S.supported.usb;
    return h('div', { class: 'stationbox' },
      h('p', { class: 'muted', style: 'margin:0 0 8px' }, 'เซิร์ฟเวอร์อยู่บนคลาวด์ จึงสั่งเครื่องพิมพ์โดยตรงไม่ได้ ให้เปิดหน้า “ขาย” ในเบราว์เซอร์ของคอมพิวเตอร์ที่ต่อเครื่องพิมพ์ (Chrome หรือ Edge) แล้วเชื่อมต่อด้านล่าง หน้านั้นจะรับคิวพิมพ์มาสั่งเครื่องเอง ต้องเปิดค้างไว้ตอนขาย'),
      unsupported ? h('p', { class: 'warnbox' }, 'เบราว์เซอร์นี้เชื่อมต่อเครื่องพิมพ์ไม่ได้ (iPad/Safari/Firefox ใช้ไม่ได้) ใช้ Chrome หรือ Edge บน Windows หรือ Android') : null,
      h('div', { class: 'erow' },
        connect('serial', 'เชื่อมต่อ Bluetooth / COM', S.supported.serial),
        connect('usb', 'เชื่อมต่อ USB', S.supported.usb)),
      field('ความเร็วพอร์ต (baud)', baud, h('span', { class: 'muted' }, 'Bluetooth ไม่มีผล ถ้าเป็นสาย USB-Serial ดูในคู่มือเครื่อง ค่าทั่วไปคือ 9600 หรือ 115200')),
      h('label', { class: 'checkrow' }, on, h('span', null, 'ใช้เครื่องนี้เป็นสถานีพิมพ์')),
      h('p', { style: 'margin:6px 0 0' }, 'สถานะ: ', status));
  }

  function renderPrint() {
    const p = state.print;
    if (!p) return [h('p', { class: 'muted' }, 'กำลังโหลด…')];
    const targets = state.targets || { printers: [], ports: [] };
    const field = (label, el, note) => h('label', { class: 'field' }, h('span', null, label), el, note || null);
    const check = (label, value) => {
      const box = h('input', { type: 'checkbox', class: 'pcheck', checked: value });
      return { box, row: h('label', { class: 'checkrow' }, box, h('span', null, label)) };
    };

    const mode = h('select', null, ...PRINT_MODES.map(([v, label]) => h('option', { value: v, selected: p.mode === v }, label)));
    const printer = h('select', null,
      h('option', { value: '' }, targets.printers.length ? '— เลือกเครื่องพิมพ์ —' : '— ไม่พบเครื่องพิมพ์ใน Windows —'),
      ...[...new Set([...targets.printers, ...(p.mode === 'printer' && p.target ? [p.target] : [])])].map((n) => h('option', { value: n, selected: p.mode === 'printer' && p.target === n }, n)));
    const com = h('input', { value: p.mode === 'com' ? p.target : '', list: 'comlist', placeholder: 'เช่น COM5', autocomplete: 'off', maxlength: 6 });
    const comList = h('datalist', { id: 'comlist' }, targets.ports.map((c) => h('option', { value: c })));
    const stationBox = p.mode === 'web' ? buildStationBox(field) : null;
    const num = (value, placeholder, max) => h('input', { value: String(value), inputmode: 'numeric', placeholder, maxlength: max, autocomplete: 'off' });
    const headDots = num(p.head_dots || '', 'อัตโนมัติ (' + (p.width === 80 ? 576 : 384) + ')', 3);
    const shift = num(p.shift, '0', 4);
    const gap = num(p.gap_secs, '4', 2);
    const width = h('select', null, h('option', { value: '58', selected: p.width === 58 }, '58 มม. (เครื่องพิมพ์เล็ก/บลูทูธ)'), h('option', { value: '80', selected: p.width === 80 }, '80 มม.'));
    const style = h('select', null,
      h('option', { value: 'image', selected: p.style === 'image' }, 'รูปภาพ (แนะนำ ภาษาไทยถูกต้องทุกเครื่อง)'),
      h('option', { value: 'text', selected: p.style === 'text' }, 'ข้อความ (เร็วกว่า ต้องตั้ง Code page ให้ตรงกับเครื่อง)'));
    const codepage = h('input', { value: String(p.codepage), inputmode: 'numeric', maxlength: 3, autocomplete: 'off' });
    const ticket = check('พิมพ์บิลลูกค้า (บัตรคิว + ใบเสร็จ) เมื่อชำระเงินแล้ว', p.ticket);
    const slip = check('พิมพ์ใบออเดอร์สำหรับร้าน (ตัวใหญ่ ไม่มีราคา) ต่อจากบิลลูกค้า', p.order_slip);
    const cut = check('ตัดกระดาษหลังพิมพ์ (ตัดแยกแต่ละใบ)', p.cut);

    const current = () => ({
      mode: mode.value,
      target: mode.value === 'printer' ? printer.value : mode.value === 'com' ? com.value.trim() : '',
      width: Number(width.value),
      codepage: Number.parseInt(codepage.value, 10),
      ticket: ticket.box.checked, cut: cut.box.checked,
      order_slip: slip.box.checked,
      head_dots: Number.parseInt(headDots.value || '0', 10), shift: Number.parseInt(shift.value || '0', 10), gap_secs: Number.parseInt(gap.value || '0', 10),
      style: style.value,
    });
    const redraw = () => { state.print = { ...state.print, ...current() }; render(); };
    mode.addEventListener('change', redraw);
    style.addEventListener('change', redraw);

    async function save() {
      const body = current();
      if (!Number.isInteger(body.codepage) || body.codepage < 0 || body.codepage > 255) throw new Error('Code page ต้องเป็นตัวเลข 0-255');
      if (![body.head_dots, body.shift, body.gap_secs].every(Number.isInteger)) throw new Error('ความกว้าง ตำแหน่ง และเวลาหน่วง ต้องเป็นตัวเลข');
      state.print = await api('PUT', '/api/print/settings', body);
    }
    const run = (btn, work) => async () => {
      btn.disabled = true;
      try { await work(); } catch (err) { toast(err.message); }
      btn.disabled = false;
    };
    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, 'บันทึก');
    saveBtn.addEventListener('click', run(saveBtn, async () => { await save(); toast('บันทึกแล้ว ใช้งานได้ทันที'); render(); }));
    const testBtn = (label, kind, doneText) => {
      const btn = h('button', { class: 'btn', type: 'button' }, label);
      btn.addEventListener('click', run(btn, async () => { await save(); await runTest(kind); toast(doneText); }));
      return btn;
    };
    const refresh = h('button', { class: 'btn small', type: 'button', disabled: state.targetsLoading }, state.targetsLoading ? 'กำลังค้นหา…' : 'ค้นหาเครื่องพิมพ์/พอร์ตใหม่');
    refresh.addEventListener('click', () => loadTargets(true));

    return [
      h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'เครื่องพิมพ์ใบเสร็จ'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ต่อเครื่องพิมพ์เข้ากับคอมพิวเตอร์ที่รันโปรแกรม POS นี้ (USB หรือ Bluetooth) ส่วน iPad หรือมือถือเป็นแค่หน้าจอสั่งพิมพ์ ใช้ได้ทุกเครื่อง'),
        field('วิธีเชื่อมต่อ', mode),
        p.mode === 'printer' ? field('เครื่องพิมพ์', printer, refresh) : null,
        p.mode === 'com' ? field('พอร์ต COM', com, refresh) : null,
        comList,
        p.mode !== 'off' ? field('ขนาดกระดาษ', width) : null,
        p.mode !== 'off' ? field('รูปแบบการพิมพ์', style, h('span', { class: 'muted' }, 'แบบรูปภาพใช้ฟอนต์ไทยวาดบิลเป็นภาพส่งไปพิมพ์ จึงไม่เพี้ยนไม่ว่าเครื่องพิมพ์รุ่นไหน')) : null,
        p.mode !== 'off' && p.style === 'image' ? h('details', { class: 'adv', open: p.shift !== 0 || p.head_dots !== 0 },
          h('summary', null, 'ตำแหน่งบนกระดาษ (ถ้าตัวอักษรไม่อยู่กึ่งกลาง)'),
          h('p', { class: 'muted', style: 'margin:6px 0 10px' }, 'ถ้าตัวอักษรเอียงไปด้านใดด้านหนึ่งหรือขอบขวาขาด แปลว่าความกว้างที่ตั้งไม่ตรงกับเครื่อง กด “ทดสอบตำแหน่งกระดาษ” จะพิมพ์ไม้บรรทัดมีเลขกำกับทุก 48 จุด ดูเลขสุดท้ายที่ยังอ่านออกครบ เอาเลขนั้น × 48 ใส่ในช่อง “ความกว้างที่พิมพ์ได้” เช่น เห็นถึงเลข 8 = 384 ถ้าเห็นถึง 12 = 576 แล้วกดบันทึก จากนั้นเส้นประกลางหน้าทดสอบควรตรงกลางกระดาษ ถ้ายังเยื้อง ปรับ “เลื่อนตำแหน่ง” (+ ไปขวา − ไปซ้าย)'),
          field('ความกว้างที่พิมพ์ได้ (จุด)', headDots, h('span', { class: 'muted' }, 'เว้นว่างให้ใช้ค่าอัตโนมัติ ช่วง 240-832')),
          field('เลื่อนตำแหน่งซ้าย/ขวา (จุด)', shift, h('span', { class: 'muted' }, '8 จุด ≈ 1 มม. ช่วง −120 ถึง 120'))) : null,
        p.mode !== 'off' && p.style === 'text' ? field('Code page ภาษาไทย', codepage, h('span', { class: 'muted' }, 'ถ้าพิมพ์ภาษาไทยเป็นตัวเพี้ยน กด “พิมพ์หน้าทดสอบ Code page” แล้วเลือกเลขที่บรรทัดนั้นอ่านออก')) : null,
        p.mode === 'com' ? h('p', { class: 'muted', style: 'margin:0' }, 'หาเลขพอร์ตของ Bluetooth: Windows > Bluetooth และอุปกรณ์อื่น ๆ > ตัวเลือก Bluetooth เพิ่มเติม > แท็บ COM Ports (เลือกแบบ Outgoing)') : null,
        stationBox),
      p.mode !== 'off' ? h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:8px' }, 'พิมพ์เมื่อไหร่'),
        ticket.row, slip.row,
        cut.row,
        field('หน่วงเวลาระหว่างใบ (วินาที)', gap, h('span', { class: 'muted' }, 'รอหลังพิมพ์แต่ละใบ ก่อนพิมพ์ใบถัดไป (บัตรคิว → ใบออเดอร์ → ใบเสร็จ) จะได้มีเวลาดึงกระดาษ ใส่ 0 ถ้าไม่ต้องการ')),
        h('p', { class: 'muted', style: 'margin:6px 0 0;font-size:14px' }, 'แก้ชื่อร้าน หัวบิล ท้ายบิล และรูปแบบเลขคิวได้ที่แท็บ “บิลและเลขคิว”')) : null,
      h('div', { class: 'erow' }, saveBtn,
        ...(p.mode !== 'off' ? [testBtn('ทดสอบบิลลูกค้า', 'ticket', 'ส่งบิลตัวอย่างแล้ว'), testBtn('ทดสอบใบออเดอร์ร้าน', 'slip', 'ส่งใบออเดอร์ตัวอย่างแล้ว'), ...(p.style === 'image' ? [testBtn('ทดสอบตำแหน่งกระดาษ', 'position', 'ส่งหน้าทดสอบตำแหน่งแล้ว')] : []), ...(p.style === 'text' ? [testBtn('พิมพ์หน้าทดสอบ Code page', 'codepages', 'ส่งหน้าทดสอบ Code page แล้ว')] : [])] : [])),
      h('p', { class: 'muted', style: 'font-size:14px' }, 'ปุ่มทดสอบจะบันทึกค่าก่อนแล้วสั่งพิมพ์ “ส่งแล้ว” แปลว่าส่งงานเข้าคิวของเครื่องพิมพ์สำเร็จ ถ้ากระดาษไม่ออก ให้เช็กสายหรือการจับคู่ Bluetooth'),
    ];
  }

  async function init() {
    render();
    try { await loadProducts(); } catch (err) { toast(err.message); }
    try { state.settings = await api('GET', '/api/settings'); } catch (err) { toast(err.message); }
    try { state.print = await api('GET', '/api/print/settings'); } catch (err) { toast(err.message); }
    try { state.bill = await api('GET', '/api/bill/settings'); } catch (err) { toast(err.message); }
    if (state.tab === 'print') loadTargets(false);
    render();
  }

  window.addEventListener('hashchange', () => { state.tab = tabFromHash(); if (state.tab === 'print') loadTargets(false); render(); });
  $('#editmodal').addEventListener('click', (e) => { if (e.target.id === 'editmodal') e.target.classList.remove('open'); });
  init();
})();
