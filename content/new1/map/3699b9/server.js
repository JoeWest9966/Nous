// 3699b9「New-Template」—— Nous 会话预设私库的管理页。
//
// 只管一件事：把 `Nous-prompts` 仓库里现在有哪些模板，摆成一页卡片让人看清、改正文、发布。
// **自己不读仓库、不跑 git、不改文件** —— 类别、正文、发布、归档全是页面 POST 给
// qrd3wh 的，由它写盘、commit、push；这一块只把 public/ 发出去。
//
// 数据不走这个进程：页面自己去问 qrd3wh 的 `GET /v1/prompts`，地址从宿主的
// `__nous/ctx` 里找（块之间只走 HTTP）。所以这里只剩**发静态文件**。
//

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const PORT = Number(process.env.PORT ?? 4786);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};

/**
 * 宿主把这一块挂在 `NOUS_SELF` 那个前缀下面，但进程只认自己那截路径。
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

// 这一块没有后端状态：没有 data/ 要写、没有 git 要跑、没有进程要管。
// 模板仓库的读写全在 qrd3wh 那一个入口，这一页只是它的视图。
server.listen(PORT, () => console.log(`[3699b9] 听 ${PORT} · 纯静态（模板数据在页面里问 qrd3wh）`));
