(function () {
  const OPS = '+−×÷';
  const PRECISION = 1e8;
  const MAX_EXPR = 60;
  const BILLS = [20, 50, 100, 500, 1000];
  const HISTORY_SHOWN = 3;

  // ------------------------------------------------------------------ engine
  // expr := ['−'] term (('+'|'−') term)*      term := factor (('×'|'÷') factor)*
  // factor := number ['%']   — `a + b%` means a plus b percent of a (like a shop calculator).
  function evaluate(src) {
    const toks = src.match(/\d+\.?\d*|\.\d+|[+−×÷%]/g);
    if (!toks) throw new Error('syntax');
    let i = 0;
    const factor = () => {
      const t = toks[i];
      if (t === undefined || !/^[\d.]/.test(t)) throw new Error('syntax');
      i += 1;
      const v = Number.parseFloat(t);
      if (toks[i] === '%') { i += 1; return { v: v / 100, pct: true }; }
      return { v, pct: false };
    };
    const term = () => {
      const first = factor();
      let v = first.v;
      let pct = first.pct;
      while (toks[i] === '×' || toks[i] === '÷') {
        const op = toks[i];
        i += 1;
        const r = factor();
        pct = false;
        if (op === '×') v *= r.v;
        else if (r.v === 0) throw new Error('div0');
        else v /= r.v;
      }
      return { v, pct };
    };
    let negative = false;
    if (toks[i] === '−') { negative = true; i += 1; }
    const head = term();
    let acc = negative ? -head.v : head.v;
    while (i < toks.length) {
      const op = toks[i];
      i += 1;
      if (op !== '+' && op !== '−') throw new Error('syntax');
      const t = term();
      const rhs = t.pct ? acc * t.v : t.v;
      acc = op === '+' ? acc + rhs : acc - rhs;
    }
    return Math.round(acc * PRECISION) / PRECISION;
  }

  /** Result as a number, or null while the expression is incomplete, or 'div0'. */
  function tryEvaluate(src) {
    try { return evaluate(src); } catch (err) { return err.message === 'div0' ? 'div0' : null; }
  }

  const group = (digits) => digits.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  // Thousands separators for every number in the text being typed.
  const pretty = (text) => text.replace(/\d+(\.\d*)?/g, (m) => { const [int, frac] = m.split('.'); return group(int) + (frac === undefined ? '' : '.' + frac); });
  const plain = (n) => n.toFixed(8).replace(/\.?0+$/, '').replace('-', '−');
  const fmt = (n) => pretty(plain(n));

  if (typeof document === 'undefined') { module.exports = { evaluate, tryEvaluate, pretty, plain, fmt }; return; }

  // ---------------------------------------------------------------------- UI
  const { h, baht } = PM;
  let expr = '';
  let done = false;
  let history = [];
  let overlay;
  let parts;

  const lastNumber = (text) => (text.match(/\d*\.?\d*$/) || [''])[0];
  const lastChar = (text) => text.slice(-1);
  const cartTotal = () => (typeof PM.cartTotal === 'function' ? PM.cartTotal() / 100 : 0);

  function press(key) {
    if (/^\d$/.test(key)) {
      if (done) { expr = ''; done = false; }
      if (lastChar(expr) === '%' || expr.length >= MAX_EXPR) return;
      const cur = lastNumber(expr);
      expr = cur === '0' ? expr.slice(0, -1) + key : expr + key;
    } else if (key === '.') {
      if (done) { expr = ''; done = false; }
      const cur = lastNumber(expr);
      if (cur.includes('.') || lastChar(expr) === '%') return;
      expr += cur === '' ? '0.' : '.';
    } else if (OPS.includes(key)) {
      done = false;
      if (expr === '') { if (key === '−') expr = '−'; }
      else if (expr === '−') return;
      else if (OPS.includes(lastChar(expr))) expr = expr.slice(0, -1) + key;
      else if (expr.length < MAX_EXPR) expr += key;
    } else if (key === '%') {
      if (/\d$/.test(expr)) { done = false; expr += '%'; }
    } else if (key === 'back') {
      if (done) { expr = ''; done = false; } else expr = expr.slice(0, -1);
    } else if (key === 'ac') {
      expr = ''; done = false;
    } else if (key === '=') {
      const result = tryEvaluate(expr);
      if (typeof result === 'number') {
        history = [{ expr, result }, ...history].slice(0, HISTORY_SHOWN);
        expr = plain(result);
        done = true;
      }
    }
    render();
  }

  function useValue(value) { expr = plain(value); done = true; render(); }

  function render() {
    // While an operator is dangling ("85+") keep showing the value so far.
    const shown = OPS.includes(lastChar(expr)) ? expr.slice(0, -1) : expr;
    const live = tryEvaluate(shown);
    const hasOperator = /[+−×÷]/.test(shown.slice(1));
    let big;
    let bigClass = 'calc-val num';
    if (live === 'div0') { big = 'หารด้วยศูนย์ไม่ได้'; bigClass += ' err'; }
    else if (typeof live === 'number' && (done || hasOperator || /%$/.test(shown) || shown !== expr)) big = fmt(live);
    else big = pretty(lastNumber(expr) || (expr === '−' ? '−' : '0'));
    parts.expr.textContent = expr ? pretty(expr) + (done ? ' =' : '') : ' ';
    parts.val.className = bigClass;
    parts.val.textContent = big;
    parts.val.scrollLeft = parts.val.scrollWidth;

    parts.hist.replaceChildren(...history.map((entry) => h('button', { class: 'calc-h', type: 'button', onclick: () => useValue(entry.result) },
      h('span', null, pretty(entry.expr) + ' ='), h('b', { class: 'num' }, fmt(entry.result)))));

    const total = cartTotal();
    const bills = h('div', { class: 'calc-bills', role: 'group', 'aria-label': 'รับเงินมา' },
      ...BILLS.map((b) => h('button', { type: 'button', onclick: () => useValue(b) }, '฿' + b)));
    const rows = [h('div', { class: 'calc-cashhead' }, h('span', { class: 'muted' }, 'รับเงินมา'),
      total > 0 ? h('button', { class: 'btn small', type: 'button', onclick: () => useValue(total) }, 'ใช้ยอดตะกร้า ' + baht(Math.round(total * 100))) : null), bills];
    if (total > 0 && typeof live === 'number' && live > 0) {
      const change = Math.round((live - total) * 100);
      rows.push(change >= 0
        ? h('div', { class: 'calc-change' }, h('span', null, 'เงินทอน'), h('b', { class: 'num disp' }, baht(change)))
        : h('div', { class: 'calc-change short' }, h('span', null, 'ยังขาดอีก'), h('b', { class: 'num disp' }, baht(-change))));
    }
    parts.cash.replaceChildren(...rows);
  }

  const KEYS = [
    ['ac', 'AC', 'fn', 'ล้างทั้งหมด'], ['back', '⌫', 'fn', 'ลบตัวสุดท้าย'], ['%', '%', 'fn', 'เปอร์เซ็นต์'], ['÷', '÷', 'op', 'หาร'],
    ['7', '7'], ['8', '8'], ['9', '9'], ['×', '×', 'op', 'คูณ'],
    ['4', '4'], ['5', '5'], ['6', '6'], ['−', '−', 'op', 'ลบ'],
    ['1', '1'], ['2', '2'], ['3', '3'], ['+', '+', 'op', 'บวก'],
    ['0', '0', 'zero'], ['.', '.', 'dec', 'จุดทศนิยม'], ['=', '=', 'eq', 'เท่ากับ'],
  ];

  function close() {
    overlay.classList.remove('open');
    document.removeEventListener('keydown', onKey, true);
  }

  function onKey(e) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key;
    // A comma is never read as a decimal point: "1,000" must not turn into 1.000.
    const map = { '*': '×', x: '×', X: '×', '/': '÷', '-': '−', Enter: '=', Backspace: 'back', Delete: 'ac' };
    if (k === 'Escape') { e.preventDefault(); close(); return; }
    const key = map[k] || k;
    if (/^\d$/.test(key) || ['.', '+', '−', '×', '÷', '%', '=', 'back', 'ac'].includes(key)) {
      if (e.target instanceof HTMLButtonElement && (k === 'Enter' || k === ' ')) return;
      e.preventDefault();
      press(key);
    }
  }

  function build() {
    parts = {
      hist: h('div', { class: 'calc-hist' }),
      expr: h('div', { class: 'calc-expr num' }),
      val: h('div', { class: 'calc-val num' }),
      cash: h('div', { class: 'calc-cash' }),
    };
    const keys = KEYS.map(([key, label, cls, aria]) =>
      h('button', { class: 'ck ' + (cls || ''), type: 'button', 'aria-label': aria || label, onclick: () => press(key) }, label));
    overlay = h('div', { class: 'modal calc-modal', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'เครื่องคิดเลข' },
      h('div', { class: 'sheet calc' },
        h('div', { class: 'calc-head' }, h('h3', null, 'เครื่องคิดเลข'), h('button', { class: 'btn ghost small', type: 'button', onclick: close }, 'ปิด')),
        h('div', { class: 'calc-screen', 'aria-live': 'polite' }, parts.hist, parts.expr, parts.val),
        parts.cash,
        h('div', { class: 'calc-keys' }, ...keys)));
    overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
    document.body.append(overlay);
  }

  function open() {
    if (!overlay) build();
    render();
    overlay.classList.add('open');
    document.addEventListener('keydown', onKey, true);
  }

  const button = document.getElementById('calcBtn');
  if (button) button.addEventListener('click', open);
  PM.openCalc = open;
})();
