// Service worker of "Mobbed!": makes the game installable (PWA) and opens
// the shell without a network round trip on a flaky connection. The game is
// online-only, so this is deliberately small:
//  - the page and its scripts: network first, the cached copy only offline
//    (a deploy is picked up on the next load, never a stale build). The
//    network fetch goes through the browser's HTTP cache, so the server's
//    policy holds: the page revalidates (ETag, a cheap 304), versioned
//    scripts are reused for their max-age;
//  - brand images (icons, logo): the cached copy at once, refreshed in the
//    background (stale-while-revalidate, again through the HTTP cache);
//  - everything else (/api, the WebSocket, /assets packs, other hosts):
//    not touched; the browser's HTTP cache handles the packs
//    (server/src/api/static_files.rs).
const VERSION = 'mobbed-v1';
const SHELL = ['/', '/index.html', '/main.dart.js', '/assets/engine.bundle.js', '/manifest.webmanifest'];
const BRAND = /^\/(favicon\.(svg|ico)|favicon-32\.png|apple-touch-icon\.png|icon-(192|512|maskable-512)\.png|logo\.webp|og\.jpg)$/;

self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(VERSION).then((c) => c.addAll(SHELL)).catch(() => {}).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== VERSION).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  const path = url.pathname;
  if (BRAND.test(path)) {
    const fresh = fetchAndKeep(req);
    event.waitUntil(fresh.catch(() => {}));
    event.respondWith(caches.match(req).then((hit) => hit || fresh));
  } else if (req.mode === 'navigate' || SHELL.includes(path)) {
    event.respondWith(fetchAndKeep(req).catch(() => caches.match(req).then((hit) => hit || caches.match('/'))));
  }
});

async function fetchAndKeep(req) {
  const res = await fetch(req);
  if (res.ok) {
    const copy = res.clone();
    caches.open(VERSION).then((c) => c.put(req, copy));
  }
  return res;
}
