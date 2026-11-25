// Service worker: makes the site installable and keeps the app shell usable when the network drops.
//
// Pages, scripts and styles are fetched from the network first (so an update shows up at once)
// and fall back to the last copy when offline. Product pictures are cached. Live data never is:
// /api/ and /events/ always go straight to the server, so a stale queue or payment is impossible.
// The till page itself is kept too, so cash sales can go on without a connection (see offlineq.js).
const VERSION = 'posmini-v1';
const SHELL = VERSION + '-shell';
const RUNTIME = VERSION + '-runtime';
const OFFLINE_PAGE = '/offline.html';
const PRECACHE = [OFFLINE_PAGE, '/static/app.css', '/static/common.js', '/static/icon-192.png'];
const LIVE = ['/api/', '/events/'];
const NETWORK_WAIT_MS = 6000;
const MAX_RUNTIME_ENTRIES = 80;

self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(SHELL).then((cache) => cache.addAll(PRECACHE)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    for (const name of await caches.keys()) {
      if (!name.startsWith(VERSION)) await caches.delete(name);
    }
    await self.clients.claim();
  })());
});

// Saves a copy in the background. The copy is taken right now, before the page starts reading
// the response: cloning later (after opening the cache) fails with "body is already used".
// Only complete answers are kept; a partial (206) one cannot be stored.
function remember(request, response) {
  if (!response || response.status !== 200 || response.type === 'opaque') return;
  const copy = response.clone();
  (async () => {
    const cache = await caches.open(RUNTIME);
    await cache.put(request, copy);
    const keys = await cache.keys();
    if (keys.length > MAX_RUNTIME_ENTRIES) await cache.delete(keys[0]);
  })().catch(() => { /* a copy that cannot be saved just means no offline copy of this one */ });
}

function withTimeout(promise, ms) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timeout')), ms);
    promise.then((value) => { clearTimeout(timer); resolve(value); }, (err) => { clearTimeout(timer); reject(err); });
  });
}

// Network first; if it fails (or is very slow) and there is a saved copy, use that.
async function networkFirst(request, fallbackPage) {
  try {
    const cached = await caches.match(request);
    const response = await (cached ? withTimeout(fetch(request), NETWORK_WAIT_MS) : fetch(request));
    remember(request, response);
    return response;
  } catch (err) {
    const cached = await caches.match(request);
    if (cached) return cached;
    if (fallbackPage) {
      const page = await caches.match(fallbackPage);
      if (page) return page;
    }
    throw err;
  }
}

async function cacheFirst(request) {
  const cached = await caches.match(request);
  if (cached) return cached;
  const response = await fetch(request);
  remember(request, response);
  return response;
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  if (LIVE.some((prefix) => url.pathname.startsWith(prefix))) return;
  if (url.pathname === '/sw.js') return;

  if (request.mode === 'navigate') {
    event.respondWith(networkFirst(request, OFFLINE_PAGE));
  } else if (url.pathname.startsWith('/img/') || /\.(png|jpg|jpeg|webp|svg|ico)$/.test(url.pathname)) {
    event.respondWith(cacheFirst(request));
  } else {
    event.respondWith(networkFirst(request));
  }
});
