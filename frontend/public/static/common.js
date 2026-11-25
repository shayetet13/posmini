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
    // Logged in as a cashier on an admin page: back to the till.
    if (res.status === 403 && ['/settings', '/dashboard'].includes(location.pathname)) { location.href = '/pos'; return new Promise(() => {}); }
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

  // Three low beeps for a warning the cashier must not miss (the browser only allows it after a tap).
  function alertBeep() {
    try {
      const Ctx = window.AudioContext || window.webkitAudioContext;
      if (!Ctx) return;
      const ctx = alertBeep.ctx || (alertBeep.ctx = new Ctx());
      if (ctx.state === 'suspended') ctx.resume();
      [0, 0.28, 0.56].forEach((at) => {
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.frequency.value = 440;
        gain.gain.setValueAtTime(0.0001, ctx.currentTime + at);
        gain.gain.exponentialRampToValueAtTime(0.35, ctx.currentTime + at + 0.02);
        gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + at + 0.22);
        osc.connect(gain).connect(ctx.destination);
        osc.start(ctx.currentTime + at);
        osc.stop(ctx.currentTime + at + 0.24);
      });
    } catch (_) { /* audio unavailable */ }
  }

  // ---- menu: groups open like folders; on a phone every card is one big colour block
  // A phone is a small touch screen; tablets, iPads and computers keep the usual cards.
  // `?ui=phone` / `?ui=tablet` forces one (handy for trying it on a computer).
  const forced = new URLSearchParams(location.search).get('ui');
  const isPhone = forced ? forced === 'phone'
    : window.matchMedia('(pointer: coarse)').matches && Math.min(screen.width, screen.height) < 600;
  document.documentElement.classList.toggle('phone', isPhone);

  // Black or white text, whichever reads better on this card colour.
  function inkOn(hex) {
    const v = /^#([0-9a-f]{6})$/i.exec(hex || '');
    if (!v) return '#fff';
    const [r, g, b] = [0, 2, 4].map((i) => parseInt(v[1].slice(i, i + 2), 16) / 255)
      .map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.4 ? '#1b1410' : '#fff';
  }

  // What is shown at this level: the top level (filtered by `cat` unless it is null) or the
  // inside of the group at the end of `path`.
  function menuLevel(products, path, cat) {
    const parent = path.length ? path[path.length - 1] : null;
    return products.filter((p) => (p.parent_id ?? null) === parent && (parent !== null || cat == null || p.category === cat));
  }

  // Items that can be ordered inside a group, all the way down.
  function menuCount(products, groupId) {
    const kids = products.filter((p) => p.parent_id === groupId);
    return kids.reduce((n, p) => n + (p.is_group ? menuCount(products, p.id) : 1), 0);
  }

  // One card. A group opens (`onOpen`); an item is added (`onAdd`). `showPrice` keeps the price on
  // phones (the customer's own menu); the cashier's phone shows only the name. An item already in
  // the cart also gets a trash button (`onRemove`) that takes the whole item out again.
  function menuTile(p, o) {
    const group = !!p.is_group;
    const out = !group && !!p.sold_out;
    const qty = o.qty || 0;
    const style = p.color ? '--card-bg:' + p.color + ';--card-fg:' + inkOn(p.color) : null;
    const tile = h('button', {
      class: 'tile' + (qty ? ' sel' : '') + (group ? ' grp' : '') + (out ? ' out' : '') + (o.showPrice ? ' showp' : ''), type: 'button', style,
      'aria-label': group ? 'เปิดกลุ่ม ' + p.name : p.name + ' ' + baht(p.price_satang) + (out ? ' หมดแล้ว' : ''),
      onclick: () => {
        if (group) o.onOpen(p.id);
        else if (out) toast('“' + p.name + '” หมดแล้ว');
        else o.onAdd(p.id);
      },
    },
      qty > 0 && h('span', { class: 'in' }, '×' + qty),
      out && h('span', { class: 'outtag' }, 'หมด'),
      productPic(p, true),
      h('div', { class: 'tbody' },
        h('span', { class: 'n' }, p.name),
        h('div', { class: 'trow' },
          group ? h('span', { class: 'p grp-n' }, menuCount(o.products, p.id) + ' รายการ') : h('span', { class: 'p num' }, baht(p.price_satang)),
          h('span', { class: 'add', 'aria-hidden': 'true' }, icon(group ? 'chevronR' : 'plus')))));
    if (group || !qty || !o.onRemove) return tile;
    return h('div', { class: 'tilebox' }, tile,
      h('button', { class: 'tile-del', type: 'button', 'aria-label': 'ลบ ' + p.name + ' ออกจากตะกร้า', title: 'ลบรายการนี้ออกจากตะกร้า', onclick: () => o.onRemove(p.id) }, icon('trash', 18)));
  }

  // The way back out of the groups: a button per level, the last one is where you are.
  function menuCrumbs(products, path, onGo) {
    const name = (id) => (products.find((p) => p.id === id) || {}).name || '…';
    return [
      h('button', { class: 'chip crumb back', type: 'button', onclick: () => onGo(path.slice(0, -1)) }, '‹ กลับ'),
      h('button', { class: 'chip crumb', type: 'button', onclick: () => onGo([]) }, 'ทั้งหมด'),
      ...path.map((id, i) => h('button', { class: 'chip crumb' + (i === path.length - 1 ? ' on' : ''), type: 'button', onclick: () => onGo(path.slice(0, i + 1)) }, name(id))),
    ];
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

  // A small box with one line of text to fill in. Resolves to the trimmed text, or null when the
  // person cancels. `required` stops an empty answer; `suggestions` are one-tap answers.
  function askText(opts = {}) {
    return new Promise((resolve) => {
      const previous = document.activeElement;
      const min = opts.required ? Math.max(1, opts.min || 1) : 0;
      const input = h('input', { type: 'text', maxlength: opts.max || 60, placeholder: opts.placeholder || '', autocomplete: 'off', enterkeyhint: 'done' });
      input.value = opts.value || '';
      const warn = h('p', { class: 'warnbox', role: 'alert', hidden: true }, '');
      const okBtn = h('button', { class: 'btn block ' + (opts.tone === 'danger' ? 'danger-fill' : 'primary'), type: 'button' }, opts.okText || 'ตกลง');
      const cancelBtn = h('button', { class: 'btn block', type: 'button' }, opts.cancelText || 'ยกเลิก');
      const overlay = h('div', { class: 'modal confirm-modal open' },
        h('div', { class: 'sheet confirm asktext', role: 'dialog', 'aria-modal': 'true', 'aria-label': opts.title || 'กรอกข้อความ' },
          h('h3', null, opts.title || 'กรอกข้อความ'),
          opts.message ? h('p', { class: 'cmsg' }, opts.message) : null,
          h('label', { class: 'field' }, h('span', null, opts.label || 'ข้อความ'), input),
          opts.suggestions && opts.suggestions.length
            ? h('div', { class: 'cash-quick' }, ...opts.suggestions.map((s) => h('button', { class: 'chip', type: 'button', onclick: () => { input.value = s; input.focus(); } }, s)))
            : null,
          warn,
          h('div', { class: 'btns' }, okBtn, cancelBtn)));
      const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); done(null); } };
      function done(value) {
        document.removeEventListener('keydown', onKey, true);
        overlay.remove();
        if (previous && previous.focus) previous.focus();
        resolve(value);
      }
      function submit() {
        const text = input.value.trim();
        if (text.length < min) {
          warn.textContent = opts.requiredText || 'ต้องกรอกข้อความนี้ก่อน';
          warn.hidden = false;
          input.focus();
          return;
        }
        done(text);
      }
      okBtn.addEventListener('click', submit);
      cancelBtn.addEventListener('click', () => done(null));
      input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); submit(); } });
      overlay.addEventListener('click', (e) => { if (e.target === overlay) done(null); });
      document.addEventListener('keydown', onKey, true);
      document.body.append(overlay);
      input.focus();
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

  const CHILD_PITCH = 1.8; // 0..2; higher sounds more like a child
  const SPEAK_AFTER_CHIME_MS = 150;
  const SPEAK_AFTER_CANCEL_MS = 350;

  const Voice = {
    key: 'voice:' + location.pathname.split('/')[1],
    thai: null,
    get enabled() { try { return localStorage.getItem(this.key) === '1'; } catch (_) { return false; } },
    set enabled(v) { try { localStorage.setItem(this.key, v ? '1' : '0'); } catch (_) { /* ignore */ } },
    // A Thai voice that is installed on the device. Online voices (Edge "Natural", Google) have
    // to fetch the sound first: they start late and often lose the first syllable, so they are
    // only used when the device has no offline Thai voice. Female before male.
    pickVoice() {
      if (!('speechSynthesis' in window)) return null;
      const thai = speechSynthesis.getVoices().filter((v) => v.lang && v.lang.toLowerCase().startsWith('th'));
      const offline = thai.filter((v) => v.localService);
      const score = (v) => (/premwadee|achara|narisa|kanya|female/i.test(v.name) ? 3 : 0)
        - (/niwat|pattara|somchai|\bmale\b/i.test(v.name) ? 3 : 0);
      this.thai = (offline.length ? offline : thai).sort((a, b) => score(b) - score(a))[0] || null;
      return this.thai;
    },
    // A high pitch makes the voice sound like a child; a touch faster keeps it lively.
    utter(text) {
      const u = new SpeechSynthesisUtterance(text);
      u.lang = 'th-TH';
      u.rate = 1.05;
      u.pitch = CHILD_PITCH;
      const voice = this.thai || this.pickVoice();
      if (voice) u.voice = voice;
      return u;
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
    // Speakers (Bluetooth, HDMI) go to sleep when nothing plays and swallow the first syllables
    // when they wake. A tone far too quiet to notice keeps them awake while sound is switched on.
    keepAwake() {
      try {
        const Ctx = window.AudioContext || window.webkitAudioContext;
        if (!Ctx) return;
        const ctx = this.ctx || (this.ctx = new Ctx());
        // Without a tap or key press the browser keeps audio suspended: ask again every time.
        if (ctx.state === 'suspended') ctx.resume();
        if (this.hum) return;
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.frequency.value = 60;
        gain.gain.value = 0.002;
        osc.connect(gain).connect(ctx.destination);
        osc.start();
        this.hum = osc;
      } catch (_) { /* audio unavailable */ }
    },
    stopKeepAwake() {
      try { if (this.hum) this.hum.stop(); } catch (_) { /* already stopped */ }
      this.hum = null;
    },
    speak(text) {
      if (!this.enabled) return;
      this.keepAwake();
      this.chime();
      if (!('speechSynthesis' in window)) return;
      // Only after cutting off something still being said does the engine need a moment;
      // otherwise start almost at once, just after the chime begins.
      const interrupted = speechSynthesis.speaking || speechSynthesis.pending;
      if (interrupted) speechSynthesis.cancel();
      const u = this.utter(text);
      setTimeout(() => {
        if (speechSynthesis.paused) speechSynthesis.resume();
        speechSynthesis.speak(u);
      }, interrupted ? SPEAK_AFTER_CANCEL_MS : SPEAK_AFTER_CHIME_MS);
    },
    enable() {
      this.enabled = true;
      this.keepAwake();
      if ('speechSynthesis' in window) {
        this.pickVoice();
        speechSynthesis.cancel();
        speechSynthesis.speak(this.utter('เปิดเสียงแล้ว'));
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
      btn.addEventListener('click', () => {
        if (this.enabled) { this.enabled = false; this.stopKeepAwake(); } else this.enable();
        paint();
      });
      // A page reopened with sound already on may only play audio after a tap or key press.
      const wake = () => { if (this.enabled) this.keepAwake(); };
      document.addEventListener('pointerdown', wake, { once: true });
      document.addEventListener('keydown', wake, { once: true });
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
    // The customer screen shows the name typed in settings (it may differ from the shop name).
    const shown = window.DISPLAY_NAME || shop;
    document.querySelectorAll('[data-display]').forEach((el) => { el.textContent = shown; });
    document.querySelectorAll('[data-display-initial]').forEach((el) => { el.textContent = shown.charAt(0); });
    if (shop && !document.title.includes(shop)) document.title += ' · ' + shop;
    const nav = document.getElementById('nav');
    if (nav) {
      const active = nav.dataset.active;
      nav.innerHTML = '<nav class="rail" aria-label="เมนูหลัก"><div class="brandmark" aria-hidden="true"></div>'
        + NAV_ITEMS.map(([key, href, label]) => '<a class="rail-item' + (key === active ? ' on" aria-current="page' : '') + '" href="' + href + '">' + svg(NAV_ICONS[key]) + '<span>' + label + '</span></a>').join('')
        + '<button class="rail-item" id="calcBtn" type="button" aria-label="เครื่องคิดเลข">' + svg(NAV_ICONS.calc) + '<span>คิดเลข</span></button>'
        + '<form class="railout" method="post" action="/logout"><button type="submit" aria-label="ออกจากระบบ">' + svg(NAV_ICONS.logout) + '<span>ออก</span></button></form></nav>';
      nav.querySelector('.brandmark').textContent = window.SHOP_INITIAL || '';
      // A cashier does not see the takings or the setup: take those links out of the rail.
      fetch('/api/me', { credentials: 'same-origin' }).then((r) => (r.ok ? r.json() : null)).then((me) => {
        window.ME = me;
        if (me && me.role !== 'admin') nav.querySelectorAll('a[href^="/dashboard"], a[href^="/settings"]').forEach((a) => a.remove());
      }).catch(() => { /* the rail just stays complete */ });
    }
  }
  bootPage();

  // ---- installable app (PWA): service worker + an "install" banner -------------------------------
  const INSTALL_PAGES = ['/login', '/pos', '/settings', '/dashboard'];
  const INSTALL_DISMISSED = 'posmini-install-dismissed';
  const isStandalone = () => window.matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;
  const isApple = () => /iphone|ipad|ipod/i.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
  let installEvent = null;

  function installDismissed() {
    try { return localStorage.getItem(INSTALL_DISMISSED) === '1'; } catch (_) { return false; }
  }

  function showInstallBanner(text, actionLabel, action) {
    if (isStandalone() || installDismissed() || document.getElementById('installBar')) return;
    const bar = document.createElement('div');
    bar.id = 'installBar';
    bar.className = 'install-bar';
    bar.setAttribute('role', 'region');
    bar.setAttribute('aria-label', 'ติดตั้งแอป');
    const msg = document.createElement('span');
    msg.textContent = text;
    bar.append(msg);
    if (action) {
      const go = document.createElement('button');
      go.type = 'button';
      go.className = 'btn small primary';
      go.textContent = actionLabel;
      go.addEventListener('click', async () => { bar.remove(); await action(); });
      bar.append(go);
    }
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'btn small ghost';
    close.textContent = 'ปิด';
    close.addEventListener('click', () => {
      bar.remove();
      try { localStorage.setItem(INSTALL_DISMISSED, '1'); } catch (_) { /* private mode */ }
    });
    bar.append(close);
    document.body.append(bar);
  }

  function setupInstall() {
    if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js').catch(() => { /* needs https; fine without */ });
    const offered = INSTALL_PAGES.includes(location.pathname) || location.pathname.startsWith('/display/');
    if (!offered || isStandalone()) return;
    // Android / desktop Chrome and Edge: the browser tells us the site can be installed.
    window.addEventListener('beforeinstallprompt', (ev) => {
      ev.preventDefault();
      installEvent = ev;
      showInstallBanner('ติดตั้งเป็นแอปบนเครื่องนี้ เปิดได้เต็มจอจากหน้าจอหลัก', 'ติดตั้ง', async () => {
        installEvent.prompt();
        await installEvent.userChoice.catch(() => {});
        installEvent = null;
      });
    });
    window.addEventListener('appinstalled', () => document.getElementById('installBar')?.remove());
    // iPhone / iPad (Safari) cannot be asked from a page, so show how to do it by hand.
    if (isApple()) showInstallBanner('ติดตั้งเป็นแอป: แตะปุ่มแชร์ (สี่เหลี่ยมมีลูกศร) แล้วเลือก “เพิ่มลงในหน้าจอโฮม”', '', null);
  }
  setupInstall();

  window.PM = { $, h, baht, api, upload, productPic, icon, alertBeep, isPhone, inkOn, menuLevel, menuCount, menuTile, menuCrumbs, qrCard, confirm: confirmBox, askText, toast, listen, Voice, keepAwake };
})();
