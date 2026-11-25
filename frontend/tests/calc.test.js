'use strict';
// The shop calculator's engine (static/calc.js exports it when there is no DOM).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { evaluate, tryEvaluate, pretty, plain, fmt } = require('../public/static/calc.js');

test('arithmetic follows operator precedence', () => {
  for (const [expr, want] of [
    ['2+3×4', 14],
    ['10÷4', 2.5],
    ['7−2−1', 4],
    ['3×3×3', 27],
    ['1÷3×3', 1],
    ['−5+2', -3],
    ['1000−250.5', 749.5],
  ]) assert.equal(evaluate(expr), want, expr);
});

test('decimal sums are exact to 8 places', () => {
  assert.equal(evaluate('0.1+0.2'), 0.3);
});

test('percent works like a shop calculator', () => {
  assert.equal(evaluate('200+10%'), 220);
  assert.equal(evaluate('125−10%'), 112.5);
  assert.equal(evaluate('100×10%'), 10);
  assert.equal(evaluate('50%'), 0.5);
});

test('incomplete or impossible input is reported, not thrown to the UI', () => {
  assert.equal(tryEvaluate('10÷0'), 'div0');
  assert.equal(tryEvaluate('5×'), null);
  assert.equal(tryEvaluate('+3'), null);
  assert.equal(tryEvaluate(''), null);
});

test('numbers are shown with thousands separators', () => {
  assert.equal(pretty('1234567.891'), '1,234,567.891');
  assert.equal(pretty('12+3456×7'), '12+3,456×7');
  assert.equal(pretty('5.'), '5.');
  assert.equal(plain(1234.5), '1234.5');
  assert.equal(plain(-3), '−3');
  assert.equal(fmt(1500), '1,500');
});
