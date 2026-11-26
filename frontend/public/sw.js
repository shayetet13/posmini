// Service worker: makes the site installable and keeps the app usable on a weak connection.
//
// Pages, scripts and styles are answered from the saved copy at once and refreshed from the
// network in the background, so moving between screens never waits on the connection. A newer
// copy is used from the next visit on, and the open page is told that one arrived. The first
// visit (nothing saved yet) goes to the network. Product pictures and the web fonts are kept too.
// Live data never is: /api/ and /events/ always go straight to the server, so a stale queue or
// payment is impossible. The till page itself is kept, so cash sales can go on without a
// connection (see offlineq.js).
const VERSION = 'posmini-v2';
const APP = VERSION + '-app';
const OFFLINE_PAGE = '/offline.html';
const PAGES = ['/pos', '/dashboard', '/settings', '/login'];
const STATIC = ['app.css', 'common.js', 'calc.js', 'cartmath.js', 'offlineq.js', 'station.js', 'pos.js', 'dashboard.js', 'settings.js', 'icon-192.png'];
const PRECACHE = [OFFLINE_PAGE, ...PAGES, ...STATIC.map((name) => '/static/' + name)];
const LIVE = ['/api/', '/events/'];
const FONT_HOSTS = ['fonts.googleapis.com', 'fonts.gstatic.com'];
const MAX_ENTRIES = 120;

self.addEventListener('install', (event) => {
  // One missing file must not stop the install, so each is added on its own.
  event.waitUntil((async () => {
    const cache = await caches.open(APP);
    await Promise.all(PRECACHE.map((url) => cache.add(url).catch(() => { /* fetched on first use instead */ })));
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

// Saves a copy. The copy is taken right now, before the page starts reading the response:
// cloning later fails with "body is already used". Only complete answers are kept (not a 206
// or an error); an opaque one (a cross-origin stylesheet) is kept only when `allowOpaque`.
function remember(request, response, allowOpaque) {
  if (!response) return undefined;
  const keep = response.status === 200 || (allowOpaque && response.type === 'opaque');
  if (!keep) return undefined;
  const copy = response.clone();
  return (async () => {
    const cache = await caches.open(APP);
    await cache.put(request, copy);
    const keys = await cache.keys();
    if (keys.length > MAX_ENTRIES) {
      const keepUrls = new Set(PRECACHE.map((path) => new URL(path, self.location.origin).href));
      const old = keys.find((key) => !keepUrls.has(key.url));
      if (old) await cache.delete(old);
    }
  })().catch(() => { /* a copy that cannot be saved just means no offline copy of this one */ });
}

async function tellPagesAboutUpdate() {
  for (const client of await self.clients.matchAll({ type: 'window' })) client.postMessage({ type: 'updated' });
}

// The saved copy now, the network copy for next time. Without a saved copy the network decides,
// and if that fails too the offline page stands in for a screen.
function staleWhileRevalidate(event, fallbackPage, allowOpaque) {
  const { request } = event;
  const work = []; // saving the copy and the notice finish after the page already has its answer
  const refresh = fetch(request).then(async (response) => {
    const before = await caches.match(request);
    const oldTag = before && before.headers.get('etag');
    const newTag = response.headers.get('etag');
    if (oldTag && newTag && oldTag !== newTag) work.push(tellPagesAboutUpdate());
    work.push(remember(request, response, allowOpaque));
    return response;
  });
  event.waitUntil(refresh.then(() => Promise.all(work), () => { /* offline: the saved copy is all there is */ }));
  return caches.match(request).then((cached) => cached || refresh.catch(async (err) => {
    const page = fallbackPage && await caches.match(fallbackPage);
    if (page) return page;
    throw err;
  }));
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
  if (FONT_HOSTS.includes(url.hostname)) {
    event.respondWith(staleWhileRevalidate(event, null, true));
    return;
  }
  if (url.origin !== self.location.origin) return;
  if (LIVE.some((prefix) => url.pathname.startsWith(prefix))) return;
  if (url.pathname === '/sw.js') return;

  if (request.mode === 'navigate') {
    event.respondWith(staleWhileRevalidate(event, OFFLINE_PAGE, false));
  } else if (url.pathname.startsWith('/img/') || /\.(png|jpg|jpeg|webp|svg|ico)$/.test(url.pathname)) {
    event.respondWith(cacheFirst(request));
  } else {
    event.respondWith(staleWhileRevalidate(event, null, false));
  }
});
