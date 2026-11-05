// vbe4pt「extend」—— Nous 自研扩展的清单前端。
//
// 只管一件事：把 `Nous-extend` 仓库里的东西、和正在跑的 qrd3wh 进程里真装着的东西，
// 摆在一页上让人看清。**自己不装、不卸、不碰仓库** —— 类别 / 源码 / 本机装卸都是页面
// POST 给 qrd3wh，由它落盘、撤装、归档、commit/push；这一块只把 public/ 发出去。
//
// 数据不走这个进程：页面自己去问 qrd3wh 的 `GET /v1/extensions`
// （地址从宿主的 `__nous/ctx` 里找，块之间只走 HTTP）。所以这里只剩**发静态文件**。
//
// 这一块以前是 pi CLI 扩展的管理器（对账 / 收编 / 装卸 / 快照 / 出厂检查）。
// 那套跟 pi-durable 无关，2026-11 整条退场；代码在 git 历史里，运行时存档在
// `data/legacy-pi/`（不再被任何代码读）。
//
// 宿主给 PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF，这是一把纯静态服务器，一个都不用。

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const PORT = Number(process.env.PORT ?? 4782);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};

/**
 * 宿主把这一块挂在 `/p/<board>/<id>/` 下面，但进程只认自己那截路径。
 * 不剥这一层，所有请求都是 404。
 */
const stripSelf = (url) => {
  const self = process.env.NOUS_SELF;
  if (self && url.startsWith(self)) {
    const rest = url.slice(self.length);
    return rest.startsWith("/") ? rest : `/${rest}`;
  }
  return url;
};

const send = (res, code, body, type = "text/plain; charset=utf-8") => {
  res.writeHead(code, { "content-type": type, "cache-control": "no-store" });
  res.end(body);
};

const server = createServer(async (req, res) => {
  const raw = stripSelf(req.url ?? "/");
  const pathname = decodeURIComponent(raw.split("?")[0]);
  // 目录 → index.html。尾斜杠由页面自己纠正（见 index.html 顶部那段），这里不管。
  const file = pathname === "/" || pathname === "" ? "index.html" : pathname.replace(/^\/+/, "");

  const target = path.join(PUBLIC, file);
  // 目录穿越：拼完必须还在 public/ 里 —— `..%2f..%2fetc/passwd` 这类请求一律 403。
  if (!target.startsWith(PUBLIC + path.sep)) return send(res, 403, "forbidden");

  try {
    const body = await readFile(target);
    return send(res, 200, body, TYPES[path.extname(target)] ?? "application/octet-stream");
  } catch {
    return send(res, 404, "not found");
  }
});

server.listen(PORT, () => console.log(`[vbe4pt] 听 ${PORT} · 纯静态（数据在页面里问 qrd3wh）`));
