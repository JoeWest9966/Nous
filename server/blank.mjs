/**
 * 一个块「还没写代码」时的默认进程。
 *
 * 它是个真进程：自己的端口、自己的 cwd、自己的生命周期，崩了只坏那一页。
 * 它做两件事，没有第三件：
 *
 * 1. 文件夹里的静态文件原样端出去（`index.html`、css、js、图片）。
 * 2. 没有 `index.html` 就给一张空白页 —— 那就是这个块此刻的界面。
 *
 * 为什么不交给壳渲染：一旦宿主能替块画页面，「块 = 独立进程」就变成有条件生效的特权，
 * 而不是铁律。空白页也得是块自己的空白页。
 *
 * 由宿主用 `node blank.mjs <文件夹>` 起来，环境变量跟别的块一样：
 * PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF。
 */
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import http from "node:http";
import path from "node:path";

const DIR = path.resolve(process.argv[2] ?? ".");
const PORT = Number(process.env.PORT || 0);
const SELF = process.env.NOUS_SELF || "";
const ORIGIN = process.env.NOUS_ORIGIN || "";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".gif": "image/gif",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
  ".md": "text/plain; charset=utf-8",
  ".wasm": "application/wasm",
};

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/**
 * 「空白页」就是字面意思：什么都没有。
 *
 * 没有标题、没有说明、没有链接。没开发的东西不该长得像一个页面 —— 它只是还没被写。
 * 只有一件事得留着：底色跟宿主一致，不然切过去像闪了一下白屏。
 * （刷不出来什么也别怀疑：`view-source` 里也是空的，那是对的。）
 */
const blank = () =>
  '<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title></title><style>html{background:#0b0d11}</style>';

const missing = () => `<!doctype html><meta charset="utf-8">
<body style="background:#0b0d11;color:#8b93a7;font:13px ui-monospace,monospace;padding:40px">
404 · 这个块里没有这个文件</body>`;

http
  .createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://x");
    let rel = decodeURIComponent(url.pathname);
    if (rel.endsWith("/")) rel += "index.html";
    // 只认文件夹里的东西；`..` 和绝对路径一律按「没有」处理。
    const file = path.resolve(DIR, `.${path.posix.normalize(rel)}`);
    if (!file.startsWith(DIR + path.sep)) {
      res.writeHead(403, { "content-type": "text/plain; charset=utf-8" }).end("不在这个块里");
      return;
    }
    stat(file)
      .then((st) => {
        if (!st.isFile()) throw new Error("not a file");
        res.writeHead(200, {
          "content-type": TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream",
          "content-length": st.size,
          "cache-control": "no-cache",
        });
        createReadStream(file).pipe(res);
      })
      .catch(() => {
        // 没有 index.html → 这块的界面就是空白页。别的都算 404。
        if (rel === "index.html" || rel === "/index.html") {
          res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(blank());
          return;
        }
        res.writeHead(404, { "content-type": "text/html; charset=utf-8" }).end(missing());
      });
  })
  .listen(PORT, () => console.log(`empty block on ${PORT} · ${DIR}`));
