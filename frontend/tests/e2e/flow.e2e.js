'use strict';
// The whole sell, pay, call story against the real binary, with a pretend payment API and a pretend LINE.
//
//   cargo build            (the test starts target/debug/posmini, or $POSMINI_BIN)
//   node --test frontend/tests/e2e/flow.e2e.js
//
// The app runs in an empty temp folder with an explicit environment, so it can never read a
// developer's own .env and reach a real payment API.
const { test, before, after } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const { spawn } = require('node:child_process');

const ROOT = path.resolve(__dirname, '..', '..', '..');
const EXE = process.env.POSMINI_BIN || path.join(ROOT, 'target', 'debug', process.platform === 'win32' ? 'posmini.exe' : 'posmini');
const ADMIN_PASSWORD = 'e2e-only-password-1';
const SHOP = 'ร้านทดสอบ';

// ---------------------------------------------------------------- pretend servers

function listen(handler) {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const chunks = [];
      req.on('data', (c) => chunks.push(c));
      req.on('end', () => handler(req, Buffer.concat(chunks).toString('utf8'), res));
    });
    server.listen(0, '127.0.0.1', () => resolve({ server, base: 'http://127.0.0.1:' + server.address().port }));
  });
}

const send = (res, status, body) => {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(body));
};

// The payment API: one online bot with one chat; a payment stays pending until the test pays it.
function payApi() {
  const payments = new Map();
  let next = 1;
  const handler = (req, body, res) => {
    const url = new URL(req.url, 'http://x');
    if (req.headers.authorization !== 'Bearer e2e-key') return send(res, 401, { error: { message: 'bad key' } });
    if (req.method === 'GET' && url.pathname === '/v1/bots') return send(res, 200, { data: [{ id: 'bot_1', name: 'kbank', enabled: true, status: 'online' }] });
    if (req.method === 'GET' && url.pathname === '/v1/bots/bot_1/targets') return send(res, 200, { data: [{ id: 'chat_1', name: 'KBank Live', enabled: true }] });
    if (req.method === 'POST' && url.pathname === '/v1/promptpay/payments') {
      const asked = JSON.parse(body);
      const id = 'pay_' + next++;
      const payment = { id, status: 'pending', qrPayload: '0002010102113' + id, amount: asked.amount, expiresAt: new Date(Date.now() + 120000).toISOString() };
      payments.set(id, payment);
      return send(res, 200, { data: payment });
    }
    const one = url.pathname.match(/^\/v1\/promptpay\/payments\/([^/]+)(\/cancel)?$/);
    if (one && payments.has(one[1])) {
      const payment = payments.get(one[1]);
      if (req.method === 'POST' && one[2]) { payment.status = 'cancelled'; return send(res, 200, { data: payment }); }
      if (req.method === 'GET') return send(res, 200, { data: payment });
    }
    return send(res, 404, { error: { message: 'not found' } });
  };
  return { handler, payments, pay(id) { Object.assign(payments.get(id), { status: 'succeeded', bank: 'kbank', payerName: 'สมชาย ใจดี', amountSatang: Math.round(Number(payments.get(id).amount) * 100) }); } };
}

// LINE: accepts one ID token and writes down every push.
function lineApi() {
  const pushes = [];
  const handler = (req, body, res) => {
    if (req.url === '/oauth2/v2.1/verify') {
      const form = new URLSearchParams(body);
      return form.get('id_token') === 'good-token' && form.get('client_id') === '1234567890'
        ? send(res, 200, { sub: 'U-e2e-customer' })
        : send(res, 400, { error: 'invalid_request' });
    }
    if (req.url === '/v2/bot/message/push') {
      pushes.push({ auth: req.headers.authorization, body: JSON.parse(body) });
      return send(res, 200, {});
    }
    return send(res, 404, {});
  };
  return { handler, pushes };
}

// ---------------------------------------------------------------- the app under test

const ctx = {};

async function until(check, what, ms = 15000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > end) throw new Error('timed out waiting for ' + what + '\n' + ctx.log());
    await new Promise((r) => setTimeout(r, 100));
  }
}

before(async () => {
  assert.ok(fs.existsSync(EXE), 'build the backend first (cargo build): ' + EXE);
  ctx.pay = payApi();
  ctx.line = lineApi();
  ctx.payServer = await listen(ctx.pay.handler);
  ctx.lineServer = await listen(ctx.line.handler);
  ctx.dir = fs.mkdtempSync(path.join(os.tmpdir(), 'posmini-e2e-'));
  const port = await new Promise((resolve) => {
    const probe = http.createServer().listen(0, '127.0.0.1', () => { const p = probe.address().port; probe.close(() => resolve(p)); });
  });
  ctx.base = 'http://127.0.0.1:' + port;
  let output = '';
  ctx.log = () => output.slice(-3000);
  ctx.proc = spawn(EXE, [], {
    cwd: ctx.dir,
    env: {
      PATH: process.env.PATH, SystemRoot: process.env.SystemRoot,
      DATABASE_URL: 'sqlite://' + path.join(ctx.dir, 'e2e.db').replace(/\\/g, '/') + '?mode=rwc',
      BIND_ADDR: '127.0.0.1:' + port, SHOP_SLUG: 'main', SHOP_NAME: SHOP,
      ADMIN_USER: 'admin', ADMIN_PASSWORD, COOKIE_SECURE: 'false',
      PAYMENT_API_BASE: ctx.payServer.base, PROMPTPAY_API_KEY: 'e2e-key', PROMPTPAY_ID: '0812345678',
      LINE_API_BASE: ctx.lineServer.base, BACKUP_DIR: path.join(ctx.dir, 'backups'),
    },
  });
  ctx.proc.stdout.on('data', (d) => { output += d; });
  ctx.proc.stderr.on('data', (d) => { output += d; });
  await until(async () => fetch(ctx.base + '/health').then((r) => r.ok).catch(() => false), 'the app to start');
});

after(async () => {
  if (ctx.proc) ctx.proc.kill();
  ctx.payServer && ctx.payServer.server.close();
  ctx.lineServer && ctx.lineServer.server.close();
  // Windows keeps the database file locked for a moment after the process ends.
  await new Promise((r) => setTimeout(r, 300));
  try { fs.rmSync(ctx.dir, { recursive: true, force: true }); } catch (_) { /* leftover temp files are harmless */ }
});

// ---------------------------------------------------------------- a tiny client

function client() {
  let cookie = '';
  const call = async (method, url, body) => {
    const res = await fetch(ctx.base + url, {
      method,
      headers: { ...(body ? { 'content-type': 'application/json' } : {}), ...(cookie ? { cookie } : {}) },
      body: body ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    let json = null;
    try { json = JSON.parse(text); } catch (_) { /* not json */ }
    return { status: res.status, json, text };
  };
  return {
    call,
    async login() {
      const res = await fetch(ctx.base + '/login', {
        method: 'POST', redirect: 'manual',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ username: 'admin', password: ADMIN_PASSWORD }),
      });
      cookie = (res.headers.getSetCookie() || []).map((c) => c.split(';')[0]).join('; ');
      assert.ok(cookie, 'login gives a session cookie');
    },
  };
}

const bangkokToday = () => new Date(Date.now() + 7 * 3600 * 1000).toISOString().slice(0, 10);

// ---------------------------------------------------------------- the story

test('a customer orders on their phone, pays by QR, is told on LINE, and the shop sees it all', async () => {
  const shop = client();
  await shop.login();
  const tea = (await shop.call('POST', '/api/products', { name: 'ชาเย็น', category: '', price_satang: 4000 })).json.id;
  assert.ok(tea, 'the shop has something to sell');

  const setup = await shop.call('PUT', '/api/settings/line', { liff_id: '1234-e2e', login_channel_id: '1234567890', channel_token: 'e2e-line-token' });
  assert.equal(setup.status, 200, setup.text);

  // The till says automatic checking works.
  const health = await shop.call('GET', '/api/pay/health');
  assert.equal(health.json.mode, 'auto');
  assert.equal(health.json.ok, true, JSON.stringify(health.json));

  // 1. A customer scans the shop QR and orders, with a note.
  const customer = client();
  const menu = await customer.call('GET', '/api/m/main/products');
  assert.ok(menu.json.some((p) => p.id === tea));
  const order = await customer.call('POST', '/api/m/main/orders', { items: [{ product_id: tea, qty: 2, note: 'ไม่หวาน' }], note: 'ถึงแล้วโทรหา' });
  assert.equal(order.status, 200, order.text);
  const token = order.json.token;

  // 2. Only one QR waits at a time: the next customer is asked to hold on.
  const second = await client().call('POST', '/api/m/main/orders', { items: [{ product_id: tea, qty: 1 }] });
  assert.equal(second.status, 409, second.text);
  assert.match(second.json.error, /รอ|QR/);

  // 3. The customer's page shows the QR and the amount.
  const waiting = (await customer.call('GET', '/api/q/' + token)).json;
  assert.equal(waiting.status, 'pending_payment');
  assert.equal(waiting.total_satang, 8000);
  assert.ok(waiting.qr_svg, 'a QR to scan');
  assert.equal(waiting.items[0].note, 'ไม่หวาน');
  assert.equal(waiting.note, 'ถึงแล้วโทรหา');
  const [paymentId, payment] = [...ctx.pay.payments].find(([, p]) => p.amount === '80.00') || [];
  assert.ok(paymentId, 'the verify API was asked for 80.00 baht, got ' + JSON.stringify([...ctx.pay.payments.values()].map((p) => p.amount)));

  // 4. They link LINE; a forged token is refused.
  const offer = (await customer.call('GET', '/api/q/' + token + '/line')).json;
  assert.deepEqual([offer.enabled, offer.linked, offer.liff_id], [true, false, '1234-e2e']);
  assert.equal((await customer.call('POST', '/api/q/' + token + '/line', { id_token: 'forged' })).status, 400);
  assert.equal((await customer.call('POST', '/api/q/' + token + '/line', { id_token: 'good-token' })).status, 200);

  // 5. The bank notification arrives; the app notices by itself and issues the queue number.
  ctx.pay.pay(paymentId);
  const paid = await until(async () => {
    const view = (await customer.call('GET', '/api/q/' + token)).json;
    return view.status === 'paid' ? view : null;
  }, 'the payment to be noticed');
  assert.equal(paid.queue_label, 'A001');

  // 6. The slip records who paid and that the bot, not a person, checked it.
  const day = (await shop.call('GET', '/api/dashboard?period=day&date=' + bangkokToday())).json;
  assert.equal(day.totals.revenue, 8000);
  assert.equal(day.slips, undefined, 'the slips come from their own request, a page at a time');
  const slips = (await shop.call('GET', '/api/dashboard/slips?period=day&date=' + bangkokToday())).json;
  assert.equal(slips.rows.length, 1);
  assert.deepEqual([slips.rows[0].verified, slips.rows[0].bank, slips.rows[0].payer_name], [true, 'kbank', 'สมชาย ใจดี']);
  assert.equal(day.audit.counts.confirm_manual, undefined, 'no one pressed confirm by hand');

  // 7. The place in line is free again for the next customer.
  const next = await client().call('POST', '/api/m/main/orders', { items: [{ product_id: tea, qty: 1 }] });
  assert.equal(next.status, 200, next.text);

  // 8. The cashier calls the queue: the screen changes and LINE is told.
  const called = await shop.call('POST', '/api/orders/' + paid.id + '/call');
  assert.equal(called.status, 200, called.text);
  const push = await until(() => ctx.line.pushes[0], 'the LINE message');
  assert.equal(push.auth, 'Bearer e2e-line-token');
  assert.equal(push.body.to, 'U-e2e-customer');
  assert.ok(push.body.messages[0].text.includes('A001') && push.body.messages[0].text.includes(SHOP), push.body.messages[0].text);
  assert.equal((await customer.call('GET', '/api/q/' + token)).json.status, 'called');
  assert.equal(ctx.line.pushes.length, 1, 'only the customer who asked is messaged');
  assert.equal((await shop.call('POST', '/api/orders/' + paid.id + '/done')).status, 200);
});

test('a cash sale with a discount and a repeated offline send makes exactly one order', async () => {
  const shop = client();
  await shop.login();
  const bun = (await shop.call('POST', '/api/products', { name: 'ซาลาเปา', category: '', price_satang: 2500 })).json.id;
  const sale = { method: 'cash', cash_received_satang: 10000, items: [{ product_id: bun, qty: 2 }], discount: { kind: 'percent', value: 10, reason: 'ลูกค้าประจำ' }, client_id: 'e2e-offline-0001', offline_at: new Date().toISOString() };
  const first = await shop.call('POST', '/api/orders', sale);
  assert.equal(first.status, 200, first.text);
  assert.equal(first.json.total_satang, 4500);
  assert.equal(first.json.change_satang, 5500);
  const again = await shop.call('POST', '/api/orders', sale);
  assert.equal(again.json.id, first.json.id, 'a retry gives the same order back');

  // A paid order cannot be cancelled without saying why, and the reason is kept.
  const bare = await shop.call('POST', '/api/orders/' + first.json.id + '/cancel', {});
  assert.equal(bare.status, 400);
  const why = await shop.call('POST', '/api/orders/' + first.json.id + '/cancel', { reason: 'ลูกค้าเปลี่ยนใจ' });
  assert.equal(why.status, 200, why.text);
  const day = (await shop.call('GET', '/api/dashboard?period=day&date=' + bangkokToday())).json;
  assert.ok(day.cancelled.reasons.some((r) => r.reason === 'ลูกค้าเปลี่ยนใจ'));
});
