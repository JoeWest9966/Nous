// 壳的离线缓存。四条规矩：
//   1. /api/ 和 /__nous/ 一律不过缓存 —— 前者是活的会话，后者是宿主的文档页。
//      缓存住它俩的后果是「看到旧文档还以为块坏了」。
//   2. 壳走 network-first：宿主一改文件就把进程收了，所以联网时永远拿最新的；
//      只有断网才回落到缓存。反过来（cache-first）会让用户卡在一个旧版本里，改十次都看不到。
//   3. 只缓存自己这个路径底下的东西。
//   4. 不在安全上下文里根本不会注册（http://IP 下浏览器不给 SW）。

const CACHE = "bjes6b-v1";
const SHELL = ["./", "./index.html", "./app.css", "./app.js", "./manifest.webmanifest", "./icon-192.png", "./icon-512.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (url.origin !== location.origin) return;
  if (url.pathname.includes("/api/") || url.pathname.includes("/__nous/")) return;

  event.respondWith(
    fetch(request)
      .then((response) => {
        if (response.ok) {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put(request, copy));
        }
        return response;
      })
      .catch(() => caches.match(request).then((hit) => hit ?? Response.error())),
  );
});
