// Keeps the app shell available offline. API answers always come from the network:
// an old departure board shown as current is worse than none.

const SHELL = 'odjazd-shell-v1';
const LIBS = 'odjazd-libs-v1';

const SHELL_FILES = [
  '/',
  '/style.css',
  '/app.js',
  '/api.js',
  '/format.js',
  '/map.js',
  '/manifest.webmanifest',
  '/icons/icon.svg',
  '/icons/icon-192.png',
  '/icons/apple-touch-icon.png',
];

// Versioned URLs, so they never change: cache first.
const LIB_FILES = [
  'https://unpkg.com/leaflet@1.9.4/dist/leaflet.js',
  'https://unpkg.com/leaflet@1.9.4/dist/leaflet.css',
];

self.addEventListener('install', (event) => {
  event.waitUntil((async () => {
    await (await caches.open(SHELL)).addAll(SHELL_FILES);
    const libs = await caches.open(LIBS);
    await Promise.all(LIB_FILES.map((url) => libs.add(new Request(url, { mode: 'cors' })).catch(() => {})));
    await self.skipWaiting();
  })());
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) {
      if (key !== SHELL && key !== LIBS) await caches.delete(key);
    }
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin === self.location.origin) {
    if (!url.pathname.startsWith('/api/')) event.respondWith(networkFirst(request));
  } else if (LIB_FILES.includes(url.href)) {
    event.respondWith(caches.match(request).then((hit) => hit ?? fetch(request)));
  }
});

/** The app's own files: fresh when online, from the cache when not. */
async function networkFirst(request) {
  const cache = await caches.open(SHELL);
  // Every page of the app is the same document, whatever its query (`?at=`) or hash.
  const key = request.mode === 'navigate' ? '/' : request;
  try {
    const response = await fetch(request);
    if (response.ok) await cache.put(key, response.clone());
    return response;
  } catch {
    return (await cache.match(key, { ignoreSearch: true })) ?? Response.error();
  }
}
