(function () {
  const { $, h, baht, api, productPic, toast } = PM;
  const ALL = 'ทั้งหมด';
  const state = { products: [], cat: ALL, cart: new Map(), busy: false };

  const byId = (id) => state.products.find((p) => p.id === id);
  const lines = () => [...state.cart].map(([id, qty]) => ({ p: byId(id), qty })).filter((l) => l.p);
  const total = () => lines().reduce((s, l) => s + l.p.price_satang * l.qty, 0);
  const count = () => lines().reduce((s, l) => s + l.qty, 0);

  function change(id, delta) {
    const next = (state.cart.get(id) || 0) + delta;
    if (next <= 0) state.cart.delete(id); else state.cart.set(id, Math.min(next, 99));
    renderGrid(); renderBar();
    if ($('#cartmodal').classList.contains('open')) renderCart();
  }

  function renderChips() {
    const cats = [ALL, ...new Set(state.products.map((p) => p.category).filter(Boolean))];
    $('#chips').replaceChildren(...cats.map((c) =>
      h('button', { class: 'chip' + (c === state.cat ? ' on' : ''), type: 'button', onclick: () => { state.cat = c; renderChips(); renderGrid(); } }, c)));
  }

  function renderGrid() {
    const list = state.products.filter((p) => state.cat === ALL || p.category === state.cat);
    $('#grid').replaceChildren(...list.map((p) => {
      const qty = state.cart.get(p.id) || 0;
      return h('button', { class: 'tile' + (qty ? ' sel' : ''), type: 'button', 'aria-label': p.name + ' ' + baht(p.price_satang), onclick: () => change(p.id, 1) },
        qty > 0 && h('span', { class: 'in' }, '×' + qty),
        productPic(p, true),
        h('div', { class: 'tbody' },
          h('span', { class: 'n' }, p.name),
          h('div', { class: 'trow' }, h('span', { class: 'p num' }, baht(p.price_satang)), h('span', { class: 'add', 'aria-hidden': 'true' }, PM.icon('plus')))));
    }));
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
    try {
      const items = [...state.cart].map(([product_id, qty]) => ({ product_id, qty }));
      const res = await api('POST', '/api/m/' + window.SLUG + '/orders', { items });
      location.href = '/q/' + res.token;
    } catch (err) {
      toast(err.message);
      state.busy = false;
    }
  }

  function renderCart() {
    const ls = lines();
    $('#cartbox').replaceChildren(
      h('h3', { style: 'margin-bottom:6px' }, 'ตะกร้าของคุณ'),
      h('div', { class: 'ticket' }, ...(ls.length ? ls.map(({ p, qty }) => h('div', { class: 'row' },
        h('span', { class: 'nm' }, p.name),
        h('span', { class: 'price num' }, baht(p.price_satang * qty)),
        h('div', { class: 'stepper' },
          h('button', { class: 'qb', type: 'button', 'aria-label': 'ลด ' + p.name, onclick: () => change(p.id, -1) }, '−'),
          h('span', { class: 'qty num' }, qty),
          h('button', { class: 'qb', type: 'button', 'aria-label': 'เพิ่ม ' + p.name, onclick: () => change(p.id, 1) }, '+'))))
        : [h('p', { class: 'muted empty' }, 'ยังไม่ได้เลือกสินค้า')])),
      h('div', { class: 'perf' }),
      h('div', { class: 'total', style: 'margin-top:14px' }, h('span', { class: 'muted' }, 'รวมทั้งหมด'), h('b', { class: 'num' }, baht(total()))),
      h('div', { class: 'btns', style: 'display:grid;gap:8px;margin-top:14px' },
        h('button', { class: 'btn primary block lg', style: 'flex-direction:row', type: 'button', onclick: submit }, 'ยืนยันและไปชำระเงิน'),
        h('button', { class: 'btn block', type: 'button', onclick: () => $('#cartmodal').classList.remove('open') }, 'เลือกสินค้าเพิ่ม')));
  }

  $('#open').addEventListener('click', () => { renderCart(); $('#cartmodal').classList.add('open'); });
  $('#cartmodal').addEventListener('click', (e) => { if (e.target.id === 'cartmodal') e.target.classList.remove('open'); });

  api('GET', '/api/m/' + window.SLUG + '/products')
    .then((list) => { state.products = list; renderChips(); renderGrid(); renderBar(); })
    .catch((err) => toast(err.message));
})();
