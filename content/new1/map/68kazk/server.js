// 68kazk「skills」—— Nous 自持技能的清单面板。
//
// 只管一件事：把 `Nous-skills` 仓库里现在有哪些技能，摆成一页卡片让人看清。
// **自己不读仓库、不跑 git、不改文件** —— 类别、正文、发布、归档全是页面 POST 给
// qrd3wh 的，由它写盘、commit、push；这一块只把 public/ 发出去。
//
// 地址是真路径，不是 hash：`/` 是技能清单（这一块的正身），`/upstream` 是上游那块视图，
// `/upstream/<订阅>` 顺带选中那条订阅，`/pending` 是未同步的改动。所以这里除了发文件，
// 还要认「没扩展名的路径 = 路由」，把它也发成 index.html。
//
// 数据不走这个进程：页面自己去问 qrd3wh 的 `GET /v1/skills`，地址从宿主的
// `__nous/ctx` 里找（块之间只走 HTTP）。所以这里只剩**发静态文件**。
//
// 上一版是「技能编目」的店面：反代到上游编目服务、Bearer token、源卡 + 对账报告 +
// 版本比对 + 重新登记。上游编目整个退场（技能变成我们自己仓库里的文件），
// 那套代码在 git 历史里。

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const PORT = Number(process.env.PORT ?? 4784);

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

/** 这一块在宿主里的根：`/p/new1/68kazk`（没有尾斜杠）。没挂宿主时是空串。 */
const MOUNT = (process.env.NOUS_SELF ?? "").replace(/\/+$/, "");

/**
 * index.html 发出去之前钉两条钉子 —— 深路径下这两件事都会算错地方：
 *
 *   · `<base href="{mount}/">`：`./style.css` / `./app.js` 从**块根**算，不是从 `/upstream` 算；
 *   · `window.__nous_self`：页面问 `__nous/ctx`（找 qrd3wh 的地址）也得从块根算，
 *     不然在 `/upstream` 上会去问 `/upstream/__nous/ctx`。
 *
 * 这是「页面的地址」和「块的地址」不是一回事带来的，不是临时补丁：一屏一个真路径，
 * 块的位置只有一个。
 */
const page = async () => {
  const html = await readFile(path.join(PUBLIC, "index.html"), "utf8");
  if (!MOUNT) return html;
  const base = `<base href="${MOUNT}/" />`;
  const self = `<script>window.__nous_self = ${JSON.stringify(`${MOUNT}/`)};</script>`;
  return html.replace(/<head>/i, `<head>\n    ${base}\n    ${self}`);
};

const server = createServer(async (req, res) => {
  const raw = stripSelf(req.url ?? "/");
  const pathname = decodeURIComponent(raw.split("?")[0]);
  // 目录 → index.html。尾斜杠不用管：资源靠注入的 `<base>` 定位，不是靠尾斜杠。
  const file = pathname === "/" || pathname === "" ? "index.html" : pathname.replace(/^\/+/, "");

  const target = path.join(PUBLIC, file);
  // 目录穿越：拼完必须还在 public/ 里 —— `..%2f..%2fetc/passwd` 这类请求一律 403。
  if (!target.startsWith(PUBLIC + path.sep)) return send(res, 403, "forbidden");

  if (!path.extname(target)) {
    // 没有扩展名 = 一条**路由**，不是文件：`/`、`/upstream`、`/upstream/matt`、`/pending`
    // 全发 index.html，由页面看 `location.pathname` 决定画哪一屏。
    // 带扩展名的照旧走磁盘、找不到就 404 —— 把 `style.css` 回退成 HTML 只会让浏览器
    // 报一句 MIME 错，比 404 难查一百倍。
    return send(res, 200, await page(), TYPES[".html"]);
  }

  try {
    const body = await readFile(target);
    return send(res, 200, body, TYPES[path.extname(target)] ?? "application/octet-stream");
  } catch {
    return send(res, 404, "not found");
  }
});

// 这一块没有后端状态：没有 data/ 要写、没有 git 要跑、没有进程要管。
// 技能仓库的读写全在 qrd3wh 那一个入口（spec I4），这一页只是它的视图。
server.listen(PORT, () => console.log(`[68kazk] 听 ${PORT} · 纯静态（技能数据在页面里问 qrd3wh）`));
