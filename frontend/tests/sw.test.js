'use strict';
// Runs public/sw.js against a small fake of the service-worker environment (caches, fetch, events),
// so the offline and slow-connection behaviour is checked without a browser.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'public', 'sw.js'), 'utf8');
const ORIGIN = 'http://shop.test';

// Like the real Cache API, a relative address means "on this site".
const urlOf = (request) => (typeof request === 'string' ? new URL(request, ORIGIN).href : request.url);

class FakeCache {
  constructor() { this.entries = new Map(); }
  async match(request) { const hit = this.entries.get(urlOf(request)); return hit ? hit.clone() : undefined; }
  async put(request, response) { this.entries.set(urlOf(request), response); }
  async delete(request) { return this.entries.delete(urlOf(request)); }
  async keys() { return [...this.entries.keys()].map((url) => ({ url })); }
  async add(url) { this.entries.set(ORIGIN + url, await network.fetch(new Request(ORIGIN + url))); }
}

const caches = {
  stores: new Map(),
  slow: 0, // pretend opening the cache takes this many ms (the page reads the body meanwhile)
  async open(name) {
    if (this.slow) await new Promise((resolve) => setTimeout(resolve, this.slow));
    if (!this.stores.has(name)) this.stores.set(name, new FakeCache());
    return this.stores.get(name);
  },
  async match(request) {
    for (const store of this.stores.values()) { const hit = await store.match(request); if (hit) return hit; }
    return undefined;
  },
  async keys() { return [...this.stores.keys()]; },
  async delete(name) { return this.stores.delete(name); },
};

// A pretend network: can be switched off or made very slow, counts requests, and sends a version
// tag (ETag) so a changed file can be told from an unchanged one.
const network = {
  online: true,
  hits: [],
  version: 1,
  date: null, // when set, sent as the Date header (how old the copy is)
  hang: null,
  async fetch(request) {
    const url = new URL(urlOf(request));
    this.hits.push(url.pathname);
    if (!this.online) throw new TypeError('Failed to fetch');
    if (this.hang) await this.hang;
    const headers = { etag: 'v' + this.version };
    if (this.date) headers.date = this.date;
    return new Response('body of ' + url.pathname + ' v' + this.version, { status: 200, headers });
  },
};

const told = [];
const waits = [];
const settle = () => Promise.all(waits.splice(0));

function boot() {
  caches.stores.clear();
  caches.slow = 0;
  network.online = true;
  network.hits = [];
  network.version = 1;
  network.date = null;
  network.hang = null;
  told.length = 0;
  waits.length = 0;
  const handlers = {};
  const self = {
    location: { origin: ORIGIN },
    addEventListener: (type, fn) => { handlers[type] = fn; },
    skipWaiting: async () => {},
    clients: { claim: async () => {}, matchAll: async () => [{ postMessage: (m) => told.push(m) }] },
  };
  vm.runInNewContext(SOURCE, { self, caches, fetch: (r) => network.fetch(r), Request, Response, URL, Promise, Set, setTimeout, clearTimeout, Error });
  return handlers;
}

async function lifecycle(handlers, type) {
  let pending;
  handlers[type]({ waitUntil: (p) => { pending = p; } });
  await pending;
}

/** What the worker answers for a request, or `undefined` when it leaves the request alone. */
async function ask(handlers, url, { method = 'GET', mode = 'cors' } = {}) {
  const request = new Request(url, { method });
  Object.defineProperty(request, 'mode', { value: mode });
  let answer;
  handlers.fetch({ request, respondWith: (p) => { answer = p; }, waitUntil: (p) => { waits.push(p); } });
  return answer && (await answer);
}

test('install keeps the offline page, the till and its scripts for later (not every screen)', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  assert.equal(caches.stores.size, 1, 'one cache, so a refreshed copy is never hidden by an older one');
  const [store] = [...caches.stores.values()];
  const kept = (await store.keys()).map((r) => new URL(r.url).pathname);
  for (const wanted of ['/offline.html', '/pos', '/static/app.css', '/static/pos.js', '/static/common.js']) {
    assert.ok(kept.includes(wanted), wanted + ' missing from ' + kept.join(','));
  }
  assert.ok(!kept.includes('/static/settings.js') && !kept.includes('/dashboard'), 'rarely used screens wait until they are opened');
});

test('activate clears caches left by an older version', async () => {
  const handlers = boot();
  await caches.open('posmini-v0-runtime');
  await lifecycle(handlers, 'install');
  await lifecycle(handlers, 'activate');
  assert.ok(![...caches.stores.keys()].some((name) => name.startsWith('posmini-v0')));
});

test('live data is never touched: API, live updates, other sites and non-GET go to the network', async () => {
  const handlers = boot();
  assert.equal(await ask(handlers, ORIGIN + '/api/orders/active'), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/api/print/station/claim'), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/events/pos'), undefined);
  assert.equal(await ask(handlers, 'https://example.com/tracker.js'), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/login', { method: 'POST' }), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/sw.js'), undefined);
  assert.deepEqual(network.hits, [], 'the worker itself made no requests for any of these');
});

test('a page opens from the network the first time, and from the saved copy when offline', async () => {
  const handlers = boot();
  const online = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await online.text(), 'body of /m/abc v1');
  await settle();
  network.online = false;
  const offline = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await offline.text(), 'body of /m/abc v1', 'last good copy');
});

test('a saved page opens at once even when the connection is very slow', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  let release;
  network.hang = new Promise((resolve) => { release = resolve; });
  const answer = await ask(handlers, ORIGIN + '/pos', { mode: 'navigate' });
  assert.equal(await answer.text(), 'body of /pos v1', 'did not wait for the network');
  release();
  await settle();
});

test('a page is still saved when the page has already read it before the cache opens', async () => {
  const handlers = boot();
  caches.slow = 5;
  const online = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await online.text(), 'body of /m/abc v1'); // the body is used up here
  await settle();
  await new Promise((resolve) => setTimeout(resolve, 30));
  caches.slow = 0;
  network.online = false;
  const offline = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await offline.text(), 'body of /m/abc v1', 'the copy was taken before the body was read');
});

test('a page never seen before shows the offline page when there is no connection', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  network.online = false;
  const answer = await ask(handlers, ORIGIN + '/m/never-opened', { mode: 'navigate' });
  assert.equal(await answer.text(), 'body of /offline.html v1');
});

test('a script is answered from the copy and refreshed behind it; the next visit has the new one', async () => {
  const handlers = boot();
  await ask(handlers, ORIGIN + '/static/zzz.js');
  await settle();
  network.version = 2;
  network.hits = [];
  const first = await ask(handlers, ORIGIN + '/static/zzz.js');
  assert.equal(await first.text(), 'body of /static/zzz.js v1', 'the saved copy, with no waiting');
  await settle();
  assert.deepEqual(network.hits, ['/static/zzz.js'], 'refreshed from the network behind it');
  const second = await ask(handlers, ORIGIN + '/static/zzz.js');
  assert.equal(await second.text(), 'body of /static/zzz.js v2', 'the next visit uses the new file');
});

test('a copy saved a moment ago is used as it is, with no request behind it', async () => {
  const handlers = boot();
  network.date = new Date().toUTCString();
  await ask(handlers, ORIGIN + '/static/zzz.js');
  await settle();
  network.hits = [];
  const again = await ask(handlers, ORIGIN + '/static/zzz.js');
  assert.equal(await again.text(), 'body of /static/zzz.js v1');
  await settle();
  assert.deepEqual(network.hits, [], 'young enough: nothing asked of the network');
  network.date = new Date(Date.now() - 10 * 60 * 1000).toUTCString();
  await ask(handlers, ORIGIN + '/static/old.js');
  await settle();
  network.hits = [];
  await ask(handlers, ORIGIN + '/static/old.js');
  await settle();
  assert.deepEqual(network.hits, ['/static/old.js'], 'an old copy is refreshed behind the answer');
});

test('the open page is told when a newer version of a file arrived', async () => {
  const handlers = boot();
  await ask(handlers, ORIGIN + '/static/zzz.js');
  await settle();
  await ask(handlers, ORIGIN + '/static/zzz.js');
  await settle();
  assert.deepEqual(told, [], 'same file again: nothing to tell');
  network.version = 2;
  await ask(handlers, ORIGIN + '/static/zzz.js');
  await settle();
  assert.deepEqual(JSON.parse(JSON.stringify(told)), [{ type: 'updated' }], 'the page is told once');
});

test('the web fonts are kept so a slow connection does not hold the page up for them', async () => {
  const handlers = boot();
  const css = 'https://fonts.googleapis.com/css2?family=Prompt';
  const first = await ask(handlers, css);
  assert.ok(first, 'the worker handles it');
  await settle();
  network.hang = new Promise(() => {});
  const again = await ask(handlers, css);
  assert.ok(again, 'answered from the copy while the network hangs');
});

test('pictures are served from the cache after the first visit', async () => {
  const handlers = boot();
  await ask(handlers, ORIGIN + '/img/7?v=1');
  await settle();
  await new Promise((resolve) => setTimeout(resolve, 10));
  network.hits = [];
  const again = await ask(handlers, ORIGIN + '/img/7?v=1');
  assert.equal(await again.text(), 'body of /img/7 v1');
  assert.deepEqual(network.hits, []);
});
