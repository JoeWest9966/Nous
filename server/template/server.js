/**
 * 一个块的起点。零依赖，`node server.js` 就能跑 —— 你现在看到的就是它。
 *
 * 三条规矩，宿主之外也成立：
 *
 * - 听 `PORT`（宿主给的；自己跑时给个默认值）。
 * - 数据写 `NOUS_DATA`（宿主里 = 这个文件夹下的 `data/`；自己跑 = `./data`）。
 *   数据在你自己的文件夹里，`cp -r` 走就带走了。
 * - 页面里的地址写**相对路径**（`api/health` 而不是 `/api/health`）——
 *   这样它在宿主的前缀底下、在你自己的域名底下、在 `localhost:5055` 底下都是同一页。
 */
import { createReadStream } from "node:fs";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DIR = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(DIR, "public");
const DATA = process.env.NOUS_DATA || path.join(DIR, "data");
const PORT = Number(process.env.PORT || 5055);
/** 我在宿主里叫什么（`/p/<board>/<id>`）。自己跑时是空的。 */
const SELF = process.env.NOUS_SELF || "";

const STORE = path.join(DATA, "notes.json");

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
};

const readNotes = async () => {
  try {
    const parsed = JSON.parse(await readFile(STORE, "utf8"));
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
};

const writeNotes = async (notes) => {
  await mkdir(DATA, { recursive: true });
  await writeFile(STORE, `${JSON.stringify(notes, null, 2)}\n`, "utf8");
};

const json = (res, status, body) => {
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
};

const body = async (req) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  if (!chunks.length) return {};
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return null;
  }
};

/** API 都在这儿。加路由就往这张表里加，别在下面写 if 长龙。 */
const API = {
  "GET /api/health": async (req, res) =>
    json(res, 200, { ok: true, self: SELF, id: path.basename(DIR), data: DATA, now: new Date().toISOString() }),

  "GET /api/notes": async (req, res) => json(res, 200, { notes: await readNotes() }),

  "POST /api/notes": async (req, res) => {
    const input = await body(req);
    const text = typeof input?.text === "string" ? input.text.trim() : "";
    if (!text) return json(res, 400, { error: "text required" });
    const notes = await readNotes();
    const note = { id: Math.random().toString(36).slice(2, 8), text, at: new Date().toISOString() };
    notes.push(note);
    await writeNotes(notes);
    return json(res, 201, { note });
  },

  "DELETE /api/notes": async (req, res) => {
    await writeNotes([]);
    return json(res, 200, { ok: true });
  },
};

http
  .createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://x");
    const key = `${req.method} ${url.pathname}`;

    if (url.pathname.startsWith("/api/")) {
      const route = API[key];
      if (!route) return json(res, 404, { error: "no such route", route: key });
      try {
        return await route(req, res);
      } catch (err) {
        return json(res, 500, { error: err instanceof Error ? err.message : String(err) });
      }
    }

    // 其余都是静态文件：public/ 里有什么就端什么，`/` 是 index.html。
    let rel = decodeURIComponent(url.pathname);
    if (rel.endsWith("/")) rel += "index.html";
    const file = path.resolve(PUBLIC, `.${path.posix.normalize(rel)}`);
    if (!file.startsWith(PUBLIC + path.sep)) {
      res.writeHead(403, { "content-type": "text/plain; charset=utf-8" }).end("no");
      return;
    }
    try {
      const st = await stat(file);
      if (!st.isFile()) throw new Error("not a file");
      res.writeHead(200, {
        "content-type": TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream",
        "content-length": st.size,
        "cache-control": "no-cache",
      });
      createReadStream(file).pipe(res);
    } catch {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" }).end("404");
    }
  })
  .listen(PORT, () => {
    console.log(`block ${SELF || DIR} listening on ${PORT}`);
  });
