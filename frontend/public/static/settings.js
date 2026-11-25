(function () {
  const { $, h, baht, api, upload, productPic, toast } = PM;
  const IMAGE_SIDE = 640;
  const TAB_KEYS = ['menu', 'pay', 'bill', 'print', 'staff', 'reset'];
  const tabFromHash = () => (TAB_KEYS.includes(location.hash.slice(1)) ? location.hash.slice(1) : 'menu');
  const state = { tab: tabFromHash(), products: [], settings: null, testMsg: null, selecting: false, selected: new Set(), print: null, bill: null, billKind: 'ticket', targets: null, targetsLoading: false, bot: null, botChecking: false };

  // A test sheet is printed by the server, or (cloud server, web mode) by this browser.
  async function runTest(kind) {
    const res = await api('POST', '/api/print/test', { kind });
    if (res && res.web) {
      if (!window.Station) throw new Error('หน้านี้ยังไม่พร้อมพิมพ์ผ่านเบราว์เซอร์');
      await Station.printBytes(Station.fromBase64(res.bytes));
    }
  }

  const TABS = [['menu', 'เมนูสินค้า'], ['pay', 'การรับเงิน'], ['bill', 'บิลและเลขคิว'], ['print', 'เครื่องพิมพ์'], ['staff', 'พนักงาน'], ['reset', 'ข้อมูล']];
  const ID_TYPES = { phone: 'เบอร์โทรศัพท์', national_id: 'เลขบัตรประชาชน', ewallet: 'e-Wallet' };

  function setTab(tab) {
    state.tab = tab;
    history.replaceState(null, '', '#' + tab);
    if (tab === 'print') loadTargets(false);
    if (tab === 'staff') loadUsers();
    if (tab === 'reset') loadBackups();
    watchBot(tab === 'pay' && !!state.settings);
    render();
  }

  function renderTabs() {
    $('#tabs').replaceChildren(...TABS.map(([key, label]) =>
      h('button', { class: 'chip' + (key === state.tab ? ' on' : ''), type: 'button', role: 'tab', 'aria-selected': key === state.tab, onclick: () => setTab(key) }, label)));
  }

  function render() {
    renderTabs();
    const views = { menu: renderMenu, pay: renderPay, bill: renderBill, print: renderPrint, staff: renderStaff, reset: renderReset };
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

  // The menu is a tree: a group holds items and other groups. `depth` indents the rows.
  function productRow(p, depth) {
    const swatch = h('span', { class: 'swatch', style: p.color ? 'background:' + p.color : null, title: 'สีการ์ด ' + (p.color || '') });
    const sub = p.is_group
      ? 'กลุ่ม · ' + PM.menuCount(state.products, p.id) + ' รายการ'
      : (depth ? 'อยู่ในกลุ่ม' : p.category || 'ไม่มีหมวด');
    const info = [
      swatch,
      h('div', { class: 'pthumb' }, productPic(p, true) || h('span', { class: 'im ph' }, '')),
      h('div', { class: 'pinfo' },
        h('div', { class: 'pname' }, (p.is_group ? '📁 ' : '') + p.name),
        h('div', { class: 'muted' }, sub)),
      p.is_group ? null : h('b', { class: 'num' }, baht(p.price_satang)),
    ];
    const indent = depth ? 'margin-left:' + depth * 20 + 'px' : null;
    if (!state.selecting) {
      const grip = h('button', { class: 'drag', type: 'button', title: 'ลากเพื่อเปลี่ยนตำแหน่ง (หรือกดลูกศรขึ้น/ลง)', 'aria-label': 'ลากเพื่อเปลี่ยนตำแหน่ง ' + p.name }, '⠿');
      return h('div', { class: 'prow' + (p.is_group ? ' grow' : ''), style: indent }, grip, ...info,
        p.is_group ? h('button', { class: 'btn small', type: 'button', onclick: () => openEditor(null, { parent: p.id }) }, '+ เพิ่มในกลุ่ม') : null,
        h('button', { class: 'btn small', type: 'button', onclick: () => openEditor(p) }, 'แก้ไข'));
    }
    const box = h('input', { type: 'checkbox', class: 'pcheck', 'aria-label': 'เลือก ' + p.name, checked: state.selected.has(p.id) });
    box.addEventListener('change', () => toggleSelected(p.id, box.checked));
    return h('label', { class: 'prow selectable', style: indent }, box, ...info);
  }

  // One list per group (and one for the top level): every item is a row, and for a group also
  // the list of what is inside it, so dragging a group moves it together with its contents.
  function treeRows(parent, depth) {
    const items = state.products.filter((p) => (p.parent_id ?? null) === parent);
    if (!items.length) return [];
    const list = h('div', { class: 'sortlist' }, ...items.map((p) =>
      h('div', { class: 'sortitem', 'data-id': String(p.id) }, productRow(p, depth), ...(p.is_group ? treeRows(p.id, depth + 1) : []))));
    if (!state.selecting) makeSortable(list, parent);
    return [list];
  }

  // Drag a row by its handle (mouse, finger or pen) to change its place among the rows next to it;
  // the arrow keys on the handle do the same. The new order is saved when the row is dropped.
  async function saveOrder(parent, list) {
    const ids = [...list.children].map((c) => Number(c.dataset.id));
    const before = state.products.filter((p) => (p.parent_id ?? null) === parent).map((p) => p.id);
    if (ids.every((id, i) => id === before[i])) return;
    try { await api('PUT', '/api/products/order', { parent_id: parent, ids }); } catch (err) { toast(err.message); }
    await loadProducts().catch(() => {});
    render();
  }

  function makeSortable(list, parent) {
    list.addEventListener('keydown', (e) => {
      const handle = e.target.closest && e.target.closest('.drag');
      if (!handle || !['ArrowUp', 'ArrowDown'].includes(e.key)) return;
      const item = handle.closest('.sortitem');
      if (!item || item.parentElement !== list) return;
      e.preventDefault();
      const other = e.key === 'ArrowUp' ? item.previousElementSibling : item.nextElementSibling;
      if (!other) return;
      if (e.key === 'ArrowUp') list.insertBefore(item, other); else list.insertBefore(other, item);
      handle.focus();
      clearTimeout(makeSortable.keyTimer);
      makeSortable.keyTimer = setTimeout(() => saveOrder(parent, list), 600);
    });
    list.addEventListener('pointerdown', (e) => {
      const handle = e.target.closest && e.target.closest('.drag');
      if (!handle) return;
      const item = handle.closest('.sortitem');
      if (!item || item.parentElement !== list) return;
      e.preventDefault();
      try { handle.setPointerCapture(e.pointerId); } catch (_) { /* pointer already gone: the window listeners still follow it */ }
      item.classList.add('dragging');
      // The held row itself is never taken out of the page (that would drop the finger/mouse
      // capture); the rows it passes are moved around it instead.
      const move = (ev) => {
        const y = ev.clientY;
        for (let guard = 0; guard < 100; guard++) {
          const prev = item.previousElementSibling;
          const next = item.nextElementSibling;
          const above = prev && prev.getBoundingClientRect();
          const below = next && next.getBoundingClientRect();
          if (prev && y < above.top + above.height / 2) list.insertBefore(prev, item.nextSibling);
          else if (next && y > below.top + below.height / 2) list.insertBefore(next, item);
          else break;
        }
        // Near the top or bottom of the screen: keep the page moving while the row is held there.
        if (y < 90) window.scrollBy(0, -14); else if (y > window.innerHeight - 90) window.scrollBy(0, 14);
      };
      let finished = false;
      const drop = () => {
        if (finished) return;
        finished = true;
        for (const type of ['pointermove']) { handle.removeEventListener(type, move); window.removeEventListener(type, move); }
        for (const type of ['pointerup', 'pointercancel']) { handle.removeEventListener(type, drop); window.removeEventListener(type, drop); }
        item.classList.remove('dragging');
        saveOrder(parent, list);
      };
      handle.addEventListener('pointermove', move);
      window.addEventListener('pointermove', move);
      for (const type of ['pointerup', 'pointercancel']) { handle.addEventListener(type, drop); window.addEventListener(type, drop); }
    });
  }

  function menuToolbar() {
    const total = state.products.length;
    if (!state.selecting) {
      return h('div', { class: 'total', style: 'margin-bottom:10px;flex-wrap:wrap' },
        h('h2', { style: 'font-size:18px' }, 'เมนูสินค้า (' + total + ')'),
        h('div', { class: 'erow', style: 'margin:0' },
          h('button', { class: 'btn small', type: 'button', disabled: !total, onclick: () => setSelecting(true) }, 'เลือกลบ'),
          h('button', { class: 'btn small danger', type: 'button', disabled: !total, onclick: deleteAll }, 'ลบทั้งหมด'),
          h('button', { class: 'btn', type: 'button', onclick: () => openEditor(null, { group: true }) }, '+ เพิ่มกลุ่ม'),
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
    const rows = treeRows(null, 0);
    return [
      menuToolbar(),
      h('div', { class: 'card' }, ...(rows.length ? rows : [h('p', { class: 'muted', style: 'text-align:center;padding:16px 0' }, 'ยังไม่มีสินค้า กด “เพิ่มสินค้า”')])),
      anyPic || !rows.length || state.selecting ? null : h('p', { class: 'muted', style: 'margin-top:10px' }, 'เคล็ดลับ: ใส่รูปสินค้าได้ที่ปุ่ม “แก้ไข” รูปจะแสดงบนหน้าขายและหน้าสั่งของลูกค้า'),
      state.selecting || !rows.length ? null : h('p', { class: 'muted', style: 'margin-top:6px' }, 'จัดเป็นกลุ่มได้: กด “เพิ่มกลุ่ม” เช่น ชาไทย แล้วเพิ่มเมนูข้างใน (เย็น, หวานน้อย …) หน้าขายจะกดเปิดกลุ่มเพื่อเลือก การ์ดแต่ละใบมีสีของตัวเอง เลือกสีได้ตอนแก้ไข และสีจะไม่ซ้ำกันในกลุ่มเดียวกัน'),
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

  // Card colours: keep in step with PALETTE in backend/src/menu.rs.
  const PALETTE = ['#E5484D', '#F76B15', '#FFB224', '#F5D90A', '#99D52A', '#46A758', '#12A594', '#05A2C2',
    '#0091FF', '#3E63DD', '#6E56CF', '#8E4EC6', '#D6409F', '#E93D82', '#AD7F58', '#7D8590',
    '#C2410C', '#0E7490', '#15803D', '#A16207', '#BE185D', '#4338CA', '#9F1239', '#1F2937'];

  // This group and everything inside it: none of them can be chosen as its own parent.
  function inside(groupId) {
    const ids = new Set([groupId]);
    for (let grew = true; grew;) {
      grew = false;
      for (const x of state.products) if (x.parent_id != null && ids.has(x.parent_id) && !ids.has(x.id)) { ids.add(x.id); grew = true; }
    }
    return ids;
  }

  // Groups to pick from, drawn as an indented list.
  function groupOptions(skip, depth, parent) {
    return state.products.filter((x) => x.is_group && (x.parent_id ?? null) === parent && !skip.has(x.id))
      .flatMap((x) => [{ id: x.id, label: '　'.repeat(depth) + x.name }, ...groupOptions(skip, depth + 1, x.id)]);
  }

  // `opts.group` makes a new group, `opts.parent` puts a new row inside that group.
  function openEditor(p, opts = {}) {
    const isGroup = p ? !!p.is_group : !!opts.group;
    const edit = { blob: null, previewUrl: null, removeImage: false, busy: false, parent: p ? (p.parent_id ?? null) : (opts.parent ?? null), color: p ? p.color : null };
    const field = (label, input) => h('label', { class: 'field' }, h('span', null, label), input);
    const name = h('input', { value: p ? p.name : '', maxlength: 60, autocomplete: 'off', placeholder: isGroup ? 'เช่น ชาไทย' : 'เช่น ลาเต้เย็น' });
    const cats = [...new Set(state.products.filter((x) => x.parent_id == null).map((x) => x.category).filter(Boolean))];
    const category = h('input', { value: p ? p.category : '', maxlength: 30, list: 'catlist', autocomplete: 'off', placeholder: 'เช่น กาแฟ' });
    const price = h('input', { value: p ? String(p.price_satang / 100) : '', inputmode: 'decimal', autocomplete: 'off', placeholder: 'เช่น 45' });

    // Which group it sits in; the top-level category only applies at the top.
    const skip = p ? inside(p.id) : new Set();
    const parent = h('select', null, h('option', { value: '' }, '— ระดับบนสุด —'),
      ...groupOptions(skip, 0, null).map((g) => h('option', { value: String(g.id), selected: g.id === edit.parent }, g.label)));
    const categoryField = field('หมวด (ปุ่มกรองด้านบนของเมนู)', category);

    // Colours used by the rows next to this one are taken; pick any other (or mix your own).
    const swatches = h('div', { class: 'swatches', role: 'radiogroup', 'aria-label': 'สีการ์ด' });
    const customColor = h('input', { type: 'color', class: 'colorpick', 'aria-label': 'เลือกสีเอง' });
    const usedByOthers = () => new Map(state.products
      .filter((x) => (x.parent_id ?? null) === edit.parent && (!p || x.id !== p.id) && x.color)
      .map((x) => [x.color.toUpperCase(), x.name]));
    function paintColors() {
      const used = usedByOthers();
      if (!edit.color || used.has(edit.color.toUpperCase())) edit.color = PALETTE.find((c) => !used.has(c)) || '#3E63DD';
      customColor.value = edit.color.toLowerCase();
      swatches.replaceChildren(...PALETTE.map((c) => {
        const taken = used.get(c);
        return h('button', { class: 'sw' + (c === edit.color.toUpperCase() ? ' on' : ''), type: 'button', role: 'radio', 'aria-checked': c === edit.color.toUpperCase(),
          style: 'background:' + c, disabled: !!taken, title: taken ? 'ใช้กับ “' + taken + '” แล้ว' : c,
          onclick: () => { edit.color = c; paintColors(); } }, c === edit.color.toUpperCase() ? '✓' : taken ? '×' : '');
      }));
    }
    customColor.addEventListener('input', () => {
      const taken = usedByOthers().get(customColor.value.toUpperCase());
      if (taken) { toast('สีนี้ใช้กับ “' + taken + '” แล้ว เลือกสีอื่น'); customColor.value = edit.color.toLowerCase(); return; }
      edit.color = customColor.value.toUpperCase();
      paintColors();
    });
    const syncParent = () => {
      edit.parent = parent.value === '' ? null : Number(parent.value);
      categoryField.hidden = edit.parent != null;
      paintColors();
    };
    parent.addEventListener('change', syncParent);
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
      const satang = isGroup ? 0 : parsePrice(price.value);
      if (!name.value.trim()) { toast(isGroup ? 'กรอกชื่อกลุ่ม' : 'กรอกชื่อสินค้า'); return; }
      if (satang == null) { toast('กรอกราคาเป็นตัวเลขมากกว่า 0'); return; }
      edit.busy = true; saveBtn.disabled = true;
      try {
        const body = { name: name.value, category: edit.parent == null ? category.value : '', price_satang: satang, parent_id: edit.parent, is_group: isGroup, color: edit.color };
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
        const what = isGroup ? 'ลบกลุ่ม “' + p.name + '” และรายการข้างใน ' + PM.menuCount(state.products, p.id) + ' รายการ ออกจากเมนู?' : 'ลบ “' + p.name + '” ออกจากเมนู?';
        if (!(await PM.confirm(what + '\nออเดอร์เก่ายังอยู่ตามเดิม', { title: isGroup ? 'ลบกลุ่ม' : 'ลบสินค้า', okText: isGroup ? 'ลบกลุ่ม' : 'ลบสินค้า' }))) return;
        try { await api('DELETE', '/api/products/' + p.id); close(); await loadProducts(); render(); toast('ลบแล้ว'); } catch (err) { toast(err.message); }
      } }, isGroup ? 'ลบกลุ่ม' : 'ลบสินค้า'));
    }
    actions.push(h('button', { class: 'btn block', type: 'button', onclick: close }, 'ยกเลิก'));

    $('#editbox').replaceChildren(
      h('h3', { style: 'margin-bottom:10px' }, (p ? 'แก้ไข' : 'เพิ่ม') + (isGroup ? 'กลุ่ม' : 'สินค้า')),
      preview,
      h('div', { class: 'erow' },
        h('button', { class: 'btn small', type: 'button', onclick: () => file.click() }, 'เลือกรูป / ถ่ายรูป'),
        removeBtn, file),
      h('datalist', { id: 'catlist' }, cats.map((c) => h('option', { value: c }))),
      field(isGroup ? 'ชื่อกลุ่ม' : 'ชื่อสินค้า', name),
      field('อยู่ในกลุ่ม', parent),
      categoryField,
      isGroup ? null : field('ราคา (บาท)', price),
      h('div', { class: 'field' }, h('span', null, 'สีการ์ด (ไม่ซ้ำกันในกลุ่มเดียวกัน)'), swatches,
        h('label', { class: 'erow', style: 'margin:6px 0 0;gap:8px;align-items:center' }, customColor, h('span', { class: 'muted' }, 'หรือผสมสีเอง'))),
      h('div', { class: 'btns' }, ...actions));
    paint();
    syncParent();
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

  // The bot that reads the bank's notifications: checked when the tab opens and every half minute.
  const BOT_POLL_MS = 30000;
  const BOT_BADGE = { online: ['ok', 'บอททำงานอยู่'], unknown: ['warn', 'บอทเปิดอยู่ (ไม่ทราบสถานะ)'], offline: ['bad', 'บอทไม่ทำงาน'], error: ['bad', 'เช็กบอทไม่ได้'], not_configured: ['warn', 'ยังไม่ได้ตั้งค่า'] };
  let botEl = null;
  let botTimer = null;

  function paintBot() {
    if (!botEl || !botEl.isConnected) return;
    const b = state.bot;
    const [kind, label] = state.botChecking && !b ? ['warn', 'กำลังเช็กบอท…'] : b ? (BOT_BADGE[b.state] || ['warn', b.state]) : ['warn', 'ยังไม่ได้เช็ก'];
    botEl.replaceChildren(
      h('span', { class: 'badge ' + kind }, label),
      b && b.message ? h('span', { class: 'muted', style: 'display:block;margin-top:4px;white-space:pre-line' }, b.message) : null);
  }

  async function checkBot() {
    if (state.botChecking) return;
    state.botChecking = true;
    paintBot();
    try { state.bot = await api('GET', '/api/settings/bot'); } catch (err) { state.bot = { state: 'error', ok: false, message: err.message }; }
    state.botChecking = false;
    paintBot();
  }

  function watchBot(on) {
    clearInterval(botTimer);
    botTimer = null;
    if (!on) return;
    checkBot();
    botTimer = setInterval(() => { if (document.visibilityState === 'visible') checkBot(); }, BOT_POLL_MS);
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
    botEl = h('div', { class: 'botstate', role: 'status', 'aria-live': 'polite', style: 'margin:0 0 12px' });
    paintBot();
    const botBtn = h('button', { class: 'btn small', type: 'button', style: 'margin-left:8px' }, 'เช็กบอทตอนนี้');
    botBtn.addEventListener('click', checkBot);
    // More than one bot in the API: list them with their chats and fill the two IDs from a tap.
    const pickBox = h('div', { class: 'routebox', style: 'margin:8px 0' });
    const showRoutes = (res) => {
      if (!res.ok) { pickBox.replaceChildren(h('p', { class: 'warnbox' }, res.message)); return; }
      if (!res.bots.length) { pickBox.replaceChildren(h('p', { class: 'warnbox' }, 'API ยังไม่มีบอทเลย สร้างและเปิดบอทในระบบ API ก่อน')); return; }
      const use = (b, t) => () => {
        bot.value = b.id;
        target.value = t ? t.id : '';
        toast('ใส่ค่าแล้ว กด “บันทึก” ด้านล่างเพื่อใช้งาน');
      };
      pickBox.replaceChildren(...res.bots.map((b) => h('div', { class: 'card', style: 'margin:0 0 8px;padding:12px' },
        h('b', null, (b.name || b.id) + (b.enabled ? '' : ' (ปิดอยู่)')),
        h('div', { class: 'muted', style: 'font-size:14px;overflow-wrap:anywhere' }, 'สถานะ ' + (b.status || 'ไม่ทราบ') + ' · ' + b.id),
        ...b.targets.map((t) => h('div', { class: 'erow', style: 'margin:6px 0 0;align-items:center' },
          h('span', { style: 'flex:1;overflow-wrap:anywhere' }, 'แหล่งแจ้งเงินเข้า: ' + (t.name || t.id) + (t.enabled ? '' : ' (ปิดอยู่)')),
          h('button', { class: 'btn small', type: 'button', disabled: !b.enabled || !t.enabled, onclick: use(b, t) }, 'ใช้ตัวนี้'))),
        b.targets.length ? null : h('button', { class: 'btn small', type: 'button', style: 'margin-top:6px', disabled: !b.enabled, onclick: use(b, null) }, 'ใช้บอทนี้'))));
    };
    const routesBtn = h('button', { class: 'btn small', type: 'button' }, 'ดึงรายชื่อบอทจาก API');
    routesBtn.addEventListener('click', async () => {
      routesBtn.disabled = true;
      pickBox.replaceChildren(h('p', { class: 'muted' }, 'กำลังดึงรายชื่อ…'));
      try { showRoutes(await api('GET', '/api/settings/routes')); } catch (err) { showRoutes({ ok: false, message: err.message }); }
      routesBtn.disabled = false;
    });
    const mustChoose = !!(state.bot && /ระบุเอง|Bot ID/.test(state.bot.message || ''));
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
        state.bot = null;
        render();
        checkBot();
      } catch (err) { toast(err.message); saveBtn.disabled = false; }
    });
    testBtn.addEventListener('click', async () => {
      testBtn.disabled = true;
      try { state.testMsg = await api('POST', '/api/settings/test'); } catch (err) { state.testMsg = { ok: false, message: err.message }; }
      render();
      checkBot();
    });

    return [
      h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'เลขพร้อมเพย์ที่รับเงิน'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ใช้สร้าง QR พร้อมเพย์ให้ลูกค้าสแกนจ่าย พร้อมยอดเงินของแต่ละออเดอร์'),
        field('เลขพร้อมเพย์', ppId, ppHint)),
      h('div', { class: 'card' },
        h('div', { class: 'total', style: 'margin-bottom:6px' }, h('h2', { style: 'font-size:18px' }, 'ระบบตรวจยอดเงินเข้า'), status),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'เมื่อกรอกครบ ระบบจะตรวจว่าลูกค้าจ่ายแล้วให้อัตโนมัติ แล้วพูดจำนวนเงินเป็นภาษาไทย ถ้ายังไม่กรอก ให้แคชเชียร์กด “ยืนยันรับเงินแล้ว” เอง'),
        h('div', { style: 'display:flex;align-items:center;flex-wrap:wrap;margin:0 0 4px' }, h('b', null, 'สถานะบอทตรวจยอด'), botBtn),
        botEl,
        gaps.length && !s.verify_active ? h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ยังขาด: ' + gaps.join(', ')) : null,
        field('ที่อยู่ API', base),
        field('API key', key, h('span', { class: 'muted' }, 'เก็บในฐานข้อมูลของเครื่องนี้ ไม่ส่งกลับมาแสดงในหน้านี้')),
        field('เลข 4 ตัวท้ายบัญชีปลายทาง', last4, h('span', { class: 'muted' }, 'ไม่บังคับ ใส่เมื่อ API ของคุณต้องการ')),
        h('details', { class: 'adv', open: mustChoose },
          h('summary', null, 'ขั้นสูง: Bot ID / Source target ID (ปกติเว้นว่างได้)'),
          h('p', { class: 'muted', style: 'margin:6px 0 10px' }, 'API ต้องใช้สองค่านี้ ถ้าเว้นว่าง ระบบจะหาให้เองจาก API key เมื่อมีบอทที่เปิดใช้งาน 1 ตัว และแหล่งแจ้งเงินเข้าที่เปิดใช้งาน 1 อัน ถ้ามีหลายบอท (เช่น แยกตามธนาคาร) ต้องเลือกตัวที่ใช้รับเงินของร้านนี้'),
          mustChoose ? h('p', { class: 'warnbox' }, 'ตอนนี้ API มีบอทที่เปิดใช้งานหลายตัว ระบบเลือกเองไม่ได้ จึงตรวจยอดอัตโนมัติไม่ได้ กด “ดึงรายชื่อบอทจาก API” แล้วเลือกบอทของบัญชีที่รับเงิน (เช่น KBank) จากนั้นกด “บันทึก”') : null,
          h('div', { style: 'margin:0 0 6px' }, routesBtn),
          pickBox,
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
      display_name: f.display_name.value,
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
      display_name: text(b.display_name, 40, b.shop_default),
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
          field('ชื่อที่โชว์บนจอลูกค้า', f.display_name, h('span', { class: 'muted' }, 'ชื่อที่หัวจอลูกค้าและหน้าจอรอ ตั้งเองได้ไม่ต้องตรงกับชื่อร้านจริง เว้นว่างจะใช้ “' + b.shop_default + '” (เปิดหน้าจอลูกค้าใหม่หรือรีเฟรชเพื่อเห็นชื่อใหม่)')),
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
    const names = { off: 'ปิดอยู่', noport: 'ยังไม่ได้เชื่อมต่อเครื่องพิมพ์', ready: 'พร้อมพิมพ์', printing: 'กำลังพิมพ์', error: 'ผิดพลาด', nomode: 'ตั้งโหมดพิมพ์ผ่านเบราว์เซอร์ก่อน' };
    const paint = () => { status.textContent = (names[S.status] || S.status) + (S.label ? ' ' + S.label : '') + (S.message ? ' · ' + S.message : ''); };
    paint();
    if (stopStationWatch) stopStationWatch();
    stopStationWatch = S.onChange(paint);

    const baud = h('select', null, ...S.bauds.map((b) => h('option', { value: String(b), selected: c.baud === b }, String(b))));
    baud.addEventListener('change', () => S.setBaud(Number(baud.value)));
    const speed = h('select', null, ...Object.entries(S.speeds).map(([key, label]) => h('option', { value: key, selected: (c.speed || 'slow') === key }, label)));
    speed.addEventListener('change', () => S.setSpeed(speed.value));
    const on = h('input', { type: 'checkbox', class: 'pcheck', checked: c.on });
    on.addEventListener('change', () => S.setOn(on.checked));
    const connect = (kind, label, ok) => {
      const btn = h('button', { class: 'btn', type: 'button', disabled: !ok }, label);
      btn.addEventListener('click', async () => {
        try { await S.connect(kind); toast('เลือกเครื่องพิมพ์แล้ว'); } catch (err) { toast(err.message); }
      });
      return btn;
    };
    const unsupported = !S.supported.serial && !S.supported.usb && !S.supported.ble;
    const phone = !S.supported.serial && S.supported.ble;
    return h('div', { class: 'stationbox' },
      h('p', { class: 'muted', style: 'margin:0 0 8px' }, 'เซิร์ฟเวอร์อยู่บนคลาวด์ จึงสั่งเครื่องพิมพ์โดยตรงไม่ได้ ให้เปิดหน้า “ขาย” ในเบราว์เซอร์ของเครื่องที่ต่อเครื่องพิมพ์ (Chrome หรือ Edge บนคอมพิวเตอร์ หรือ Chrome บนมือถือ/แท็บเล็ต Android) แล้วเชื่อมต่อด้านล่าง หน้านั้นจะรับคิวพิมพ์มาสั่งเครื่องเอง ต้องเปิดค้างไว้ตอนขาย'),
      unsupported ? h('p', { class: 'warnbox' }, 'เบราว์เซอร์นี้เชื่อมต่อเครื่องพิมพ์ไม่ได้ (iPhone/iPad และ Safari/Firefox ใช้ไม่ได้) ใช้ Chrome หรือ Edge บนคอมพิวเตอร์ หรือ Chrome บน Android') : null,
      phone ? h('p', { class: 'okhint' }, 'มือถือ/แท็บเล็ต Android: กด “เชื่อมต่อ Bluetooth LE” แล้วเลือกเครื่องพิมพ์ในรายการที่ค้นเจอ (เปิดเครื่องพิมพ์และวางใกล้มือถือ ไม่ต้องจับคู่ในตั้งค่า Bluetooth ของมือถือก่อน)') : null,
      h('div', { class: 'erow' },
        connect('ble', 'เชื่อมต่อ Bluetooth LE (มือถือ Android)', S.supported.ble),
        connect('serial', 'เชื่อมต่อ Bluetooth / COM (คอมพิวเตอร์)', S.supported.serial),
        connect('usb', 'เชื่อมต่อ USB', S.supported.usb)),
      S.supported.ble ? h('details', { class: 'more', style: 'margin:6px 0 10px' }, h('summary', null, 'ค้นไม่เจอเครื่องพิมพ์?'),
        h('div', { class: 'okhint', style: 'margin-top:6px' },
          '1) เปิดเครื่องพิมพ์ให้ไฟ Bluetooth กะพริบ และอยู่ใกล้มือถือ ',
          h('br'), '2) ปิดการเชื่อมต่อของเครื่องพิมพ์กับมือถือเครื่องอื่นหรือแอปอื่นก่อน เพราะเครื่องพิมพ์ LE ต่อได้ทีละเครื่อง ',
          h('br'), '3) เปิด Bluetooth และตำแหน่งที่ตั้ง (Location) ของมือถือ Android ต้องเปิดไว้ถึงจะค้นอุปกรณ์ได้ ',
          h('br'), '4) ถ้าค้นแล้วไม่มีชื่อเครื่องพิมพ์เลย เครื่องรุ่นนั้นน่าจะเป็น Bluetooth แบบคลาสสิก (SPP) ซึ่งเว็บบนมือถือเชื่อมต่อไม่ได้ ให้ใช้สาย USB-OTG หรือเครื่องพิมพ์ที่รองรับ Bluetooth LE'))
        : null,
      field('ความเร็วส่งข้อมูลไปเครื่องพิมพ์', speed, h('span', { class: 'muted' }, 'ถ้าพิมพ์แล้วมีตัวอักษรมั่ว ๆ ปนกลางบิล ให้เลือก “ช้า” เครื่องพิมพ์ Bluetooth ราคาประหยัดรับข้อมูลเร็วเกินไปไม่ไหว')),
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

  // ------------------------------------------------------------- staff tab

  const ROLE_NAME = { admin: 'แอดมิน (ทำได้ทุกอย่าง)', cashier: 'แคชเชียร์ (ขาย รับเงิน เรียกคิว)' };

  async function loadUsers() {
    try { state.users = await api('GET', '/api/users'); } catch (err) { toast(err.message); state.users = state.users || []; }
    if (state.tab === 'staff') render();
  }

  function openStaffEditor(u) {
    const field = (label, input) => h('label', { class: 'field' }, h('span', null, label), input);
    const role = h('select', null, ...Object.entries(ROLE_NAME).map(([v, label]) => h('option', { value: v, selected: u.role === v }, label)));
    const active = h('input', { type: 'checkbox', class: 'pcheck', checked: u.active });
    const password = h('input', { type: 'password', autocomplete: 'new-password', placeholder: 'เว้นว่าง = ไม่เปลี่ยน (อย่างน้อย 8 ตัว)' });
    const close = () => $('#editmodal').classList.remove('open');
    const save = h('button', { class: 'btn primary block', type: 'button' }, 'บันทึก');
    save.addEventListener('click', async () => {
      save.disabled = true;
      try {
        const body = { role: role.value, active: active.checked };
        if (password.value) body.password = password.value;
        await api('PUT', '/api/users/' + u.id, body);
        close(); toast('บันทึกแล้ว'); await loadUsers();
      } catch (err) { toast(err.message); save.disabled = false; }
    });
    const del = h('button', { class: 'btn danger block', type: 'button' }, 'ลบบัญชีนี้');
    del.addEventListener('click', async () => {
      if (!(await PM.confirm('ลบบัญชี “' + u.username + '” ถาวร?\nประวัติที่เขาทำไว้ในบันทึกการทำงานยังอยู่', { title: 'ลบพนักงาน', okText: 'ลบบัญชี' }))) return;
      try { await api('DELETE', '/api/users/' + u.id); close(); toast('ลบแล้ว'); await loadUsers(); } catch (err) { toast(err.message); }
    });
    $('#editbox').replaceChildren(
      h('h3', { style: 'margin-bottom:10px' }, 'แก้ไขบัญชี ' + u.username),
      field('ตำแหน่ง', role),
      h('label', { class: 'checkrow' }, active, h('span', null, 'เปิดใช้งานบัญชีนี้')),
      field('รหัสผ่านใหม่', password),
      h('div', { class: 'btns' }, save, del, h('button', { class: 'btn block', type: 'button', onclick: close }, 'ยกเลิก')));
    $('#editmodal').classList.add('open');
  }

  function renderStaff() {
    const field = (label, input) => h('label', { class: 'field' }, h('span', null, label), input);
    const name = h('input', { autocomplete: 'off', maxlength: 30, placeholder: 'เช่น somchai (a-z 0-9 . _ -)' });
    const password = h('input', { type: 'password', autocomplete: 'new-password', placeholder: 'อย่างน้อย 8 ตัว' });
    const role = h('select', null, ...Object.entries(ROLE_NAME).reverse().map(([v, label]) => h('option', { value: v }, label)));
    const add = h('button', { class: 'btn primary', type: 'button' }, '+ เพิ่มพนักงาน');
    add.addEventListener('click', async () => {
      add.disabled = true;
      try {
        await api('POST', '/api/users', { username: name.value, password: password.value, role: role.value });
        name.value = ''; password.value = ''; toast('เพิ่มพนักงานแล้ว'); await loadUsers();
      } catch (err) { toast(err.message); }
      add.disabled = false;
    });
    const users = state.users || [];
    return [
      h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'เพิ่มพนักงาน'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'แคชเชียร์ขาย รับเงิน เรียกคิวได้ แต่เข้าหน้าจัดการ สรุปยอด และล้างยอดขายไม่ได้ ทุกการกดยืนยันเองหรือยกเลิกจะถูกบันทึกชื่อไว้'),
        field('ชื่อผู้ใช้', name), field('รหัสผ่าน', password), field('ตำแหน่ง', role), add),
      h('div', { class: 'card' },
        h('h2', { style: 'font-size:18px;margin-bottom:8px' }, 'บัญชีทั้งหมด (' + users.length + ')'),
        ...users.map((u) => h('div', { class: 'prow' },
          h('div', { class: 'pinfo' }, h('div', { class: 'pname' }, u.username), h('div', { class: 'muted' }, (ROLE_NAME[u.role] || u.role) + (u.active ? '' : ' · ปิดใช้งาน'))),
          h('button', { class: 'btn small', type: 'button', onclick: () => openStaffEditor(u) }, 'แก้ไข')))),
    ];
  }

  // -------------------------------------------------------------- reset tab

  // Deletes every order, payment and print job and starts the queue over; menu and settings stay.
  // A small box that asks for the admin's password; resolves to the text, or null when cancelled.
  function askPassword(title, message) {
    return new Promise((resolve) => {
      const input = h('input', { type: 'password', autocomplete: 'current-password', placeholder: 'รหัสผ่านแอดมิน' });
      const done = (value) => { $('#editmodal').classList.remove('open'); resolve(value); };
      const ok = h('button', { class: 'btn danger-fill block', type: 'button' }, 'ยืนยัน');
      ok.addEventListener('click', () => done(input.value || null));
      input.addEventListener('keydown', (e) => { if (e.key === 'Enter') ok.click(); });
      $('#editbox').replaceChildren(
        h('h3', { style: 'margin-bottom:8px' }, title),
        h('p', { class: 'muted', style: 'white-space:pre-line;margin:0 0 10px' }, message),
        h('label', { class: 'field' }, h('span', null, 'รหัสผ่านแอดมินของคุณ'), input),
        h('div', { class: 'btns' }, ok, h('button', { class: 'btn block', type: 'button', onclick: () => done(null) }, 'ยกเลิก')));
      $('#editmodal').classList.add('open');
      input.focus();
    });
  }

  async function resetSales() {
    const ask = 'ลบออเดอร์ การรับเงิน และคิวพิมพ์ทั้งหมด แล้วเริ่มนับคิวใหม่จากคิวแรก\nเมนูสินค้าและการตั้งค่าทั้งหมดยังอยู่ครบ\nย้อนกลับไม่ได้ ถ้าแค่จะจบวัน ใช้ “ปิดยอดวัน” ในหน้าสรุปยอดแทน เพราะเก็บรายงานไว้ให้';
    if (!(await PM.confirm(ask, { title: 'ล้างยอดขายและคิวทั้งหมด', okText: 'ไปขั้นต่อไป' }))) return;
    const password = await askPassword('ใส่รหัสผ่านเพื่อล้างยอดขาย', 'ปุ่มนี้ลบข้อมูลถาวร จึงต้องยืนยันด้วยรหัสผ่านของคุณอีกครั้ง');
    if (!password) return;
    try {
      const done = await api('POST', '/api/settings/reset-sales', { confirm: 'reset-sales', password });
      toast('ล้างแล้ว ' + done.orders + ' ออเดอร์ เริ่มคิวใหม่เรียบร้อย');
    } catch (err) { toast(err.message); }
  }

  // ---- backup: a copy of the whole database, downloaded now or kept by the server each night
  async function loadBackups() {
    try { state.backups = await api('GET', '/api/backup/files'); } catch (err) { state.backups = { files: [], keep: 0 }; toast(err.message); }
    if (state.tab === 'reset') render();
  }

  async function backupNow(btn) {
    btn.disabled = true;
    try {
      const res = await fetch('/api/backup', { method: 'POST', credentials: 'same-origin' });
      if (!res.ok) throw new Error((await res.json().catch(() => ({}))).error || 'สำรองข้อมูลไม่สำเร็จ');
      const name = (/filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') || '') || [])[1] || 'posmini-backup.db';
      const url = URL.createObjectURL(await res.blob());
      const link = h('a', { href: url, download: name });
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
      toast('ดาวน์โหลดไฟล์สำรองแล้ว');
      loadBackups();
    } catch (err) { toast(err.message); }
    btn.disabled = false;
  }

  const kb = (n) => (n >= 1048576 ? (n / 1048576).toFixed(1) + ' MB' : Math.max(1, Math.round(n / 1024)) + ' KB');

  function renderReset() {
    const b = state.backups;
    const backupBtn = h('button', { class: 'btn primary', type: 'button' }, 'สำรองและดาวน์โหลดตอนนี้');
    backupBtn.addEventListener('click', () => backupNow(backupBtn));
    return [
      h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'สำรองข้อมูล'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ไฟล์สำรองมีทุกอย่าง (เมนู ออเดอร์ สลิป บัญชี การตั้งค่า) เซิร์ฟเวอร์สำรองเองวันละครั้งและเก็บล่าสุด ' + (b ? b.keep : 14) + ' ไฟล์ แต่ไฟล์ที่เก็บในเครื่องเดียวกับฐานข้อมูลกันได้แค่พลาดเอง ถ้าเครื่องเสียจะหายด้วยกัน ควรดาวน์โหลดเก็บไว้ที่อื่นเป็นระยะ'),
        h('div', { class: 'erow' }, backupBtn),
        b && b.files.length ? h('div', { style: 'margin-top:10px' }, ...b.files.map((f) => h('div', { class: 'prow' },
          h('div', { class: 'pinfo' }, h('div', { class: 'pname num' }, f.at), h('div', { class: 'muted' }, f.name + ' · ' + kb(f.size))),
          h('a', { class: 'btn small', href: '/api/backup/files/' + encodeURIComponent(f.name), download: f.name }, 'ดาวน์โหลด')))) : h('p', { class: 'muted', style: 'margin:10px 0 0' }, b ? 'ยังไม่มีไฟล์สำรองในเซิร์ฟเวอร์' : 'กำลังโหลด…')),
      h('div', { class: 'card', style: 'margin-bottom:12px' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'ปิดยอดวัน'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'จบวันให้ปิดยอดแทนการลบ: ระบบสรุปยอดเงินสด ยอดโอน รายการที่กดยืนยันเอง ให้นับเงินสดในลิ้นชักเทียบ และเก็บรายงานไว้ดูย้อนหลัง'),
        h('a', { class: 'btn', href: '/dashboard' }, 'ไปหน้าสรุปยอดเพื่อปิดยอด')),
      h('div', { class: 'card' },
        h('h2', { style: 'font-size:18px;margin-bottom:4px' }, 'ล้างยอดขายและคิว'),
        h('p', { class: 'muted', style: 'margin:0 0 10px' }, 'ใช้เมื่อหลังทดลองระบบหรือเริ่มใหม่หมด ต้องใส่รหัสผ่านแอดมินยืนยัน ทุกเครื่องที่เปิดอยู่จะโหลดรายการใหม่ให้เอง'),
        h('p', { style: 'margin:0 0 4px' }, h('b', null, 'จะถูกลบ: '), 'ออเดอร์ทั้งหมด ประวัติรับเงิน คิวพิมพ์ และเลขคิว (เริ่มใหม่จากคิวแรก)'),
        h('p', { style: 'margin:0 0 14px' }, h('b', null, 'ยังอยู่: '), 'เมนูสินค้าและรูป การตั้งค่า บัญชีพนักงาน สลิปการโอน บันทึกการทำงาน และรายงานปิดยอด'),
        h('button', { class: 'btn danger-fill', type: 'button', onclick: resetSales }, 'ล้างยอดขายและคิวทั้งหมด')),
    ];
  }

  async function init() {
    render();
    try { await loadProducts(); } catch (err) { toast(err.message); }
    try { state.settings = await api('GET', '/api/settings'); } catch (err) { toast(err.message); }
    try { state.print = await api('GET', '/api/print/settings'); } catch (err) { toast(err.message); }
    try { state.bill = await api('GET', '/api/bill/settings'); } catch (err) { toast(err.message); }
    if (state.tab === 'print') loadTargets(false);
    if (state.tab === 'staff') loadUsers();
    if (state.tab === 'reset') loadBackups();
    watchBot(state.tab === 'pay');
    render();
  }

  window.addEventListener('hashchange', () => { state.tab = tabFromHash(); if (state.tab === 'print') loadTargets(false); if (state.tab === 'staff') loadUsers(); watchBot(state.tab === 'pay'); render(); });
  $('#editmodal').addEventListener('click', (e) => { if (e.target.id === 'editmodal') e.target.classList.remove('open'); });
  init();
})();
