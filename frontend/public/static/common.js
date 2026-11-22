(function () {
  const $ = (sel, root) => (root || document).querySelector(sel);

  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === 'class') el.className = v;
      else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? '' : v);
    }
    for (const kid of kids.flat()) {
      if (kid == null || kid === false) continue;
      el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    }
    return el;
  }

  function baht(satang) {
    const n = (satang || 0) / 100;
    return '฿' + n.toLocaleString('th-TH', { minimumFractionDigits: n % 1 ? 2 : 0, maximumFractionDigits: 2 });
  }

  async function api(method, url, body) {
    const res = await fetch(url, {
      method,
      headers: body ? { 'Content-Type': 'application/json' } : undefined,
      body: body ? JSON.stringify(body) : undefined,
      credentials: 'same-origin',
    });
    if (res.status === 401 && ['/pos', '/settings', '/dashboard'].includes(location.pathname)) { location.href = '/login'; return new Promise(() => {}); }
    let data = null;
    try { data = await res.json(); } catch (_) { /* empty body */ }
    if (!res.ok) {
      const err = new Error((data && data.error) || 'เกิดข้อผิดพลาด ลองใหม่อีกครั้ง');
      err.status = res.status;
      throw err;
    }
    return data;
  }

  async function upload(url, blob) {
    const res = await fetch(url, {
      method: 'PUT',
      headers: { 'Content-Type': blob.type || 'application/octet-stream' },
      body: blob,
      credentials: 'same-origin',
    });
    let data = null;
    try { data = await res.json(); } catch (_) { /* empty body */ }
    if (!res.ok) throw new Error((data && data.error) || 'อัปโหลดรูปไม่สำเร็จ');
    return data;
  }

  const QR_TOTAL_SECS = 120;
  const QR_WARN_SECS = 20;

  // Thai QR Payment / PromptPay template around a payment QR. `svg` is the server-made QR markup.
  // Pass `expiresIn` (seconds) to show the expiry countdown; `onExpire` fires once when it hits 0.
  function qrCard(svg, opts = {}) {
    const code = h('div', { class: 'thaiqr-code' });
    code.innerHTML = svg;
    code.append(
      h('img', { class: 'thaiqr-mark', src: '/static/brand-thaiqr-mark.png', alt: '' }),
      h('div', { class: 'thaiqr-expired' }, h('b', null, 'QR หมดอายุ'), h('span', null, 'สแกนจ่ายไม่ได้แล้ว')));
    const card = h('div', { class: 'thaiqr' },
      h('div', { class: 'thaiqr-head' }, h('img', { src: '/static/brand-thaiqr-logo.png', alt: 'Thai QR Payment' })),
      h('div', { class: 'thaiqr-pp' }, h('img', { src: '/static/brand-promptpay-logo.png', alt: 'PromptPay พร้อมเพย์' })),
      code);
    if (opts.expiresIn == null) return card;

    const label = h('span', { class: 'tt' });
    const fill = h('i');
    const timer = h('div', { class: 'thaiqr-timer', role: 'timer' }, label, h('div', { class: 'tbar' }, fill));
    card.append(timer);
    const deadline = Date.now() + Math.max(0, opts.expiresIn) * 1000;
    const total = Math.max(QR_TOTAL_SECS, opts.expiresIn);
    const two = (n) => String(n).padStart(2, '0');
    let wasConnected = false;
    let expired = false;
    let handle;
    const tick = () => {
      if (card.isConnected) wasConnected = true;
      else if (wasConnected) { clearInterval(handle); return; }
      const left = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
      label.textContent = left > 0 ? 'หมดอายุใน ' + two(Math.floor(left / 60)) + ':' + two(left % 60) : 'QR หมดอายุแล้ว';
      fill.style.width = (left * 100) / total + '%';
      timer.classList.toggle('warn', left > 0 && left <= QR_WARN_SECS);
      if (left === 0 && !expired) {
        expired = true;
        clearInterval(handle);
        card.classList.add('expired');
        if (opts.onExpire) setTimeout(opts.onExpire, 0);
      }
    };
    handle = setInterval(tick, 1000);
    tick();
    return card;
  }

  // Small line-icon set (24px grid, stroke follows the text colour).
  const ICONS = {
    plus: '<path d="M12 5v14M5 12h14"/>',
    bag: '<path d="M3 10h18l-2 9a2 2 0 0 1-2 1.5H7a2 2 0 0 1-2-1.5z"/><path d="M8 10l4-6 4 6"/>',
    ticket: '<path d="M3 8a2 2 0 0 0 0 4a2 2 0 0 1 0 4v2h18v-2a2 2 0 0 1 0-4a2 2 0 0 0 0-4V6H3z"/><path d="M14 6v12" stroke-dasharray="2 3"/>',
    check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
    pencil: '<path d="M4 20h4L19 9l-4-4L4 16z"/>',
    trash: '<path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13"/>',
    image: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="M21 16l-5-5-9 9"/>',
    print: '<path d="M7 9V3h10v6M7 17H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2h-2"/><rect x="7" y="14" width="10" height="7" rx="1"/>',
    download: '<path d="M12 4v11M7 11l5 5 5-5M5 20h14"/>',
    chevronL: '<path d="M15 5l-7 7 7 7"/>',
    chevronR: '<path d="M9 5l7 7-7 7"/>',
  };

  function icon(name, size) {
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('class', 'ico');
    svg.setAttribute('aria-hidden', 'true');
    if (size) { svg.style.width = size + 'px'; svg.style.height = size + 'px'; }
    svg.innerHTML = ICONS[name] || '';
    return svg;
  }

  // A stable soft colour per product name for items without a photo.
  function hueOf(text) {
    let hash = 0;
    for (const ch of text) hash = (hash * 31 + ch.codePointAt(0)) % 360;
    return hash;
  }

  function productPic(p, withPlaceholder) {
    if (p.image_v != null) {
      return h('img', { class: 'im', src: '/img/' + p.id + '?v=' + p.image_v, alt: '', loading: 'lazy', decoding: 'async' });
    }
    return withPlaceholder ? h('span', { class: 'im ph', style: '--h:' + hueOf(p.name), 'aria-hidden': 'true' }, [...p.name][0] || '') : null;
  }

  // Centered confirm dialog (replaces window.confirm). Resolves true on OK, false on cancel/Esc/backdrop.
  function confirmBox(message, opts = {}) {
    return new Promise((resolve) => {
      const previous = document.activeElement;
      const tone = opts.tone === 'primary' ? 'primary' : 'danger-fill';
      const okBtn = h('button', { class: 'btn block ' + tone, type: 'button' }, opts.okText || 'ตกลง');
      const cancelBtn = h('button', { class: 'btn block', type: 'button' }, opts.cancelText || 'ยกเลิก');
      const overlay = h('div', { class: 'modal confirm-modal open' },
        h('div', { class: 'sheet confirm', role: 'alertdialog', 'aria-modal': 'true', 'aria-label': opts.title || 'ยืนยันการทำรายการ' },
          h('h3', null, opts.title || 'ยืนยันการทำรายการ'),
          h('p', { class: 'cmsg' }, message),
          h('div', { class: 'btns' }, okBtn, cancelBtn)));
      const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); done(false); } };
      function done(value) {
        document.removeEventListener('keydown', onKey, true);
        overlay.remove();
        if (previous && previous.focus) previous.focus();
        resolve(value);
      }
      okBtn.addEventListener('click', () => done(true));
      cancelBtn.addEventListener('click', () => done(false));
      overlay.addEventListener('click', (e) => { if (e.target === overlay) done(false); });
      document.addEventListener('keydown', onKey, true);
      document.body.append(overlay);
      cancelBtn.focus();
    });
  }

  let toastTimer;
  function toast(message) {
    let el = $('#toast');
    if (!el) { el = h('div', { id: 'toast', class: 'toast', role: 'status', 'aria-live': 'polite' }); document.body.append(el); }
    el.textContent = message;
    el.classList.add('show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.classList.remove('show'), 3200);
  }

  function listen(url, handlers, onReconnect) {
    const source = new EventSource(url);
    for (const [name, fn] of Object.entries(handlers)) {
      source.addEventListener(name, (ev) => {
        let data = {};
        try { data = JSON.parse(ev.data); } catch (_) { /* ignore */ }
        fn(data);
      });
    }
    let dropped = false;
    source.onerror = () => { dropped = true; };
    source.onopen = () => { if (dropped && onReconnect) onReconnect(); dropped = false; };
    return source;
  }

  const Voice = {
    key: 'voice:' + location.pathname.split('/')[1],
    thai: null,
    get enabled() { try { return localStorage.getItem(this.key) === '1'; } catch (_) { return false; } },
    set enabled(v) { try { localStorage.setItem(this.key, v ? '1' : '0'); } catch (_) { /* ignore */ } },
    pickVoice() {
      if (!('speechSynthesis' in window)) return null;
      const voices = speechSynthesis.getVoices();
      this.thai = voices.find((v) => v.lang && v.lang.toLowerCase().startsWith('th')) || null;
      return this.thai;
    },
    chime() {
      try {
        const Ctx = window.AudioContext || window.webkitAudioContext;
        if (!Ctx) return;
        const ctx = this.ctx || (this.ctx = new Ctx());
        if (ctx.state === 'suspended') ctx.resume();
        [660, 880].forEach((freq, i) => {
          const osc = ctx.createOscillator();
          const gain = ctx.createGain();
          osc.frequency.value = freq;
          gain.gain.setValueAtTime(0.0001, ctx.currentTime + i * 0.18);
          gain.gain.exponentialRampToValueAtTime(0.25, ctx.currentTime + i * 0.18 + 0.02);
          gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + i * 0.18 + 0.16);
          osc.connect(gain).connect(ctx.destination);
          osc.start(ctx.currentTime + i * 0.18);
          osc.stop(ctx.currentTime + i * 0.18 + 0.17);
        });
      } catch (_) { /* audio unavailable */ }
    },
    speak(text) {
      if (!this.enabled) return;
      this.chime();
      if (!('speechSynthesis' in window)) return;
      speechSynthesis.cancel();
      const u = new SpeechSynthesisUtterance(text);
      u.lang = 'th-TH';
      u.rate = 0.95;
      const voice = this.thai || this.pickVoice();
      if (voice) u.voice = voice;
      setTimeout(() => {
        if (speechSynthesis.paused) speechSynthesis.resume();
        speechSynthesis.speak(u);
      }, 350);
    },
    enable() {
      this.enabled = true;
      if ('speechSynthesis' in window) {
        this.pickVoice();
        speechSynthesis.cancel();
        const warm = new SpeechSynthesisUtterance('เปิดเสียงแล้ว');
        warm.lang = 'th-TH';
        if (this.thai) warm.voice = this.thai;
        speechSynthesis.speak(warm);
      }
      this.chime();
      if (!this.thai) toast('เครื่องนี้ยังไม่มีเสียงภาษาไทย (iPad: ตั้งค่า > การเข้าถึง > เนื้อหาที่พูด > เสียง > ไทย)');
    },
    bindButton(btn) {
      const paint = () => {
        btn.textContent = this.enabled ? 'เสียง: เปิด' : 'เปิดเสียง';
        btn.setAttribute('aria-pressed', this.enabled);
        btn.classList.toggle('attn', !this.enabled);
      };
      btn.addEventListener('click', () => { if (this.enabled) this.enabled = false; else this.enable(); paint(); });
      if ('speechSynthesis' in window) speechSynthesis.addEventListener?.('voiceschanged', () => this.pickVoice());
      paint();
    },
  };

  async function keepAwake() {
    if (!('wakeLock' in navigator)) return;
    const lock = async () => { try { await navigator.wakeLock.request('screen'); } catch (_) { /* denied */ } };
    await lock();
    document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') lock(); });
  }


  // ---- page boot: shop name, brand letter and the staff menu rail, filled in by the browser so the
  // pages are plain static files (the same ones work from a separate frontend service).
  const NAV_ICONS = {
    pos: '<path d="M6 3h12v18l-3-2-3 2-3-2-3 2z"/><path d="M9 8h6M9 12h6"/>',
    dashboard: '<path d="M4 20V11M10 20V4M16 20v-6M22 20H2"/>',
    settings: '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="18" r="2"/>',
    calc: '<rect x="5" y="3" width="14" height="18" rx="2"/><path d="M8 7h8M8 12h.01M12 12h.01M16 12h.01M8 16h.01M12 16h.01M16 16h.01"/>',
    logout: '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>',
  };
  const NAV_ITEMS = [['pos', '/pos', 'ขาย'], ['dashboard', '/dashboard', 'สรุปยอด'], ['settings', '/settings#menu', 'จัดการ']];
  const svg = (paths) => '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true">' + paths + '</svg>';

  function bootPage() {
    const shop = window.SHOP || '';
    document.querySelectorAll('[data-shop]').forEach((el) => { el.textContent = shop; });
    document.querySelectorAll('[data-initial]').forEach((el) => { el.textContent = window.SHOP_INITIAL || ''; });
    if (shop && !document.title.includes(shop)) document.title += ' · ' + shop;
    const nav = document.getElementById('nav');
    if (nav) {
      const active = nav.dataset.active;
      nav.innerHTML = '<nav class="rail" aria-label="เมนูหลัก"><div class="brandmark" aria-hidden="true"></div>'
        + NAV_ITEMS.map(([key, href, label]) => '<a class="rail-item' + (key === active ? ' on" aria-current="page' : '') + '" href="' + href + '">' + svg(NAV_ICONS[key]) + '<span>' + label + '</span></a>').join('')
        + '<button class="rail-item" id="calcBtn" type="button" aria-label="เครื่องคิดเลข">' + svg(NAV_ICONS.calc) + '<span>คิดเลข</span></button>'
        + '<form class="railout" method="post" action="/logout"><button type="submit" aria-label="ออกจากระบบ">' + svg(NAV_ICONS.logout) + '<span>ออก</span></button></form></nav>';
      nav.querySelector('.brandmark').textContent = window.SHOP_INITIAL || '';
    }
  }
  bootPage();

  window.PM = { $, h, baht, api, upload, productPic, icon, qrCard, confirm: confirmBox, toast, listen, Voice, keepAwake };
})();
