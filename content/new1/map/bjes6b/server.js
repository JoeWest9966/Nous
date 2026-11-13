// bjes6b —— 微信样子的 Nous 会话页。
//
// 这一块**不跑 agent**。会话、模型、工具、持久化全在 qrd3wh（那是个真引擎，SSE 出来的
// 第一帧就是 snapshot，断线重连自然续上）。这里只有两样东西：
//
//   1. 一层 80 行的 BFF：把 /api/* 翻成 qrd3wh 的 /v1/*，顺手把「分支」滤掉。
//   2. public/ 里的静态文件。
//
// 所以这个进程**无状态**：随便杀，重开就重连。宿主那套「改文件就杀进程」「闲 10 分钟下桌」
// 在这里不是威胁，是免费的热重载。
//
// 地址不拼。qrd3wh 在哪儿只问 `__nous/resolve`，前端只知道 /api/*。
import { createServer } from "node:http";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PORT = Number(process.env.PORT ?? 4800);
const PUBLIC = path.join(HERE, "public");

/** 引擎块的 id（名字会改，id 不会）。搬走也不改这里 —— 地址是 resolve 给的。 */
const ENGINE = process.env.BJES6B_ENGINE ?? "qrd3wh";
/** resolve 结果缓存多久。引擎是持久进程，地址基本不变。 */
const RESOLVE_TTL = 60_000;
/**
 * 模板 tab（第二个菜单）读的那一段。写死在这儿，不是写死在前端 ——
 * 前端只知道 `/api/presets`，段名换了它一行都不用动。
 */
const PRESET_SECTION = process.env.BJES6B_PRESET_SECTION ?? "Session-Template";
/** 我在宿主里叫什么。只用来拼「问宿主」的地址，块自己的地址一律走 resolve。 */
const SELF = process.env.NOUS_SELF ?? "/p/new1/bjes6b";

/**
 * 宿主的地址**按请求现算**，不认死 `NOUS_ORIGIN`。
 *
 * 宿主起进程时把 `NOUS_ORIGIN` 钉成 `http://<那一刻的 Host>`（见 blocks.ts:336）。有两个坑：
 * 从 Tailscale 进来时 Host 是 `xxx.ts.net` 而协议是 https —— 它排出来的却是 `http://`，
 * 打到 80 端口，直接 fetch failed；更糟的是进程一被复用，这个错地址就跟着进程活一整轮。
 * 所以候选表按可靠性排：请求自己的转发头 → NOUS_ORIGIN → 从 NOUS_ORIGIN 里抠出的回环地址。
 */
/** `localhost` 在有些机器上先解析到 ::1，而宿主只听 0.0.0.0（v4）。别让 DNS 决定连不连得上。 */
const toLoopback = (value) => value.replace("//localhost", "//127.0.0.1");

const originsOf = (req) => {
  const pick = (value) => (Array.isArray(value) ? value[0] : value) ?? "";
  const proto = String(pick(req.headers["x-forwarded-proto"])).split(",")[0].trim() || "http";
  const host = String(pick(req.headers["x-forwarded-host"]) || pick(req.headers.host)).split(",")[0].trim();

  const list = [];

  // 1. 宿主自己听的那个地址。反代、隧道、域名都不影响它，这是唯一一条「一定到得了」的路。
  const local = process.env.NOUS_LOCAL_ORIGIN;
  if (local) list.push(toLoopback(local));

  // 2. 外面看到的名字。只有反代把整条前缀也一起透传时才通 —— 常常不通，所以排在后面。
  if (host) list.push(`${proto}://${host}`);

  // 3. NOUS_ORIGIN，以及从它抠出来的回环（老宿主没给 NOUS_LOCAL_ORIGIN 时的兜底）。
  const origin = process.env.NOUS_ORIGIN;
  if (origin) {
    list.push(origin);
    try {
      const url = new URL(origin);
      list.push(`http://127.0.0.1:${url.port || 80}`);
    } catch {
      // origin 不是个 URL，那就没有候选。
    }
  }

  if (!list.length) list.push("http://127.0.0.1:4790");
  return [...new Set(list.map(toLoopback))];
};

/** origin → { url, at }。不同 Host 进来的请求各自缓存一份，互不相干。 */
const engines = new Map();

const resolveEngine = async (origins, force = false) => {
  // 开发用：绕开 resolve 直指一个假引擎（验流式渲染时不必真花 token）。
  if (process.env.BJES6B_ENGINE_URL) return process.env.BJES6B_ENGINE_URL;

  for (const origin of origins) {
    if (force) engines.delete(origin);
    const cached = engines.get(origin);
    if (cached && Date.now() - cached.at < RESOLVE_TTL) return cached.url;
    try {
      const res = await fetch(`${origin}${SELF}/__nous/resolve/${ENGINE}`);
      if (!res.ok) continue;
      const { url } = await res.json();
      engines.set(origin, { url, at: Date.now() });
      return url;
    } catch {
      // 这个候选不通，下一个。
    }
  }
  throw new Error(`resolve ${ENGINE} 失败（试过 ${origins.join(" , ")}）`);
};

/** 打引擎。第一次失败就重解析一次地址再试 —— 它可能刚搬过家或者刚被宿主收掉重起。 */
const call = async (origins, rest, init) => {
  try {
    return await fetch(`${await resolveEngine(origins)}${rest}`, init);
  } catch {
    return fetch(`${await resolveEngine(origins, true)}${rest}`, init);
  }
};

/* ── 置顶：这一页自己的状态，不进引擎。data/ 是宿主 watcher 的豁免区，写它不杀进程。 ── */

const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const PINS = path.join(DATA, "pins.json");

let pins = { ids: [] };

const loadPins = async () => {
  try {
    const raw = JSON.parse(await readFile(PINS, "utf8"));
    if (Array.isArray(raw.ids)) pins = { ids: raw.ids.map(Number) };
  } catch {
    // 没有文件就是没有置顶。别的错也一样：置顶丢了不该挡住列表。
  }
};

const savePins = async () => {
  await mkdir(DATA, { recursive: true });
  const tmp = path.join(DATA, `.pins.${process.pid}.tmp`);
  // 备份 → 临时文件 → 原子替换。写坏 pins.json 只该丢置顶，不该丢进程。
  await writeFile(tmp, JSON.stringify({ v: 1, ids: pins.ids }));
  await rename(tmp, PINS);
};

const isPinned = (id) => pins.ids.includes(Number(id));

/** 置顶 / 取消。返回动作是否真的改变了什么。 */
const setPin = (id, want) => {
  const key = Number(id);
  const at = pins.ids.indexOf(key);
  if (want && at === -1) {
    pins.ids.unshift(key); // 新置顶的排最前 —— 和微信一致
    return true;
  }
  if (!want && at !== -1) {
    pins.ids.splice(at, 1);
    return true;
  }
  return false;
};

const json = (res, status, value) => {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
  });
  res.end(body);
};

const readBody = async (req) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
};

/**
 * 上游响应原样转出去：状态码、那一段 body，一个字都不改。
 *
 * 为什么要原样：引擎报错时那句 `{error}` 已经说清了是什么坏了，包一层
 * 「加载失败」等于把唯一有用的那句话扔掉。也是 3699b9 那句「不重写服务端的话」。
 */
const relay = async (res, up) => {
  const body = await up.text();
  res.writeHead(up.status, {
    "content-type": up.headers.get("content-type") ?? "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
  });
  res.end(body);
};

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webmanifest": "application/manifest+json",
  ".ico": "image/x-icon",
};

const serveStatic = async (res, pathname) => {
  const rel = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
  const file = path.resolve(PUBLIC, rel);
  // 出目录的不给看。public/ 之外没有东西是这一页的。
  if (!file.startsWith(PUBLIC + path.sep) && file !== path.join(PUBLIC, "index.html")) {
    return json(res, 403, { error: "no" });
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, {
      "content-type": MIME[path.extname(file)] ?? "application/octet-stream",
      "content-length": body.byteLength,
      // 改一行代码宿主就把进程收了，所以永远别让它缓存旧壳。
      "cache-control": "no-cache",
    });
    res.end(body);
  } catch {
    // 客户端路由（#/c/2）走 hash，不会打到这儿；真 404 就给 404。
    json(res, 404, { error: "没有这个文件", path: pathname });
  }
};

/** SSE：原样透传，一字节都不攒。攒一下流式就变成一坨一坨地吐，用户会说「卡」。 */
const proxyEvents = async (req, res, id, origins) => {
  const ac = new AbortController();
  res.on("close", () => ac.abort());

  let up;
  try {
    up = await call(origins, `/v1/sessions/${id}/events`, {
      headers: { accept: "text/event-stream" },
      signal: ac.signal,
    });
  } catch (error) {
    return json(res, 502, { error: `引擎连不上：${error.message}` });
  }
  if (!up.ok || !up.body) return json(res, up.status, { error: `引擎给了 ${up.status}` });

  res.writeHead(200, {
    "content-type": "text/event-stream; charset=utf-8",
    "cache-control": "no-cache, no-transform",
    connection: "keep-alive",
    "x-accel-buffering": "no",
  });
  res.flushHeaders?.();

  try {
    for await (const chunk of up.body) {
      if (res.writableEnded) break;
      if (!res.write(chunk)) {
        await new Promise((resolve) => res.once("drain", resolve));
      }
    }
  } catch {
    // 客户端关了或者上游断了，两种都不用吭声。
  }
  res.end();
};

const route = async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const p = url.pathname;
  const origins = originsOf(req);

  // 归档库浏览。引擎给什么就是什么：gen 分组、每条的 name / archived 墓碑 / live 全在响应里，
  // 四态（原件在 / 已销毁 / 已捞回 / 无墓碑）由前端拿 archived / live 推，这一层不碰。
  if (p === "/api/archive" && req.method === "GET") {
    return relay(res, await call(origins, "/v1/archive"));
  }

  // 打捞：body（{"gen"}）一个字不改地递过去。
  const restore = p.match(/^\/api\/archive\/(\d+)\/restore$/);
  if (restore && req.method === "POST") {
    const body = await readBody(req);
    return relay(
      res,
      await call(origins, `/v1/archive/${restore[1]}/restore`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body,
      }),
    );
  }

  if (p === "/api/health") {
    return json(res, 200, { ok: true, origins: originsOf(req), engines: [...engines.entries()] });
  }

  // 模板：引擎的清单**原样**递出去。这一层不筛、不补字段、不算状态 —— 那是第二个真相。
  if (p === "/api/presets" && req.method === "GET") {
    return relay(res, await call(origins, `/v1/presets/${encodeURIComponent(PRESET_SECTION)}`));
  }

  // 建会话。目前唯一的调用方是模板 tab 的「点一行 = 开一条」：body 带 `preset`，引擎拿它打底。
  // 生效值（model / skills / template / missing）全在响应里，前端认那份，不认自己发的。
  if (p === "/api/sessions" && req.method === "POST") {
    const body = await readBody(req);
    return relay(
      res,
      await call(origins, "/v1/sessions", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body,
      }),
    );
  }

  // 会话列表。**只要主线**：parent 非空的是从别处 fork 出来的分支，这一页不认。
  if (p === "/api/sessions" && req.method === "GET") {
    const up = await call(origins, "/v1/sessions");
    if (!up.ok) return json(res, up.status, { error: `引擎给了 ${up.status}` });
    const { items = [] } = await up.json();

    const mains = items
      .filter((s) => s.parent == null)
      .map((s) => ({
        id: s.id,
        name: s.name,
        model: s.model,
        busy: s.busy ?? null,
        last: s.last ?? null,
        pinned: isPinned(s.id),
      }));

    // 置顶的在前（pins.ids 里的先后就是置顶的先后）；其余按最近活跃，拿 last.id 当钟。
    const order = new Map(pins.ids.map((id, index) => [id, index]));
    mains.sort((a, b) => {
      const pa = order.get(a.id);
      const pb = order.get(b.id);
      if (pa !== undefined || pb !== undefined) return (pa ?? Infinity) - (pb ?? Infinity);
      return (b.last?.id ?? 0) - (a.last?.id ?? 0);
    });

    return json(res, 200, { items: mains });
  }

  const m = p.match(/^\/api\/sessions\/(\d+)(\/[a-z]+)?$/);
  if (m) {
    const [, id, tail = ""] = m;

    if (tail === "/events" && req.method === "GET") return proxyEvents(req, res, id, origins);

    if (tail === "/entries" && req.method === "GET") {
      const limit = url.searchParams.get("limit") ?? "200";
      const up = await call(origins, `/v1/sessions/${id}/entries?limit=${encodeURIComponent(limit)}`);
      if (!up.ok) return json(res, up.status, { error: `引擎给了 ${up.status}` });
      // 引擎给的是新的在前；聊天页要从上往下读。
      const { items = [], next = null } = await up.json();
      return json(res, 200, { items: items.slice().reverse(), next });
    }
    if (tail === "/messages" && req.method === "POST") {
      const body = await readBody(req);
      return relay(
        res,
        await call(origins, `/v1/sessions/${id}/messages`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body,
        }),
      );
    }

    if (tail === "/abort" && req.method === "POST") {
      return relay(res, await call(origins, `/v1/sessions/${id}/abort`, { method: "POST" }));
    }

    if (tail === "/archive" && req.method === "POST") {
      // 归档 = 复印进引擎的 gen 文件 + 写墓碑。成功后引擎的列表就藏掉它，
      // 这一页的 /api/sessions 自然也就不见了 —— 这里没有第二份名单。
      return relay(res, await call(origins, `/v1/sessions/${id}/archive`, { method: "POST" }));
    }

    if (tail === "/purge" && req.method === "POST") {
      // 撕原件：硬条件（已归档、闲置、无活后代）全在引擎那侧拦，409 连 {error} 原样递回来。
      return relay(res, await call(origins, `/v1/sessions/${id}/purge`, { method: "POST" }));
    }

    if (tail === "/pin" && req.method === "POST") {
      const body = JSON.parse((await readBody(req)) || "{}");
      const want = Boolean(body.pinned);
      // 引擎里有没有这个会话不归我验 —— 引擎 404 了自然把错递回去。
      // 用 config 探存在性（空 body 不改任何东西）；引擎没做 GET /v1/sessions/:id。
      const probe = await call(origins, `/v1/sessions/${id}/config`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{}",
      });
      if (probe.status === 404) return json(res, 404, { error: "引擎里没有这个会话" });
      const changed = setPin(id, want);
      if (changed) await savePins();
      return json(res, 200, { id: Number(id), pinned: want, changed });
    }

    return json(res, 404, { error: "这一块没有这条路", tail });
  }

  if (req.method === "GET" || req.method === "HEAD") return serveStatic(res, p);
  return json(res, 405, { error: "不会" });
};

const server = createServer((req, res) => {
  route(req, res).catch((error) => {
    console.error("[bjes6b]", error);
    if (!res.headersSent) json(res, 500, { error: String(error?.message ?? error) });
    else res.end();
  });
});

server.on("error", (error) => {
  console.error(`[bjes6b] 起不来：${error.code ?? error.message}`);
  process.exit(1); // 端口被占就死给宿主看 —— 悄悄挂着只会让人以为它活着。
});

server.listen(PORT, async () => {
  await loadPins();
  // PORT=0 时打印实际端口，测试和脚本才找得到它。
  console.log(`[bjes6b] 听 ${server.address().port} · 引擎 ${ENGINE} · ${SELF} · 置顶 ${pins.ids.length}`);
});
