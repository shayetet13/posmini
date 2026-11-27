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

const waits = [];
const settle = () => Promise.all(waits.splice(0));

const timers = [];
const fire = () => timers.splice(0).forEach((fn) => fn());

function boot() {
  caches.stores.clear();
  caches.slow = 0;
  network.online = true;
  network.hits = [];
  network.version = 1;
  network.date = null;
  network.hang = null;
  waits.length = 0;
  timers.length = 0;
  const handlers = {};
  const self = {
    location: { origin: ORIGIN },
    addEventListener: (type, fn) => { handlers[type] = fn; },
    skipWaiting: async () => {},
    clients: { claim: async () => {} },
  };
  vm.runInNewContext(SOURCE, { self, caches, fetch: (r) => network.fetch(r), Request, Response, URL, Promise, Set, setTimeout: (fn) => timers.push(fn), clearTimeout: () => {}, Error });
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

test('a page opens from the network when online, and from the saved copy when offline', async () => {
  const handlers = boot();
  const online = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await online.text(), 'body of /m/abc v1');
  await new Promise((resolve) => setTimeout(resolve, 10)); // the copy is saved in the background
  network.online = false;
  const offline = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await offline.text(), 'body of /m/abc v1', 'last good copy');
});

test('pages and scripts are asked from the network first, so old and new files never mix', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  network.version = 2;
  const page = await ask(handlers, ORIGIN + '/pos', { mode: 'navigate' });
  const script = await ask(handlers, ORIGIN + '/static/pos.js');
  assert.equal(await page.text(), 'body of /pos v2', 'not the copy saved at install');
  assert.equal(await script.text(), 'body of /static/pos.js v2');
});

test('a connection too slow to wait for falls back to the saved copy, and saves the late answer', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  network.version = 2;
  let release;
  network.hang = new Promise((resolve) => { release = resolve; });
  const pending = ask(handlers, ORIGIN + '/static/pos.js');
  await new Promise((resolve) => setTimeout(resolve, 20));
  fire(); // the waiting limit passes
  const answer = await pending;
  assert.equal(await answer.text(), 'body of /static/pos.js v1', 'the saved copy');
  release();
  await settle();
  await new Promise((resolve) => setTimeout(resolve, 10));
  network.online = false;
  const later = await ask(handlers, ORIGIN + '/static/pos.js');
  assert.equal(await later.text(), 'body of /static/pos.js v2', 'the late answer was kept');
});

test('a page is still saved when the page has already read it before the cache opens', async () => {
  const handlers = boot();
  caches.slow = 5;
  const online = await ask(handlers, ORIGIN + '/m/abc', { mode: 'navigate' });
  assert.equal(await online.text(), 'body of /m/abc v1'); // the body is used up here
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

test('the web fonts are served from the saved copy', async () => {
  const handlers = boot();
  const css = 'https://fonts.googleapis.com/css2?family=Prompt';
  assert.ok(await ask(handlers, css), 'the worker handles it');
  await new Promise((resolve) => setTimeout(resolve, 10));
  network.hits = [];
  assert.ok(await ask(handlers, css));
  assert.deepEqual(network.hits, [], 'no request the second time');
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
