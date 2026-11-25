'use strict';
// Cash sales made with no connection: kept safely, sent in order, never sent twice as new.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const OfflineQ = require('../public/static/offlineq.js');

function fakeStorage() {
  const data = new Map();
  return {
    getItem: (k) => (data.has(k) ? data.get(k) : null),
    setItem: (k, v) => { data.set(k, String(v)); },
    raw: data,
  };
}

const sale = (n) => ({ method: 'cash', items: [{ product_id: n, qty: 1 }], cash_received_satang: 10000 });
const networkDown = () => Object.assign(new TypeError('Failed to fetch'), { status: undefined });
const refused = (status, message) => Object.assign(new Error(message), { status });

test('a saved sale survives a reload and gets its own id', () => {
  const storage = fakeStorage();
  const first = OfflineQ.create(storage).add(sale(1));
  const second = OfflineQ.create(storage).add(sale(2));
  const after = OfflineQ.create(storage).waiting();
  assert.deepEqual(after.map((s) => s.items[0].product_id), [1, 2]);
  assert.notEqual(first.client_id, second.client_id);
  assert.match(first.client_id, /^till-[0-9a-f]{24}$/);
  assert.ok(first.at);
});

test('every id is acceptable to the server (8 to 64 letters, digits, dash, underscore)', () => {
  const id = OfflineQ.newId();
  assert.ok(id.length >= 8 && id.length <= 64);
  assert.match(id, /^[A-Za-z0-9_-]+$/);
});

test('flush sends oldest first and empties the queue', async () => {
  const q = OfflineQ.create(fakeStorage());
  q.add(sale(1)); q.add(sale(2)); q.add(sale(3));
  const seen = [];
  const result = await q.flush(async (s) => { seen.push(s.items[0].product_id); });
  assert.deepEqual(seen, [1, 2, 3]);
  assert.deepEqual(result, { sent: 3, failed: 0, stopped: false });
  assert.equal(q.list().length, 0);
});

test('a lost connection stops the run and keeps everything left, in order', async () => {
  const q = OfflineQ.create(fakeStorage());
  q.add(sale(1)); q.add(sale(2)); q.add(sale(3));
  let calls = 0;
  const result = await q.flush(async () => { calls += 1; if (calls === 2) throw networkDown(); });
  assert.deepEqual(result, { sent: 1, failed: 0, stopped: true });
  assert.deepEqual(q.waiting().map((s) => s.items[0].product_id), [2, 3]);
  const again = await q.flush(async () => {});
  assert.equal(again.sent, 2, 'the next run sends the rest');
});

test('a server error keeps the sale and stops, because trying again may work', async () => {
  const q = OfflineQ.create(fakeStorage());
  q.add(sale(1)); q.add(sale(2));
  const result = await q.flush(async () => { throw refused(503, 'busy'); });
  assert.equal(result.stopped, true);
  assert.equal(q.waiting().length, 2);
  assert.equal(q.failed().length, 0);
});

test('a sale the server refuses is kept with its reason and the others go on', async () => {
  const q = OfflineQ.create(fakeStorage());
  q.add(sale(1)); q.add(sale(2));
  const result = await q.flush(async (s) => { if (s.items[0].product_id === 1) throw refused(409, '“ชาเย็น” หมดแล้ว'); });
  assert.deepEqual(result, { sent: 1, failed: 1, stopped: false });
  assert.equal(q.waiting().length, 0);
  assert.equal(q.failed()[0].failed, '“ชาเย็น” หมดแล้ว');
  const nothing = await q.flush(async () => { throw new Error('must not resend a refused sale'); });
  assert.equal(nothing.sent, 0, 'a refused sale is not retried by itself');
  q.remove(q.failed()[0].client_id);
  assert.equal(q.list().length, 0, 'the cashier can discard it');
});

test('a full or blocked storage is reported instead of pretending the sale was kept', () => {
  const blocked = { getItem: () => null, setItem: () => { throw new Error('quota'); } };
  assert.equal(OfflineQ.create(blocked).add(sale(1)), null);
});

test('the queue has a ceiling', () => {
  const q = OfflineQ.create(fakeStorage());
  for (let i = 0; i < OfflineQ.MAX_SALES; i += 1) assert.ok(q.add(sale(i)));
  assert.equal(q.add(sale(999)), null);
});

test('a damaged saved list is treated as empty, not as a crash', () => {
  const storage = fakeStorage();
  storage.setItem(OfflineQ.KEY, '{not json');
  assert.deepEqual(OfflineQ.create(storage).list(), []);
});

test('only a missing HTTP status counts as no connection', () => {
  assert.equal(OfflineQ.isNoConnection(networkDown()), true);
  assert.equal(OfflineQ.isNoConnection(refused(500, 'x')), false);
  assert.equal(OfflineQ.isRefusal(refused(409, 'x')), true);
  assert.equal(OfflineQ.isRefusal(refused(429, 'x')), false);
  assert.equal(OfflineQ.isRefusal(refused(500, 'x')), false);
});
