'use strict';
// Runs public/static/station.js against a pretend Bluetooth LE printer, so the phone / tablet
// connection (Web Bluetooth) is checked without a browser or a printer.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'public', 'static', 'station.js'), 'utf8');

/** A printer that accepts at most `maxWrite` bytes per write; `writable` false = nothing to write to. */
function fakePrinter({ maxWrite = 512, writable = true } = {}) {
  const received = [];
  const characteristic = {
    uuid: '00002af1-0000-1000-8000-00805f9b34fb',
    properties: { writeWithoutResponse: writable, write: false },
    async writeValueWithoutResponse(piece) {
      if (piece.length > maxWrite) throw new Error('value too long');
      received.push(Uint8Array.from(piece));
    },
  };
  const service = { uuid: '000018f0-0000-1000-8000-00805f9b34fb', getCharacteristics: async () => [characteristic] };
  const device = {
    id: 'printer-1',
    name: 'AN581-BT',
    listeners: [],
    gatt: { connected: false, async connect() { this.connected = true; return { getPrimaryServices: async () => [service] }; }, disconnect() { this.connected = false; } },
    addEventListener(type, fn) { this.listeners.push([type, fn]); },
    removeEventListener() {},
  };
  return { device, received };
}

function boot(printer) {
  const store = new Map();
  // The station's own idle loop sleeps for seconds forever: those timers must not keep the test alive,
  // but the short pauses a print waits for have to (else Node thinks the print will never finish).
  const timers = (fn, ms) => { const t = setTimeout(fn, ms); if (ms >= 1000) t.unref(); return t; };
  const window = {};
  const context = {
    window,
    navigator: { bluetooth: { async requestDevice() { return printer.device; } } },
    localStorage: { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, String(v)) },
    crypto: { randomUUID: () => 'station-test-id' },
    document: { addEventListener() {}, hidden: false },
    PM: { api: async () => ({}) },
    setTimeout: timers, clearTimeout, atob, Uint8Array, Promise, console, Error, Array, JSON, Math, Date, Map, Set, Number, String, fetch: async () => ({ status: 204 }),
  };
  context.window = context; // the page's `window` is its global object
  vm.runInNewContext(SOURCE, context);
  return context.Station;
}

const bytesOf = (n) => Uint8Array.from({ length: n }, (_, i) => i % 251);
const flatten = (pieces) => Uint8Array.from(pieces.flatMap((p) => [...p]));

test('a phone with Bluetooth LE (and no Web Serial) is offered Bluetooth LE', () => {
  const Station = boot(fakePrinter());
  assert.equal(Station.supported.ble, true);
  assert.equal(Station.supported.serial, false);
  assert.equal(Station.cfg().kind, 'ble', 'the default connection on such a phone');
});

test('a sheet reaches the printer whole and in order, wake-up bytes first', async () => {
  const printer = fakePrinter();
  const Station = boot(printer);
  await Station.connect('ble');
  const sheet = bytesOf(700);
  await Station.printBytes(sheet);
  const sent = flatten(printer.received);
  assert.equal(sent.length, 12 + 700, '12 harmless wake-up bytes, then the sheet');
  assert.deepEqual(sent.subarray(12), sheet);
  assert.ok(printer.received.every((p) => p.length <= 180), 'never more than one big piece');
});

test('when the link takes only short writes the pieces get shorter until they fit', async () => {
  const printer = fakePrinter({ maxWrite: 20 });
  const Station = boot(printer);
  await Station.connect('ble');
  const sheet = bytesOf(130);
  await Station.printBytes(sheet);
  assert.deepEqual(flatten(printer.received).subarray(12), sheet, 'nothing lost or repeated');
  assert.ok(printer.received.every((p) => p.length <= 20));
});

test('a device with nothing to write to says why, instead of silently failing', async () => {
  const Station = boot(fakePrinter({ writable: false }));
  await Station.connect('ble');
  await assert.rejects(() => Station.printBytes(bytesOf(10)), /คลาสสิก/);
});
