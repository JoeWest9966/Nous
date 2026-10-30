/**
 * skill 编目面板 · 后端。
 *
 * 它不做业务：所有读写都经 nw-panel 的 REST API。这个进程只干四件事：
 *
 * 1. 反代 /api/nw/* → <warehouse>/api/*（浏览器因此零外呼、零 CORS，
 *    备用入口的 Bearer token 也只活在这个进程的内存里，不进页面）。
 * 2. 发 public/ 的静态文件（相对路径，`cp -r` 走了也是同一页）。
 * 3. 存一个东西：warehouse 的 base（+可选 token），落 NOUS_DATA/config.json。
 * 4. 查上游最新 semver tag（git ls-remote，落 NOUS_DATA/upstream.json）。
 *    **只查不写编目** —— 写账还是 register 两段式那一道闸。
 *
 * 环境变量就宿主给的那四个，外加一个可选的 NW_PANEL_BASE 当默认值。
 */
import http from "node:http";
import { execFile } from "node:child_process";
import { createReadStream } from "node:fs";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const DATA = process.env.NOUS_DATA || path.join(HERE, "data");
const CONFIG = path.join(DATA, "config.json");
const PORT = Number(process.env.PORT || 8080);
const DEFAULT_BASE = process.env.NW_PANEL_BASE || "https://j1900.tail8d0018.ts.net:8443";

/** 读档 20s（ls / tags / health），写档 180s（register 要真 clone，recheck 同理）。
 *  nw-panel 自己的上限是 120s，这里留点余量：谁是瓶颈由上游的 504 说话，不是我们。 */
const TIMEOUT_READ = 20_000;
const TIMEOUT_WRITE = 180_000;
const MAX_BODY = 1 << 20;

/* ── 上游版本：本块自己查 ──────────────────────────────────────────────────
 *
 * 编目里那个 `version` 是**账面情报**（nw 注册那一刻的 latest release），会过期。
 * 「现在上游到哪了」这一列只有两条路能拿：nw 的 `source recheck`（真 clone，j1900
 * 上今天是红的：出网坏，91 秒后失败），或者本块自己问。所以这里自己问。
 *
 * 三条选择，全是为了不占配额、不依赖那台坏机器：
 *   1. 走 `git ls-remote --tags`（git 协议），**不碰 api.github.com** —— 匿名 60 次/小时
 *      是整台机器共享的，一个页面打开几次就穿了。
 *   2. 只认 semver 的 tag，按数字排，最大的那个就是「上游最新」。
 *   3. 结果落 data/upstream.json（TTL 30 分钟）：重启不丢、不冷启动就打 GitHub。
 *
 * **不写编目。** 查出来只摆给看，改账还是走面板上那个「重新登记」—— 那是 nw 的写入口，
 * 有它自己的闸。
 */
const UPSTREAM_FILE = "upstream.json";
const UPSTREAM_TTL = 30 * 60_000;
const UPSTREAM_TIMEOUT = 12_000;
const UPSTREAM_CONCURRENCY = 3;
const GIT_ENV = { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "true" };
const run = promisify(execFile);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
};

const cfg = { base: DEFAULT_BASE, token: "" };
let cfgLoaded = false;

const normBase = (s) => String(s || "").trim().replace(/\/+$/, "");

const baseOK = (s) => {
  try {
    const u = new URL(s);
    return u.protocol === "https:" || u.protocol === "http:";
  } catch {
    return false;
  }
};

async function loadConfig() {
  if (cfgLoaded) return;
  cfgLoaded = true;
  try {
    const raw = JSON.parse(await readFile(CONFIG, "utf8"));
    if (baseOK(normBase(raw.base))) cfg.base = normBase(raw.base);
    if (typeof raw.token === "string") cfg.token = raw.token;
  } catch {
    // 没有配置文件 = 用默认 base，不是错。
  }
}

async function saveConfig() {
  await mkdir(DATA, { recursive: true });
  await writeFile(CONFIG, JSON.stringify({ base: cfg.base, token: cfg.token }, null, 2));
}

const sendJSON = (res, status, body, extra = {}) => {
  const text = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    ...extra,
  });
  res.end(text);
};

/** 面板自己的错，形状故意长得像 envelope —— 前端一套渲染器就够。 */
const panelError = (res, status, code, message, hint) =>
  sendJSON(res, status, {
    schema: "panel/error.v1",
    ok: false,
    errors: [{ code, message, ...(hint ? { hint } : {}) }],
    warnings: [],
    data: {},
  });

const readBody = (req) =>
  new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (c) => {
      size += c.length;
      if (size > MAX_BODY) {
        reject(new Error("请求体太大"));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });

async function proxy(req, res, url) {
  const sub = url.pathname.replace(/^\/api\/nw/, "") || "/";
  const target = cfg.base + "/api" + sub + url.search;
  const method = (req.method || "GET").toUpperCase();
  const readOnly = method === "GET" || method === "HEAD";
  const started = Date.now();
  let body;
  try {
    body = readOnly ? undefined : await readBody(req);
  } catch (e) {
    return panelError(res, 413, "PANEL_BODY_TOO_LARGE", String(e.message || e));
  }

  const headers = { accept: "application/json" };
  if (body) headers["content-type"] = req.headers["content-type"] || "application/json";
  if (cfg.token) headers.authorization = `Bearer ${cfg.token}`;

  try {
    const up = await fetch(target, {
      method,
      headers,
      body: body || undefined,
      redirect: "follow",
      signal: AbortSignal.timeout(readOnly ? TIMEOUT_READ : TIMEOUT_WRITE),
    });
    const text = await up.text();
    res.writeHead(up.status, {
      "content-type": up.headers.get("content-type") || "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-nw-ms": String(Date.now() - started),
    });
    res.end(text);
  } catch (e) {
    const timeout = e?.name === "TimeoutError" || e?.name === "AbortError";
    if (timeout) {
      // 说真话：超时之后上游可能已经写了（面板放弃 ≠ CLI 没写）。
      return panelError(
        res,
        504,
        "PANEL_UPSTREAM_TIMEOUT",
        `上游 ${readOnly ? TIMEOUT_READ : TIMEOUT_WRITE}ms 没回话`,
        "超时不等于没写：写命令可能已经落账或只落了本地 commit。去 nw-panel 那侧确认，别凭猜重跑。",
      );
    }
    return panelError(
      res,
      502,
      "PANEL_UPSTREAM_UNREACHABLE",
      `连不上 ${cfg.base}：${e?.cause?.message || e?.message || e}`,
      "tailnet 通不通？`curl -s <base>/api/health` 看一眼；或点右上角改 base。",
    );
  }
}

async function serveStatic(res, pathname) {
  const rel = pathname === "/" ? "/index.html" : pathname;
  const file = path.join(PUBLIC, path.normalize(rel).replace(/^(\.\.[/\\])+/, ""));
  if (!file.startsWith(PUBLIC)) return panelError(res, 403, "PANEL_FORBIDDEN", "越界路径");
  try {
    const st = await stat(file);
    if (!st.isFile()) throw new Error("not a file");
    res.writeHead(200, {
      "content-type": MIME[path.extname(file)] || "application/octet-stream",
      "content-length": st.size,
      // 开发期别让浏览器缓存旧 JS 骗人。
      "cache-control": "no-cache",
    });
    createReadStream(file).pipe(res);
  } catch {
    panelError(res, 404, "PANEL_NOT_FOUND", `这块里没有 ${pathname}`);
  }
}

/* ── 上游版本：查、缓存、比对 ─────────────────────────────────────────── */

/** { v: 1, at, repos: { <repo>: { latest, tags, semverTags, checkedAt, error } } } */
let upstream = { v: 1, at: 0, repos: {} };
const UPSTREAM_PATH = path.join(DATA, UPSTREAM_FILE);

async function loadUpstream() {
  try {
    const raw = JSON.parse(await readFile(UPSTREAM_PATH, "utf8"));
    if (raw?.v === 1 && raw.repos && typeof raw.repos === "object") upstream = raw;
  } catch {
    // 没文件 = 一条都没查过，不是错。
  }
}

async function saveUpstream() {
  await mkdir(DATA, { recursive: true });
  await writeFile(UPSTREAM_PATH, JSON.stringify(upstream, null, 2));
}

/** nw 存的是 https 或 https://…git，也接受 `owner/name` 和 git@github.com: 的形式。 */
const repoHttp = (repo) => {
  const r = String(repo || "").trim();
  if (/^https?:\/\//i.test(r)) return r.replace(/\.git$/i, "");
  if (/^git@github\.com:/i.test(r)) return "https://github.com/" + r.slice("git@github.com:".length).replace(/\.git$/i, "");
  if (/^[\w.-]+\/[\w.-]+$/.test(r)) return "https://github.com/" + r;
  // 别的主机用 ssh 形式登记的：不猜它的 https 入口。
  if (/^git@[^:]+:/i.test(r)) return "";
  return "";
};

const vtuple = (v) => {
  const m = /^v?(\d+)\.(\d+)\.(\d+)/.exec(String(v || "").trim());
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
};

/** ls-remote 一次：拿下全部 tag 名，挑 semver 里最大的。 */
async function checkOne(repo) {
  const url = repoHttp(repo);
  if (!url) return { latest: null, tags: 0, semverTags: 0, checkedAt: Date.now(), error: "认不出仓库地址" };
  try {
    const { stdout } = await run("git", ["ls-remote", "--tags", "--refs", url], {
      env: GIT_ENV,
      timeout: UPSTREAM_TIMEOUT,
      maxBuffer: 4 << 20,
    });
    const names = stdout
      .split("\n")
      .map((line) => (line.split(/\s+/)[1] || "").replace(/^refs\/tags\//, ""))
      .filter(Boolean);
    // 只认 semver：mattpocock-skills@1.0.0 这种不是版本号，进来会乱排。
    const ranked = names.map((n) => ({ n, t: vtuple(n) })).filter((x) => x.t);
    ranked.sort((a, b) => a.t[0] - b.t[0] || a.t[1] - b.t[1] || a.t[2] - b.t[2]);
    return {
      latest: ranked.length ? ranked[ranked.length - 1].n : null,
      tags: names.length,
      semverTags: ranked.length,
      checkedAt: Date.now(),
      error: null,
    };
  } catch (e) {
    // 断网 / 私有仓 / git 不在 PATH：记 error，不算「版本落后」—— 那是两件事。
    return { latest: null, tags: 0, semverTags: 0, checkedAt: Date.now(), error: String(e?.message || e).slice(0, 200) };
  }
}

/** 并发 3，一条仓库一次 TCP：别把 GitHub 当自己的后端。 */
async function checkMany(repos) {
  const out = {};
  const queue = [...repos];
  const workers = Array.from({ length: Math.min(UPSTREAM_CONCURRENCY, Math.max(1, queue.length)) }, async () => {
    while (queue.length) {
      const repo = queue.shift();
      if (repo) out[repo] = await checkOne(repo);
    }
  });
  await Promise.all(workers);
  return out;
}

const staleRepos = () => {
  const now = Date.now();
  return Object.keys(upstream.repos).filter((r) => now - (upstream.repos[r]?.checkedAt || 0) > UPSTREAM_TTL);
};

const server = http.createServer(async (req, res) => {
  await loadConfig();
  const url = new URL(req.url || "/", "http://localhost");
  const p = url.pathname;

  try {
    if (p === "/api/config") {
      if (req.method === "GET") {
        return sendJSON(res, 200, {
          base: cfg.base,
          hasToken: Boolean(cfg.token),
          tokenHint: cfg.token ? cfg.token.slice(0, 3) + "…" + cfg.token.slice(-2) : "",
          defaultBase: DEFAULT_BASE,
        });
      }
      if (req.method === "PUT") {
        let body;
        try {
          body = JSON.parse((await readBody(req)) || "{}");
        } catch {
          return panelError(res, 400, "PANEL_BAD_JSON", "请求体不是 JSON");
        }
        const next = normBase(body.base);
        if (!baseOK(next)) {
          return panelError(res, 400, "PANEL_BAD_BASE", `"${body.base}" 不是 http(s) 地址`);
        }
        cfg.base = next;
        if (typeof body.token === "string") cfg.token = body.token.trim();
        await saveConfig();
        return sendJSON(res, 200, { base: cfg.base, hasToken: Boolean(cfg.token) });
      }
      return panelError(res, 405, "PANEL_METHOD", `${req.method} 不收`);
    }

    if (p === "/api/upstream") {
      if (req.method === "GET") {
        return sendJSON(res, 200, { at: upstream.at || null, ttl: UPSTREAM_TTL, stale: staleRepos(), repos: upstream.repos });
      }
      if (req.method === "POST") {
        let body = {};
        try {
          body = JSON.parse((await readBody(req)) || "{}");
        } catch {
          return panelError(res, 400, "PANEL_BAD_JSON", "请求体不是 JSON");
        }
        const wanted = (Array.isArray(body.repos) ? body.repos : []).filter((x) => typeof x === "string" && x.trim());
        const force = Boolean(body.force);
        const targets = wanted.length ? wanted : Object.keys(upstream.repos);
        const now = Date.now();
        const todo = force
          ? targets
          : targets.filter((r) => !upstream.repos[r] || now - (upstream.repos[r].checkedAt || 0) > UPSTREAM_TTL);
        const done = await checkMany(todo);
        upstream.repos = { ...upstream.repos, ...done };
        upstream.at = Date.now();
        await saveUpstream();
        return sendJSON(res, 200, { checked: Object.keys(done), at: upstream.at, ttl: UPSTREAM_TTL, repos: upstream.repos });
      }
      return panelError(res, 405, "PANEL_METHOD", `${req.method} 不收`);
    }

    if (p.startsWith("/api/nw/")) return await proxy(req, res, url);
    if (p.startsWith("/api/")) return panelError(res, 404, "PANEL_NOT_FOUND", `没有 ${p}`);

    if (req.method !== "GET" && req.method !== "HEAD") {
      return panelError(res, 405, "PANEL_METHOD", `${req.method} 不收`);
    }
    return await serveStatic(res, p);
  } catch (e) {
    if (!res.headersSent) panelError(res, 500, "PANEL_INTERNAL", String(e?.message || e));
    else res.end();
  }
});

await loadUpstream();
server.listen(PORT, "127.0.0.1", () => {
  console.log(`skill 编目面板 listening on http://127.0.0.1:${PORT} → ${cfg.base}`);
  console.log(`数据目录 NOUS_DATA=${DATA}`);
});
