// Service worker: makes the site installable and keeps the app shell usable when the network drops.
//
// Pages, scripts and styles are fetched from the network first, so every file of a screen comes
// from the same version (an old script with a new page breaks buttons). When the connection is
// down, or slow beyond NETWORK_WAIT_MS, the last saved copy is used instead. Product pictures
// and the web fonts are served from the saved copy. Live data is never kept: /api/ and /events/
// always go straight to the server, so a stale queue or payment is impossible. The till page
// itself is saved, so cash sales can go on without a connection (see offlineq.js).
const VERSION = 'posmini-v3';
const APP = VERSION + '-app';
const OFFLINE_PAGE = '/offline.html';
const STATIC = ['app.css', 'common.js', 'calc.js', 'cartmath.js', 'offlineq.js', 'station.js', 'pos.js', 'icon-192.png'];
const PAGES = ['/pos'];
const PRECACHE = [OFFLINE_PAGE, ...PAGES, ...STATIC.map((name) => '/static/' + name)];
const LIVE = ['/api/', '/events/'];
const FONT_HOSTS = ['fonts.googleapis.com', 'fonts.gstatic.com'];
const NETWORK_WAIT_MS = 3500;
const MAX_ENTRIES = 120;

self.addEventListener('install', (event) => {
  // One missing file must not stop the install, so each is added on its own.
  event.waitUntil((async () => {
    const cache = await caches.open(APP);
    await Promise.all(PRECACHE.map((url) => cache.add(url).catch(() => { /* saved on first use instead */ })));
    await self.skipWaiting();
  })());
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
// the response: cloning later fails with "body is already used". Only complete answers are kept
// (not a 206 or an error); an opaque one (a cross-origin stylesheet) only when `allowOpaque`.
function remember(request, response, allowOpaque) {
  if (!response) return;
  if (response.status !== 200 && !(allowOpaque && response.type === 'opaque')) return;
  const copy = response.clone();
  (async () => {
    const cache = await caches.open(APP);
    await cache.put(request, copy);
    const keys = await cache.keys();
    if (keys.length > MAX_ENTRIES) {
      const keep = new Set(PRECACHE.map((path) => new URL(path, self.location.origin).href));
      const old = keys.find((key) => !keep.has(key.url));
      if (old) await cache.delete(old);
    }
  })().catch(() => { /* a copy that cannot be saved just means no offline copy of this one */ });
}

function withTimeout(promise, ms) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timeout')), ms);
    promise.then((value) => { clearTimeout(timer); resolve(value); }, (err) => { clearTimeout(timer); reject(err); });
  });
}

// Network first; if it fails (or is very slow) and there is a saved copy, use that. A slow
// answer that arrives afterwards is still saved for next time.
async function networkFirst(event, fallbackPage) {
  const { request } = event;
  const cached = await caches.match(request);
  const fresh = fetch(request).then((response) => { remember(request, response); return response; });
  try {
    return await (cached ? withTimeout(fresh, NETWORK_WAIT_MS) : fresh);
  } catch (err) {
    if (cached) {
      event.waitUntil(fresh.catch(() => { /* offline: the saved copy is all there is */ }));
      return cached;
    }
    const page = fallbackPage && await caches.match(fallbackPage);
    if (page) return page;
    throw err;
  }
}

async function cacheFirst(request, allowOpaque) {
  const cached = await caches.match(request);
  if (cached) return cached;
  const response = await fetch(request);
  remember(request, response, allowOpaque);
  return response;
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (FONT_HOSTS.includes(url.hostname)) {
    event.respondWith(cacheFirst(request, true));
    return;
  }
  if (url.origin !== self.location.origin) return;
  if (LIVE.some((prefix) => url.pathname.startsWith(prefix))) return;
  if (url.pathname === '/sw.js') return;

  if (request.mode === 'navigate') {
    event.respondWith(networkFirst(event, OFFLINE_PAGE));
  } else if (url.pathname.startsWith('/img/') || /\.(png|jpg|jpeg|webp|svg|ico)$/.test(url.pathname)) {
    event.respondWith(cacheFirst(request));
  } else {
    event.respondWith(networkFirst(event));
  }
});
