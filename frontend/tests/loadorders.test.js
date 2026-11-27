'use strict';
// The till's order reload (in pos.js) runs here on its own, with a pretend server. It guards the
// bug where the QR window closed the moment it opened: a second caller got "done" before the
// reload that would include its new order had happened.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'public', 'static', 'pos.js'), 'utf8').replace(/\r\n/g, '\n');
const START = '  let ordersReady = false;';
const END = '  // Settings can change on another screen';

function till() {
  const from = SOURCE.indexOf(START);
  const to = SOURCE.indexOf(END);
  assert.ok(from > 0 && to > from, 'the reload code is where this test looks for it');
  const server = { orders: [], requests: [] };
  const state = { orders: [] };
  const shown = [];
  const toasts = [];
  // Each request answers with what the server held when it was asked, once the test lets it.
  const api = () => new Promise((resolve, reject) => {
    const held = [...server.orders];
    server.requests.push({ answer: () => resolve(held), fail: () => reject(new Error('offline')) });
  });
  const build = new Function('state', 'api', 'toast', 'renderOrders', SOURCE.slice(from, to) + '\nreturn loadOrders;');
  const loadOrders = build(state, api, (m) => toasts.push(m), () => shown.push([...state.orders]));
  return { server, state, shown, toasts, loadOrders };
}

const tick = () => new Promise((resolve) => setImmediate(resolve));

test('a caller that arrives during a reload waits for one that includes its new order', async () => {
  const { server, state, loadOrders } = till();
  loadOrders(); // a live event started a reload before the order existed on the list
  server.orders = ['new order'];
  let done = false;
  const mine = loadOrders().then(() => { done = true; });
  server.requests[0].answer(); // the old answer, without the new order
  await tick();
  assert.equal(done, false, 'not done yet: that answer was asked for before the call');
  server.requests[1].answer();
  await mine;
  assert.deepEqual(state.orders, ['new order']);
});

test('a burst of calls makes one reload plus one more, not one each', async () => {
  const { server, loadOrders } = till();
  const all = [loadOrders(), loadOrders(), loadOrders(), loadOrders()];
  server.requests[0].answer();
  await tick();
  server.requests[1].answer();
  await Promise.all(all);
  assert.equal(server.requests.length, 2);
});

test('after a failed reload the next call asks again', async () => {
  const { server, toasts, loadOrders } = till();
  const first = loadOrders();
  server.requests[0].fail();
  await first;
  assert.deepEqual(toasts, ['offline']);
  const second = loadOrders();
  server.requests[1].answer();
  await second;
  assert.equal(server.requests.length, 2);
});
