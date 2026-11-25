'use strict';
// Runs public/sw.js against a small fake of the service-worker environment (caches, fetch, events),
// so the offline behaviour is checked without a browser.
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
  async addAll(urls) { for (const url of urls) this.entries.set(ORIGIN + url, await network.fetch(new Request(ORIGIN + url))); }
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

// A pretend network: knows which paths exist, can be switched off, and counts requests.
const network = {
  online: true,
  hits: [],
  async fetch(request) {
    const url = new URL(urlOf(request));
    this.hits.push(url.pathname);
    if (!this.online) throw new TypeError('Failed to fetch');
    return new Response(`body of ${url.pathname}`, { status: 200 });
  },
};

function boot() {
  caches.stores.clear();
  caches.slow = 0;
  network.online = true;
  network.hits = [];
  const handlers = {};
  const self = {
    location: { origin: ORIGIN },
    addEventListener: (type, fn) => { handlers[type] = fn; },
    skipWaiting: async () => {},
    clients: { claim: async () => {} },
  };
  vm.runInNewContext(SOURCE, { self, caches, fetch: (r) => network.fetch(r), Request, Response, URL, Promise, setTimeout, clearTimeout, Error });
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
  handlers.fetch({ request, respondWith: (p) => { answer = p; } });
  return answer && (await answer);
}

test('install keeps the offline page and the app shell for later', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  const shell = [...caches.stores.keys()].find((name) => name.endsWith('-shell'));
  const kept = (await caches.stores.get(shell).keys()).map((r) => new URL(r.url).pathname);
  assert.ok(kept.includes('/offline.html') && kept.includes('/static/app.css'), kept.join(','));
});

test('activate clears caches left by an older version', async () => {
  const handlers = boot();
  await caches.open('posmini-v0-runtime');
  await lifecycle(handlers, 'install');
  await lifecycle(handlers, 'activate');
  assert.ok(![...caches.stores.keys()].some((name) => name.startsWith('posmini-v0')));
});

test('live data is never touched: API, live updates, other origins and non-GET go to the network', async () => {
  const handlers = boot();
  assert.equal(await ask(handlers, ORIGIN + '/api/orders/active'), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/api/print/station/claim'), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/events/pos'), undefined);
  assert.equal(await ask(handlers, 'https://fonts.googleapis.com/css2?family=Prompt'), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/login', { method: 'POST' }), undefined);
  assert.equal(await ask(handlers, ORIGIN + '/sw.js'), undefined);
  assert.deepEqual(network.hits, [], 'the worker itself made no requests for any of these');
});

test('a page opens from the network when online, and from the saved copy when offline', async () => {
  const handlers = boot();
  const online = await ask(handlers, ORIGIN + '/pos', { mode: 'navigate' });
  assert.equal(await online.text(), 'body of /pos');
  await new Promise((resolve) => setTimeout(resolve, 10)); // the copy is saved in the background
  network.online = false;
  const offline = await ask(handlers, ORIGIN + '/pos', { mode: 'navigate' });
  assert.equal(await offline.text(), 'body of /pos', 'last good copy');
});

test('a page is still saved when the page has already read it before the cache opens', async () => {
  const handlers = boot();
  caches.slow = 5;
  const online = await ask(handlers, ORIGIN + '/pos', { mode: 'navigate' });
  assert.equal(await online.text(), 'body of /pos'); // the body is used up here
  await new Promise((resolve) => setTimeout(resolve, 30));
  caches.slow = 0;
  network.online = false;
  const offline = await ask(handlers, ORIGIN + '/pos', { mode: 'navigate' });
  assert.equal(await offline.text(), 'body of /pos', 'the copy was taken before the body was read');
});

test('a page never seen before shows the offline page when there is no connection', async () => {
  const handlers = boot();
  await lifecycle(handlers, 'install');
  network.online = false;
  const answer = await ask(handlers, ORIGIN + '/dashboard', { mode: 'navigate' });
  assert.equal(await answer.text(), 'body of /offline.html');
});

test('scripts are fetched fresh first, so an update is never hidden by an old copy', async () => {
  const handlers = boot();
  await ask(handlers, ORIGIN + '/static/pos.js');
  await new Promise((resolve) => setTimeout(resolve, 10));
  network.hits = [];
  await ask(handlers, ORIGIN + '/static/pos.js');
  assert.deepEqual(network.hits, ['/static/pos.js'], 'asked the network again even though a copy exists');
});

test('pictures are served from the cache after the first visit', async () => {
  const handlers = boot();
  await ask(handlers, ORIGIN + '/img/7?v=1');
  await new Promise((resolve) => setTimeout(resolve, 10));
  network.hits = [];
  const again = await ask(handlers, ORIGIN + '/img/7?v=1');
  assert.equal(await again.text(), 'body of /img/7');
  assert.deepEqual(network.hits, []);
});
