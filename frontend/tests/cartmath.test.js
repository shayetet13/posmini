'use strict';
// The till's discount arithmetic must agree with the server's (backend/src/orders.rs).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { discountOff, totals } = require('../public/static/cartmath.js');

test('a percent discount rounds down to whole satang', () => {
  assert.equal(discountOff(20000, { kind: 'percent', value: 10 }), 2000);
  assert.equal(discountOff(3333, { kind: 'percent', value: 10 }), 333);
});

test('a fixed discount is taken as given', () => {
  assert.equal(discountOff(20000, { kind: 'amount', value: 500 }), 500);
});

test('no discount means nothing off', () => {
  assert.equal(discountOff(20000, null), 0);
});

test('refuses what the server refuses', () => {
  for (const d of [
    { kind: 'percent', value: 0 },
    { kind: 'percent', value: 100 },
    { kind: 'percent', value: 2.5 },
    { kind: 'amount', value: 0 },
    { kind: 'amount', value: -5 },
    { kind: 'amount', value: 20000 },
    { kind: 'amount', value: 19950 },
    { kind: 'free', value: 5 },
  ]) assert.equal(discountOff(20000, d), null, JSON.stringify(d));
  assert.equal(discountOff(20000, { kind: 'amount', value: 19900 }), 19900);
});

test('a percent that rounds to nothing is refused', () => {
  assert.equal(discountOff(50, { kind: 'percent', value: 1 }), null);
});

test('totals subtract a valid discount and ignore a stale one', () => {
  assert.deepEqual(totals(20000, { kind: 'percent', value: 10 }), { subtotal: 20000, discount: 2000, total: 18000, valid: true });
  assert.deepEqual(totals(150, { kind: 'amount', value: 100 }), { subtotal: 150, discount: 0, total: 150, valid: false });
  assert.deepEqual(totals(20000, null), { subtotal: 20000, discount: 0, total: 20000, valid: true });
});
