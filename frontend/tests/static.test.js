'use strict';
// Checks on the static site itself, so a broken page or a missing file is caught before deploy.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const root = path.join(__dirname, '..');
const publicDir = path.join(root, 'public');
const staticDir = path.join(publicDir, 'static');
const pages = fs.readdirSync(publicDir).filter((f) => f.endsWith('.html') && f !== 'offline.html');
const staticFiles = fs.readdirSync(staticDir);

const read = (file) => fs.readFileSync(file, 'utf8');

test('there is a page for every route', () => {
  assert.deepEqual(pages.sort(), ['dashboard.html', 'display.html', 'login.html', 'menu.html', 'pos.html', 'queue.html', 'settings.html']);
});

test('every script is valid JavaScript', () => {
  for (const file of staticFiles.filter((f) => f.endsWith('.js'))) {
    assert.doesNotThrow(() => new vm.Script(read(path.join(staticDir, file)), { filename: file }), file);
  }
});

test('pages load only files that exist and leave no template placeholders', () => {
  for (const page of pages) {
    const html = read(path.join(publicDir, page));
    assert.doesNotMatch(html, /__[A-Z]+__/, `${page} still has a server-side placeholder`);
    for (const [, ref] of html.matchAll(/(?:src|href)="(\/static\/[^"?#]+)/g)) {
      assert.ok(fs.existsSync(path.join(publicDir, ref)), `${page} references missing ${ref}`);
    }
  }
});

test('pages that show the shop name load /shop.js before common.js', () => {
  for (const page of pages) {
    const html = read(path.join(publicDir, page));
    const shop = html.indexOf('src="/shop.js"');
    const common = html.indexOf('src="/static/common.js"');
    assert.ok(shop !== -1 && common !== -1, `${page}: needs /shop.js and common.js`);
    assert.ok(shop < common, `${page}: /shop.js must come first`);
  }
});

test('staff pages get their menu rail from the browser', () => {
  for (const [page, active] of [['pos.html', 'pos'], ['settings.html', 'settings'], ['dashboard.html', 'dashboard']]) {
    assert.match(read(path.join(publicDir, page)), new RegExp(`id="nav" data-active="${active}"`), page);
  }
});

test('the web print station is loaded where printing is controlled', () => {
  for (const page of ['pos.html', 'settings.html']) {
    const html = read(path.join(publicDir, page));
    assert.match(html, /\/static\/station\.js/, page);
    assert.ok(html.indexOf('station.js') < html.indexOf(page === 'pos.html' ? 'pos.js' : 'settings.js'), page);
  }
});

test('the Caddy routes point at real pages and send the dynamic paths to the backend', () => {
  const caddy = read(path.join(root, 'Caddyfile'));
  for (const [, target] of caddy.matchAll(/rewrite\s+\S+\s+(\/\S+\.html)/g)) {
    assert.ok(fs.existsSync(path.join(publicDir, target)), `Caddyfile rewrites to missing ${target}`);
  }
  for (const route of ['/api/*', '/events/*', '/img/*', '/shop.js', '/login', '/logout', '/healthz']) {
    assert.ok(caddy.includes(route), `Caddyfile should handle ${route}`);
  }
  assert.match(caddy, /flush_interval -1/, 'live updates (SSE) must not be buffered');
});

test('the all-in-one backend can serve every static file', () => {
  // backend/src/pages.rs embeds the files; a file missing there would 404 in local mode.
  const pagesRs = read(path.join(root, '..', 'backend', 'src', 'pages.rs'));
  for (const file of staticFiles) {
    assert.ok(pagesRs.includes(`"${file}"`), `backend/src/pages.rs does not serve static/${file}`);
  }
});

test('the frontend never talks to a hard-coded backend host', () => {
  // Everything goes through relative URLs (the Caddy proxy / the all-in-one server).
  // example.com is only placeholder text in a form field.
  const absolute = /https?:\/\/(?!fonts\.|[a-z0-9.-]*example\.com)[a-z0-9.-]+\.(?:app|com|io|net)/gi;
  for (const file of staticFiles.filter((f) => f.endsWith('.js'))) {
    const hits = read(path.join(staticDir, file)).match(absolute) || [];
    assert.deepEqual(hits, [], `${file} has an absolute URL`);
  }
});

// ---- installable app (PWA)

const pngSize = (file) => {
  const buf = fs.readFileSync(file);
  assert.equal(buf.subarray(0, 8).toString('hex'), '89504e470d0a1a0a', `${file} is not a PNG`);
  return [buf.readUInt32BE(16), buf.readUInt32BE(20)];
};

test('app icons exist at the sizes the manifest promises', () => {
  const want = { 'icon-192.png': 192, 'icon-512.png': 512, 'icon-maskable-512.png': 512, 'apple-touch-icon.png': 180, 'favicon.png': 48 };
  for (const [file, size] of Object.entries(want)) {
    assert.deepEqual(pngSize(path.join(staticDir, file)), [size, size], file);
  }
});

test('every page can be installed: manifest, touch icon, theme colour', () => {
  for (const page of pages) {
    const html = read(path.join(publicDir, page));
    assert.match(html, /<link rel="manifest" href="\/manifest\.webmanifest">/, `${page}: manifest link`);
    assert.match(html, /rel="apple-touch-icon" href="\/static\/apple-touch-icon\.png"/, `${page}: touch icon`);
    assert.match(html, /name="theme-color"/, `${page}: theme colour`);
  }
});

test('the service worker is valid and never caches live data', () => {
  const source = read(path.join(publicDir, 'sw.js'));
  assert.doesNotThrow(() => new vm.Script(source, { filename: 'sw.js' }));
  assert.match(source, /const LIVE = \['\/api\/', '\/events\/'\]/, 'API and live updates must bypass the cache');
  assert.match(source, /request\.method !== 'GET'/, 'only GET requests may be cached');
  assert.match(source, /OFFLINE_PAGE/);
  const precache = source.match(/const PRECACHE = \[([^\]]+)\]/);
  assert.ok(precache, 'PRECACHE list');
  const files = [...precache[1].matchAll(/'([^']+)'|OFFLINE_PAGE/g)].map((m) => m[1] || '/offline.html');
  for (const file of files) assert.ok(fs.existsSync(path.join(publicDir, file)), `precached ${file} is missing`);
});

test('the offline page needs nothing from the network', () => {
  const html = read(path.join(publicDir, 'offline.html'));
  assert.doesNotMatch(html, /(?:src|href)="https?:/);
  assert.match(html, /ไม่มีสัญญาณ/);
});

test('Caddy serves the manifest from the backend and the worker from the site root', () => {
  const caddy = read(path.join(root, 'Caddyfile'));
  assert.match(caddy, /\/manifest\.webmanifest/);
  assert.match(caddy, /Service-Worker-Allowed/);
  assert.ok(fs.existsSync(path.join(publicDir, 'sw.js')), 'sw.js must sit at the site root');
});
