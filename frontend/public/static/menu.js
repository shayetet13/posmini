(function () {
  const { $, h, baht, api, productPic, toast } = PM;
  const ALL = 'ทั้งหมด';
  const state = { products: [], path: [], cat: ALL, cart: new Map(), notes: new Map(), orderNote: '', busy: false, wait: null };

  const byId = (id) => state.products.find((p) => p.id === id);
  const lines = () => [...state.cart].map(([id, qty]) => ({ p: byId(id), qty })).filter((l) => l.p);
  const total = () => lines().reduce((s, l) => s + l.p.price_satang * l.qty, 0);
  const count = () => lines().reduce((s, l) => s + l.qty, 0);

  function change(id, delta) {
    if (delta > 0 && byId(id) && byId(id).sold_out) { toast('“' + byId(id).name + '” หมดแล้ว'); return; }
    state.wait = null; // the "please wait" notice was about the old cart
    const next = (state.cart.get(id) || 0) + delta;
    if (next <= 0) { state.cart.delete(id); state.notes.delete(id); } else state.cart.set(id, Math.min(next, 99));
    renderGrid(); renderBar();
    if ($('#cartmodal').classList.contains('open')) renderCart();
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
    // The customer's phone keeps the price on the card: they pay it.
    const tile = (p) => PM.menuTile(p, { products: state.products, qty: state.cart.get(p.id), showPrice: true, onAdd: (id) => change(id, 1), onOpen: (id) => goPath([...state.path, id]) });
    $('#grid').replaceChildren(...(list.length ? list.map(tile)
      : [h('p', { class: 'muted empty' }, state.path.length ? 'กลุ่มนี้ยังไม่มีรายการ' : 'ยังไม่มีสินค้า')]));
  }

  function renderBar() {
    $('#sum').textContent = baht(total());
    $('#count').textContent = count() + ' รายการ';
    $('#cartpill').hidden = count() === 0;
  }

  async function submit() {
    if (state.busy) return;
    if (!state.cart.size) { toast('ตะกร้าว่าง'); return; }
    state.busy = true;
    state.wait = null;
    try {
      const items = [...state.cart].map(([product_id, qty]) => ({ product_id, qty, note: state.notes.get(product_id) || '' }));
      const res = await api('POST', '/api/m/' + window.SLUG + '/orders', { items, note: state.orderNote });
      location.href = '/q/' + res.token;
    } catch (err) {
      // 409: another customer's QR is still waiting for its money; keep the cart and let them retry.
      if (err.status === 409) { state.wait = err.message; renderCart(); } else toast(err.message);
      state.busy = false;
    }
  }

  async function editNote(p) {
    const text = await PM.askText({ title: 'โน้ต ' + p.name, label: 'บอกร้านว่าอะไร', placeholder: 'เช่น ไม่หวาน ไม่ใส่น้ำแข็ง', value: state.notes.get(p.id) || '', max: 60, okText: 'บันทึก' });
    if (text == null) return;
    if (text) state.notes.set(p.id, text); else state.notes.delete(p.id);
    renderCart();
  }

  async function editOrderNote() {
    const text = await PM.askText({ title: 'โน้ตถึงร้าน', label: 'ข้อความถึงร้าน', placeholder: 'เช่น รีบหน่อย ห่อแยกถุง', value: state.orderNote, max: 100, okText: 'บันทึก' });
    if (text == null) return;
    state.orderNote = text;
    renderCart();
  }

  function renderCart() {
    const ls = lines();
    $('#cartbox').replaceChildren(...[
      h('h3', { style: 'margin-bottom:6px' }, 'ตะกร้าของคุณ'),
      h('div', { class: 'ticket' }, ...(ls.length ? ls.map(({ p, qty }) => h('div', { class: 'row' },
        h('span', { class: 'nm' }, p.name,
          h('button', { class: 'linenote' + (state.notes.get(p.id) ? ' has' : ''), type: 'button', 'aria-label': 'โน้ต ' + p.name, onclick: () => editNote(p) },
            state.notes.get(p.id) ? '✎ ' + state.notes.get(p.id) : '+ โน้ต')),
        h('span', { class: 'price num' }, baht(p.price_satang * qty)),
        h('div', { class: 'stepper' },
          h('button', { class: 'qb', type: 'button', 'aria-label': 'ลด ' + p.name, onclick: () => change(p.id, -1) }, '−'),
          h('span', { class: 'qty num' }, qty),
          h('button', { class: 'qb', type: 'button', 'aria-label': 'เพิ่ม ' + p.name, onclick: () => change(p.id, 1) }, '+'),
          h('button', { class: 'qb del', type: 'button', 'aria-label': 'ลบ ' + p.name + ' ออกจากตะกร้า', title: 'ลบรายการนี้', onclick: () => change(p.id, -99) }, PM.icon('trash', 16)))))
        : [h('p', { class: 'muted empty' }, 'ยังไม่ได้เลือกสินค้า')])),
      h('div', { class: 'perf' }),
      ls.length ? h('button', { class: 'chip small' + (state.orderNote ? ' on' : ''), type: 'button', style: 'margin-top:10px', onclick: editOrderNote }, state.orderNote ? '✎ ' + state.orderNote : '+ โน้ตถึงร้าน') : null,
      h('div', { class: 'total', style: 'margin-top:14px' }, h('span', { class: 'muted' }, 'รวมทั้งหมด'), h('b', { class: 'num' }, baht(total()))),
      state.wait ? h('p', { class: 'warnbox', role: 'alert', style: 'font-size:17px' }, state.wait) : null,
      h('div', { class: 'btns', style: 'display:grid;gap:8px;margin-top:14px' },
        h('button', { class: 'btn primary block lg', style: 'flex-direction:row', type: 'button', onclick: submit }, state.wait ? 'ลองใหม่' : 'ยืนยันและไปชำระเงิน'),
        h('button', { class: 'btn block', type: 'button', onclick: () => $('#cartmodal').classList.remove('open') }, 'เลือกสินค้าเพิ่ม'))].filter(Boolean));
  }

  $('#open').addEventListener('click', () => { renderCart(); $('#cartmodal').classList.add('open'); });
  $('#cartmodal').addEventListener('click', (e) => { if (e.target.id === 'cartmodal') e.target.classList.remove('open'); });

  api('GET', '/api/m/' + window.SLUG + '/products')
    .then((list) => {
      state.products = list;
      for (const id of [...state.cart.keys()]) if (!byId(id) || byId(id).sold_out) state.cart.delete(id);
      renderChips(); renderGrid(); renderBar();
    })
    .catch((err) => toast(err.message));
})();
