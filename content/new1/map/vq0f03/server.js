// vq0f03「vps」—— 这个项目 vps 侧的控制台：一张拓扑图 + 一套读写 API。
//
// 上游是 j1900 上的 nw-panel（`/api/vps` 一族）。这里是它的**代理 + 注解层**，
// 不是第二份清单：机器名/备注/用户/端口/IP 只在 /opt/Data-001/vps/vps.json 里活一份。
//
// 上游的真实契约（从面板前端 bundle 里抄出来的，不是猜的）：
//
//   GET    /api/vps                      nw/vps-ls.v1      { machines[] }
//   POST   /api/vps                      nw/vps-register.v1 {name, ips[{label,addr}], user?, password?|keyFile{name,content}, port, note?, tags?}
//   GET    /api/vps/<name>/secret         nw/vps-get.v1     凭证本体（password 或 key）—— 只打**回环面板**
//   PUT    /api/vps/<name>               同上，凭证留空 = 保留旧凭证（= 「重新登记」）
//   DELETE /api/vps/<name>               nw/vps-rm.v1
//   GET    /api/tags                     nw/tag-ls.v1      { tags[{name, refs[{kind,id}]}] }
//   POST   /api/vps/<name>/tags          {name}            ← 只认已存在的标签
//   DELETE /api/vps/<name>/tags/<tag>
//
// 三个必须记住的语义（踩过才知道）：
//   1. **登记会先做 SSH 连线测试**（打第一条地址，10 秒超时），连不上不落盘 → 写接口要等，
//      前端不能 3 秒就判超时。
//   2. **密码是明文进仓的**（面板自己的提示：#194）。密钥走 keyFile{name,content(base64)}，
//      只进 stdin。别在日志里回显任何凭证 —— 这个文件里一行都不打。
//   3. **名字是唯一键，没有 rename**：改名 = 删旧建新。所以编辑表单里名字锁死。
//
// 这一块自己只写一样东西：data/vps.json —— 上游没有的注解（role / facts / caveats）。
//
// 宿主给 PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF。这里只用前两个，加可选的 NW_BASE。

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const LEDGER = path.join(DATA, "vps.json");
const PORT = Number(process.env.PORT ?? 4782);
const NW = (process.env.NW_BASE ?? "https://j1900.tail8d0018.ts.net:8443").replace(/\/+$/, "");

// 凭据只在一处：上游的**回环面板**（Nous-Warehouse routes.go：远程门缺省不注册 secret 表项，
// 回环面板恒注册）。所以这一跳不走 NW（那是远程门），走回环。
const SECRET_BASE = (process.env.NW_SECRET_BASE ?? "http://127.0.0.1:18080").replace(/\/+$/, "");

const READ_TTL = 60_000;
const PROBE_TTL = 30_000;
const PROBE_TIMEOUT = 4_000;
const BANNER_GRACE = 1_200;
/** 写操作要等上游的 SSH 连线测试 + 落盘。实测这台机器上是 **80 秒**：`nw vps register` 写完 vps.json
 *  还得在数据仓里落一手，命令行比面板 API 想得慢得多。给 30 秒的耐心会把它判成「失败」——
 *  而它其实已经写进去了，页面说没写，就是最坏的那种谎。所以给足 150 秒。 */
const WRITE_TIMEOUT = 150_000;

const V = "v1";
const schema = (what) => `vq0f03/${what}.${V}`;

const now = () => new Date().toISOString();
const oneLine = (e) => String(e?.message ?? e).split("\n")[0];

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".json": "application/json; charset=utf-8",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
};

// ── 注解层：data/vps.json ───────────────────────────────────────────────────

let ledgerCache = { at: 0, value: null, error: null };

const ledger = async () => {
  if (ledgerCache.value !== null && Date.now() - ledgerCache.at < 5_000) return ledgerCache;
  try {
    const doc = JSON.parse(await readFile(LEDGER, "utf8"));
    ledgerCache = { at: Date.now(), value: doc, error: null };
  } catch (e) {
    ledgerCache = {
      at: Date.now(),
      value: { machines: {}, domains: [], caveats: [], source: {} },
      error: e?.code === "ENOENT" ? null : oneLine(e),
    };
  }
  return ledgerCache;
};

// ── 上游：只读缓存，不落盘 ─────────────────────────────────────────────────
//
// 上游挂了就 503 + 原因。落一份快照当「上次的样子」，就变成第二份清单了 ——
// 而那正是这个块最不该有的东西。

let cache = { at: 0, value: null };
const invalidate = () => {
  cache = { at: 0, value: null };
  probeCache.clear();
};

const upstreamGet = async (fresh = false) => {
  if (!fresh && cache.value && Date.now() - cache.at < READ_TTL) return { ...cache.value, cached: true };

  const t0 = performance.now();
  try {
    const res = await fetch(`${NW}/api/vps`, {
      headers: { accept: "application/json" },
      signal: AbortSignal.timeout(8_000),
    });
    const body = await res.json().catch(() => null);
    const value = {
      ok: res.ok && body?.ok !== false,
      status: res.status,
      ms: Math.round(performance.now() - t0),
      at: now(),
      body,
      error: res.ok ? null : `上游回了 ${res.status}`,
    };
    if (value.ok) cache = { at: Date.now(), value };
    return { ...value, cached: false };
  } catch (e) {
    const cause = e?.cause?.code ?? e?.cause?.message ?? "";
    return {
      ok: false,
      status: 0,
      ms: Math.round(performance.now() - t0),
      at: now(),
      body: null,
      error: `${oneLine(e)}${cause ? `（${cause}）` : ""}`,
      cached: false,
    };
  }
};

/** 标签编目：{name, refs[{kind,id}]}。id 就是机器名。 */
const tagsGet = async () => {
  const t0 = performance.now();
  try {
    const res = await fetch(`${NW}/api/tags`, {
      headers: { accept: "application/json" },
      signal: AbortSignal.timeout(8_000),
    });
    const body = await res.json().catch(() => null);
    const tags = (body?.data?.tags ?? []).filter((t) => t?.name);
    const raw = tags.flatMap((t) =>
      (t.refs ?? []).filter((r) => r?.kind === "vps").map((r) => String(r.id ?? "")),
    );
    return {
      ok: !!body?.ok,
      ms: Math.round(performance.now() - t0),
      catalogPath: body?.data?.catalogPath ?? "",
      tags,
      // 指着一个空 id 的引用是编目里长毛的地方：说出来，不要在页面上静默吞掉。
      dangling: tags
        .filter((t) => (t.refs ?? []).some((r) => r?.kind === "vps" && !String(r.id ?? "").trim()))
        .map((t) => t.name),
      error: body?.ok ? null : (body?.errors?.[0]?.message ?? "标签编目读不出来"),
      raw,
    };
  } catch (e) {
    return { ok: false, ms: Math.round(performance.now() - t0), tags: [], dangling: [], raw: [], error: oneLine(e) };
  }
};

// ── 探活 ──────────────────────────────────────────────────────────────────

const probeCache = new Map();

const DEBIAN = { 9: "stretch", 10: "buster", 11: "bullseye", 12: "bookworm", 13: "trixie", 14: "forky" };

const describeBanner = (banner) => {
  const text = banner.trim();
  const line = text.split("\n")[0] ?? "";
  if (!line) return { service: "", version: "", software: "", os: "" };

  // `SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u5`：版本号后面那截是注释，
  // 「这台是不是 Debian 12」正好在注释里。只取第一段等于把答案扔掉。
  const ssh = /^SSH-2\.0-(\S+)(?:\s+(.*))?$/.exec(line);
  if (ssh) {
    const [, version, comment = ""] = ssh;
    const deb = /Debian-2\+deb(\d+)/.exec(comment);
    return {
      service: "ssh",
      version,
      software: `${version}${comment ? ` ${comment}` : ""}`,
      os: deb ? `Debian ${deb[1]}${DEBIAN[deb[1]] ? ` (${DEBIAN[deb[1]]})` : ""}` : /Debian/.test(comment) ? "Debian" : "",
    };
  }
  const smtp = /^220[ -](.*)/.exec(line);
  if (smtp) return { service: "smtp", version: "", software: smtp[1].slice(0, 80), os: "" };
  const svc = /^HTTP\//.test(line) ? "http" : "";
  return { service: svc, version: "", software: line.slice(0, 80), os: "" };
};

const tcpProbe = (addr, port) =>
  new Promise((resolve) => {
    const t0 = performance.now();
    const sock = net.connect({ host: addr, port });
    let connectMs = null;
    let banner = "";
    let settled = false;

    const finish = (ok, error) => {
      if (settled) return;
      settled = true;
      sock.destroy();
      resolve({
        ok,
        addr,
        port,
        // 连上的用握手耗时；没连上的用「等到放弃为止」。两个都叫 ms，只有前者是延迟。
        ms: connectMs ?? Math.round(performance.now() - t0),
        connectMs,
        banner: banner.trim().slice(0, 200),
        ...describeBanner(banner),
        error: error ?? null,
      });
    };

    sock.setTimeout(PROBE_TIMEOUT, () => finish(false, `${PROBE_TIMEOUT}ms 内没连上`));
    sock.once("connect", () => {
      connectMs = Math.round(performance.now() - t0);
      setTimeout(() => finish(true), BANNER_GRACE);
    });
    sock.on("data", (d) => {
      banner += d.toString("latin1");
      if (banner.includes("\n") || banner.length > 200) finish(true);
    });
    sock.once("error", (e) => finish(false, e.code ?? e.message));
  });

const probe = async (addr, port, fresh = false) => {
  const key = `${addr}:${port}`;
  const hit = probeCache.get(key);
  if (!fresh && hit && Date.now() - hit.at < PROBE_TTL) {
    return { ...hit.value, checkedAt: hit.value.at, ageMs: Date.now() - hit.at, cached: true };
  }
  const value = { ...(await tcpProbe(addr, port)), at: now() };
  probeCache.set(key, { at: Date.now(), value });
  return { ...value, checkedAt: value.at, ageMs: 0, cached: false };
};

// ── 拼装 ──────────────────────────────────────────────────────────────────

const ipsOf = (m) => (m?.ips ?? []).map((i) => ({ addr: String(i?.addr ?? ""), label: String(i?.label ?? "") })).filter((i) => i.addr);

const shape = (raw, note, tagsByMachine) => {
  const machines = (raw?.machines ?? []).filter((m) => m?.name).map((m) => {
    const name = String(m.name);
    const annotated = note.machines?.[name] ?? null;
    const user = m.user ?? "root";
    const port = Number(m.port) || 22;
    return {
      name,
      note: m.note ?? "",
      user,
      port,
      ips: ipsOf(m),
      // 派生的东西一个字都不存：ssh 命令是从 user/port/ips 现拼的。
      ssh: ipsOf(m).map((i) => ({ addr: i.addr, label: i.label, cmd: `ssh -p ${port} ${user}@${i.addr}` })),
      tags: tagsByMachine.get(name) ?? [],
      annotated: !!annotated,
      role: annotated?.role ?? "",
      facts: annotated?.facts ?? [],
      caveats: annotated?.caveats ?? [],
      // 上游有、这里没注解 —— 不是错误，是「还没写」。但要说出来。
      undocumented: !annotated,
    };
  });

  const unknown = machines.filter((m) => m.undocumented).map((m) => `data/vps.json 里没有 ${m.name} 的注解`);
  return { machines, unknown };
};

const docFields = (note) => ({
  source: {
    project: note.source?.project ?? "Nous-Warehouse",
    panel: NW,
    catalog: note.source?.catalog ?? "",
    doc: note.source?.doc ?? "",
    schema: note.source?.schema ?? "",
    auth: note.source?.auth ?? "",
  },
  domains: note.domains ?? [],
  caveats: note.caveats ?? [],
});

const bundle = async (fresh = false) => {
  const note = await ledger();
  const u = await upstreamGet(fresh);
  if (!u.ok) {
    return {
      ok: false,
      errors: [`上游 /api/vps 不可用：${u.error ?? `HTTP ${u.status}`}（${NW}）`],
      warnings: note.error ? [`data/vps.json 读不出来：${note.error}`] : [],
      data: { ...docFields(note.value), machines: [], tags: [], upstream: { base: NW, ok: false, error: u.error, at: u.at } },
    };
  }

  const t = await tagsGet();
  const tagsByMachine = new Map();
  for (const tag of t.tags) {
    for (const ref of tag.refs ?? []) {
      if (ref?.kind !== "vps") continue;
      const id = String(ref.id ?? "");
      if (!id) continue;
      const list = tagsByMachine.get(id) ?? [];
      list.push(tag.name);
      tagsByMachine.set(id, list);
    }
  }

  const { machines, unknown } = shape(u.body?.data, note.value, tagsByMachine);

  // 删机器不会顺手清 tags.json：删完标签还指着一个不存在的人。这种毛得大声说，
  // 因为页面上看不出来（标签在页上是「按机器挂」的，死人不上图）。
  const names = new Set(machines.map((m) => m.name));
  const stale = t.tags.flatMap((tag) =>
    (tag.refs ?? [])
      .filter((r) => r?.kind === "vps" && String(r.id ?? "").trim() && !names.has(String(r.id)))
      .map((r) => `${tag.name} → ${r.id}`),
  );

  return {
    ok: true,
    errors: [],
    warnings: [
      ...(u.body?.warnings ?? []),
      ...(note.error ? [`data/vps.json 读不出来：${note.error}`] : []),
      ...(t.ok ? [] : [`标签编目读不出来：${t.error}`]),
      ...t.dangling.map((n) => `标签 ${n} 有一条指向空 id 的 vps 引用（tags.json 里长毛了）`),
      ...stale.map((s) => `标签 ${s} 指向的机器已经不在编目里了（删机器不清标签）`),
      ...unknown,
    ],
    data: {
      ...docFields(note.value),
      upstream: {
        base: NW,
        ok: true,
        status: u.status,
        ms: u.ms,
        at: u.at,
        cached: u.cached,
        nw: u.body?.nw ?? "",
        catalogPath: u.body?.data?.catalogPath ?? "",
        tagsPath: t.catalogPath ?? "",
      },
      counts: {
        machines: machines.length,
        tagged: machines.filter((m) => m.tags.length > 0).length,
        annotated: machines.filter((m) => m.annotated).length,
      },
      machines,
      tags: t.tags.map((g) => ({
        name: g.name,
        machines: (g.refs ?? []).filter((r) => r?.kind === "vps").map((r) => String(r.id ?? "")).filter(Boolean),
      })),
    },
  };
};

// ── 写：白名单代理，不做通用透传 ───────────────────────────────────────────
//
// 只放行上游那 5 条写路由。放一个「method + path 由前端给」的通用转发，
// 就等于把整个 nw 面板的写面挂在 tailnet 上，还是用这个块的地址。

// ── 凭据：唯一一条能读出本体的路 ──────────────────────────────────────
//
// 上游把它放在回环面板上、还要一个防 CSRF 的头（`X-NW-Reveal: 1`，不带就 400）：
// 地址栏一贴、别的页面一个 `<img src=...>` 都拿不到。这里不拆那道门，只是**如实转一手**，
// 给同一台主机上的块（qrd3wh 的远端会话）用。
//
// 三条纪律，改这段先读：
//   1. 只拷两个字段（kind / key），**原信封的其它字段一律不转** —— 少转一份就少一份会漂的东西；
//   2. 不进缓存、不进日志、不调 error/onLine 把内容拼进错误；
//   3. 机器得先在上游编目里（白名单就是编目），否则 404。
const reveal = async (name) => {
  try {
    const res = await fetch(`${SECRET_BASE}/api/vps/${encodeURIComponent(name)}/secret`, {
      headers: { accept: "application/json", "X-NW-Reveal": "1" },
      signal: AbortSignal.timeout(15_000),
    });
    const body = await res.json().catch(() => null);
    const data = body?.data ?? null;
    if (!res.ok || body?.ok === false || !data) {
      return { ok: false, status: res.status, kind: data?.kind ?? null, key: null, errors: (body?.errors ?? []).map(oneLine) };
    }
    return { ok: true, status: res.status, kind: data.kind ?? null, key: data.key ?? null, errors: [] };
  } catch (error) {
    return { ok: false, status: 0, kind: null, key: null, errors: [`回环面板不可达：${String(error?.message ?? error)}`] };
  }
};

const write = async (method, upstreamPath, payload) => {
  const t0 = performance.now();
  try {
    const res = await fetch(`${NW}${upstreamPath}`, {
      method,
      headers: { accept: "application/json", ...(payload ? { "content-type": "application/json" } : {}) },
      body: payload ? JSON.stringify(payload) : undefined,
      // 凭证只在这一跳里活一次：不记日志、不回显、不进缓存。
      signal: AbortSignal.timeout(WRITE_TIMEOUT),
    });
    const body = await res.json().catch(() => null);
    const ms = Math.round(performance.now() - t0);
    // 上游的 errors[] 是最好的一份说明（带 code / message / hint / gates），原样端出去。
    const errors = (body?.errors ?? []).map((e) => (typeof e === "string" ? e : [e.message, e.hint].filter(Boolean).join(" —— ")));
    // 上游把一次写成两件事：改本地仓，再推 GitHub。本地成了、推的时候网络断了，它照样回 500 ——
    // 可编目是真的变了（`nw vps ls` 里新机器就在）。这时候报「失败」是谎话：
    // 报成功 + 一条 warning 才是实话。判据是上游自己那句话，不是 HTTP 码。
    const partial = errors.some((e) => /本地 commit 已成功/.test(e));
    return {
      ok: partial || (res.ok && body?.ok !== false),
      partial,
      status: res.status,
      ms,
      at: now(),
      action: body?.data?.action ?? "",
      errors: partial ? [] : errors,
      warnings: partial ? [`写进数据仓了，但没推上 GitHub：${errors.join("；")}`] : [],
      body,
    };
  } catch (e) {
    const timeout = e?.name === "TimeoutError";
    return {
      ok: false,
      status: 0,
      ms: Math.round(performance.now() - t0),
      at: now(),
      errors: [
        timeout
          ? `上游 ${WRITE_TIMEOUT / 1000}s 没回话（登记会先做 SSH 连线测试，慢机器可能真的这么慢）`
          : `${oneLine(e)}${e?.cause?.code ? `（${e.cause.code}）` : ""}`,
      ],
      body: null,
    };
  }
};

// ── 静态文件 ──────────────────────────────────────────────────────────────

const serveStatic = async (res, rel) => {
  // 反代进来的路径可能是 /、/app.js，也可能是 /x/y/app.js。剥前导斜杠后再落进 PUBLIC，
  // 落出去就 403 —— 别用 path.resolve(PUBLIC, "/app.js")，绝对路径会把 PUBLIC 顶掉。
  const clean = path.normalize(decodeURIComponent(rel)).replace(/^(\.\.[/\\])+/, "").replace(/^[/\\]+/, "");
  const file = path.join(PUBLIC, clean === "" || clean === "." ? "index.html" : clean);
  if (file !== PUBLIC && !file.startsWith(PUBLIC + path.sep)) return json(res, 403, { error: "不许出这个文件夹" });
  try {
    const body = await readFile(file);
    res.writeHead(200, {
      "content-type": TYPES[path.extname(file)] ?? "application/octet-stream",
      // 块改代码宿主就收进程，缓存策略除了坑没别的用途。
      "cache-control": "no-store",
      "content-length": body.length,
    });
    res.end(body);
  } catch {
    res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    res.end("404");
  }
};

const json = (res, code, body) => {
  const text = JSON.stringify(body, null, 1);
  res.writeHead(code, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(text),
  });
  res.end(text);
};

const envelope = (code, schemaName, data, extra = {}) => ({
  schema: schemaName,
  ok: code < 400,
  errors: extra.errors ?? [],
  warnings: extra.warnings ?? [],
  at: now(),
  data,
});

const readBody = (req) =>
  new Promise((resolve, reject) => {
    let text = "";
    req.on("data", (c) => {
      text += c;
      if (text.length > 2 << 20) reject(new Error("请求体太大"));
    });
    req.on("end", () => {
      if (!text.trim()) return resolve(null);
      try {
        resolve(JSON.parse(text));
      } catch {
        reject(new Error("请求体不是合法的 JSON"));
      }
    });
    req.on("error", reject);
  });

/** 机器名 / 标签名 / 端口都先在本地过一道，省一次 30 秒的往返。
 *  闸的权威仍然在上游 —— 这里只是让「名字打成大写」这种错别等十秒。 */
const NAME_RE = /^[a-z0-9-]+$/;
const TAG_RE = /^[a-z0-9-]+$/;

const validateMachine = (body, { requireCreds }) => {
  const out = [];
  const name = String(body?.name ?? "");
  if (!NAME_RE.test(name)) out.push(`名字只许 [a-z0-9-]+，收到「${name}」`);
  const ips = Array.isArray(body?.ips) ? body.ips.filter((i) => String(i?.addr ?? "").trim()) : [];
  if (ips.length === 0) out.push("至少要一条地址（label + addr）");
  for (const i of ips) {
    if (!String(i?.label ?? "").trim()) out.push(`地址 ${i?.addr} 缺 label（公网 / 内网 / tailscale）`);
  }
  if (body?.user !== undefined && !String(body.user).trim()) out.push("user 不给就整个别传，别传空字符串");
  const port = body?.port === undefined ? 22 : Number(body.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) out.push(`端口须在 1..65535，收到「${body?.port}」`);
  if (requireCreds && !body?.password && !body?.keyFile) out.push("登记必须给凭证：password 或 keyFile 二选一（上游的 SSH 连线测试要用它）");
  if (body?.keyFile && (!body.keyFile.name || typeof body.keyFile.content !== "string")) {
    out.push("keyFile 的形状是 {name, content(base64)}");
  }
  for (const t of body?.tags ?? []) {
    if (!TAG_RE.test(String(t))) out.push(`标签只许 [a-z0-9-]+，收到「${t}」`);
  }
  return out;
};

// ── 路由 ──────────────────────────────────────────────────────────────────

const ENDPOINTS = [
  ["GET", "/api/bootstrap", "页面一次拿全：机器 + 标签 + 上游状态"],
  ["GET", "/api/vps?fresh=1", "机器清单"],
  ["GET", "/api/vps/<name>", "一台（带注解）"],
  ["GET", "/api/vps/<name>/ssh", "连它的命令（现拼，不存盘）"],
  ["GET", "/api/vps/<name>/secret", "那台机器的凭据本体（只打回环面板；调用方自己负责不落盘）"],
  ["POST", "/api/vps", "登记：{name, ips[{label,addr}], user?, port, note?, tags?, password?|keyFile{name,content}}"],
  ["PUT", "/api/vps/<name>", "覆盖登记；凭证留空 = 保留旧的"],
  ["DELETE", "/api/vps/<name>", "从编目里删掉这台"],
  ["POST", "/api/vps/<name>/tags", "给已存在的标签挂上这台 {name}（不会建新标签）"],
  ["DELETE", "/api/vps/<name>/tags/<tag>", "摘标签"],
  ["GET", "/api/tags", "标签编目，带它挂着哪些机器"],
  ["GET", "/api/probe?name=<n>[&addr=&port=]", "现探一次 TCP（白名单：编目里的目标）"],
  ["GET", "/api/health", "自己 + 上游活没活"],
  ["GET", "/api/catalog", "给 agent 的全量：机器 + 注解 + 上游端点 + 坑"],
  ["GET", "/api/upstream", "上游原始信封，一字不改"],
];

const notFound = (res, msg) =>
  json(res, 404, envelope(404, schema("error"), { endpoints: ENDPOINTS.map(([m, p, what]) => ({ method: m, path: p, what })) }, { errors: [msg] }));

const route = async (req, res, url) => {
  const seg = url.pathname.split("/").filter(Boolean);
  const fresh = url.searchParams.get("fresh") === "1";
  const isApi = seg[0] === "api";

  if (!isApi) {
    if (req.method !== "GET" && req.method !== "HEAD") {
      return json(res, 405, envelope(405, schema("error"), {}, { errors: ["静态文件只读"] }));
    }
    return serveStatic(res, seg.join("/"));
  }

  const [, a, b, c, d] = seg;

  // ── 读 ──
  if (req.method === "GET" || req.method === "HEAD") {
    if (a === "health") {
      const u = await upstreamGet(fresh);
      const code = u.ok ? 200 : 503;
      return json(
        res,
        code,
        envelope(
          code,
          schema("health"),
          {
            self: { ok: true, uptimeMs: Math.round(process.uptime() * 1000), node: process.version },
            upstream: { base: NW, ok: u.ok, status: u.status, ms: u.ms, at: u.at, error: u.error, cached: u.cached },
          },
          { errors: u.error ? [`上游不可用：${u.error}`] : [] },
        ),
      );
    }

    if (a === "bootstrap" || a === "vps" || a === "catalog") {
      const r = await bundle(fresh);
      const code = r.ok ? 200 : 503;
      const name = a === "catalog" ? "catalog" : a === "bootstrap" ? "bootstrap" : "vps-ls";
      let data = r.data;

      if (a === "vps" && seg.length === 2) {
        data = { source: r.data.source, upstream: r.data.upstream, machines: r.data.machines };
      }
      if (a === "vps" && seg.length >= 3) {
        const name2 = decodeURIComponent(b);
        const machine = r.data.machines.find((m) => m.name === name2);
        if (!machine) {
          return json(res, 404, envelope(404, schema("vps"), { known: r.data.machines.map((m) => m.name) }, { errors: [`编目里没有 ${name2}`] }));
        }
        if (c === "secret") {
          const s = await reveal(name2);
          if (!s.ok) {
            const code = s.status === 400 || s.status === 404 ? s.status : 503;
            return json(res, code, envelope(code, schema("vps-secret"), { machine: machine.name }, {
              errors: s.errors.length ? s.errors : [`拿不到 ${name2} 的凭据（status=${s.status}）`],
            }));
          }
          if (!s.key) {
            // 密码型：本体存在，但这块只说密钥型 —— 说清楚比给一个说不清的真的强。
            return json(res, 409, envelope(409, schema("vps-secret"), { machine: machine.name, kind: s.kind }, {
              errors: [`${name2} 是密码型（kind=${s.kind}），这里只转密钥型；要远端会话，先在它上面装公钥`],
            }));
          }
          return json(res, 200, envelope(200, schema("vps-secret"), {
            name: machine.name,
            kind: s.kind ?? "key",
            key: s.key,
            caveat: "明文 PEM。调用方自己负责：不落块的文件夹、不进日志、用完删。",
          }));
        }

        if (c === "ssh") {
          return json(
            res,
            200,
            envelope(200, schema("ssh"), {
              name: machine.name,
              user: machine.user,
              port: machine.port,
              commands: machine.ssh,
              primary: machine.ssh[0]?.cmd ?? "",
              caveat: "命令是拼的，不是抄的。密钥、known_hosts、跳板都不在编目里 —— 那是你的事。",
            }),
          );
        }
        return json(
          res,
          200,
          envelope(200, schema("vps"), { source: r.data.source, machine }, { warnings: r.warnings }),
        );
      }

      return json(res, code, envelope(code, schema(name), data, { warnings: r.warnings, errors: r.errors }));
    }

    if (a === "tags") {
      const r = await bundle(fresh);
      return json(res, 200, envelope(200, schema("tags"), { tags: r.data.tags ?? [], upstream: r.data.upstream }, { warnings: r.warnings }));
    }

    if (a === "upstream") {
      const u = await upstreamGet(fresh);
      const code = u.ok ? 200 : 503;
      return json(
        res,
        code,
        envelope(
          code,
          schema("upstream"),
          { fetch: { base: NW, path: "/api/vps", status: u.status, ms: u.ms, at: u.at, cached: u.cached }, raw: u.body },
          { errors: u.error ? [`上游不可用：${u.error}`] : [] },
        ),
      );
    }

    if (a === "probe") {
      const r = await bundle(fresh);
      if (!r.ok) {
        // 白名单就在上游：上游读不到，就没法证明目标在编目里 —— 拒绝，不放过。
        return json(res, 503, envelope(503, schema("probe"), {}, { errors: [`上游不可用，白名单无法验证：${r.errors[0]}`] }));
      }
      const machines = r.data.machines;
      const name = url.searchParams.get("name");
      const addr = url.searchParams.get("addr");
      const wantPort = Number(url.searchParams.get("port")) || 0;

      const pick = name
        ? machines.find((m) => m.name === name)
        : machines.find((m) => m.ips.some((i) => i.addr === addr) && (!wantPort || m.port === wantPort));

      if (!pick) {
        const allowed = machines.flatMap((m) => m.ips.map((i) => `${i.addr}:${m.port}`));
        return json(
          res,
          addr || name ? 403 : 400,
          envelope(
            addr || name ? 403 : 400,
            schema("probe"),
            { allowed },
            { errors: [addr || name ? `不在编目里：${name ?? `${addr}${wantPort ? `:${wantPort}` : ""}`} —— 探活只探编目写过的目标` : "要么给 name=，要么给 addr= 且它得在编目里"] },
          ),
        );
      }

      const target = addr && pick.ips.some((i) => i.addr === addr) ? addr : pick.ips[0]?.addr;
      const result = await probe(target, wantPort || pick.port, url.searchParams.get("cached") !== "1");
      return json(res, 200, envelope(200, schema("probe"), { machine: pick.name, ...result }));
    }

    if (seg.length === 1) {
      return json(
        res,
        200,
        envelope(200, schema("index"), {
          block: "vq0f03",
          note: "这个项目的 vps 控制台。页面在 /，这里是它的 API。",
          endpoints: ENDPOINTS.map(([m, p, what]) => ({ method: m, path: p, what })),
        }),
      );
    }
    return notFound(res, `没有这个端点：/${seg.join("/")}`);
  }

  // ── 写 ──
  let payload = null;
  if (req.method === "POST" || req.method === "PUT") {
    try {
      payload = await readBody(req);
    } catch (e) {
      return json(res, 400, envelope(400, schema("error"), {}, { errors: [oneLine(e)] }));
    }
    if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
      return json(res, 400, envelope(400, schema("error"), {}, { errors: ["请求体得是一个 JSON 对象"] }));
    }
  }

  if (a === "vps" && seg.length === 2 && req.method === "POST") {
    const problems = validateMachine(payload, { requireCreds: true });
    if (problems.length) return json(res, 400, envelope(400, schema("write"), {}, { errors: problems }));
    const r = await write("POST", "/api/vps", payload);
    if (r.ok) invalidate();
    return json(
      res,
      r.ok ? 200 : r.status || 502,
      envelope(r.ok ? 200 : r.status || 502, schema("write"), { op: "register", name: payload.name, action: r.action, upstreamMs: r.ms, upstream: r.body }, { errors: r.errors, warnings: r.warnings }),
    );
  }

  if (a === "vps" && seg.length >= 3) {
    const name = decodeURIComponent(b);

    if (req.method === "PUT" && seg.length === 3) {
      const problems = validateMachine({ ...payload, name }, { requireCreds: false });
      if (problems.length) return json(res, 400, envelope(400, schema("write"), {}, { errors: problems }));
      const r = await write("PUT", `/api/vps/${encodeURIComponent(name)}`, { ...payload, name });
      if (r.ok) invalidate();
      return json(
        res,
        r.ok ? 200 : r.status || 502,
        envelope(r.ok ? 200 : r.status || 502, schema("write"), { op: "overwrite", name, action: r.action, upstreamMs: r.ms, upstream: r.body }, { errors: r.errors, warnings: r.warnings }),
      );
    }

    if (req.method === "DELETE" && seg.length === 3) {
      const r = await write("DELETE", `/api/vps/${encodeURIComponent(name)}`);
      if (r.ok) invalidate();
      return json(
        res,
        r.ok ? 200 : r.status || 502,
        envelope(r.ok ? 200 : r.status || 502, schema("write"), { op: "remove", name, upstreamMs: r.ms, upstream: r.body }, { errors: r.errors, warnings: r.warnings }),
      );
    }

    if (c === "tags" && req.method === "POST" && seg.length === 4) {
      const tag = String(payload?.name ?? "");
      if (!TAG_RE.test(tag)) return json(res, 400, envelope(400, schema("write"), {}, { errors: [`标签只许 [a-z0-9-]+，收到「${tag}」`] }));
      const r = await write("POST", `/api/vps/${encodeURIComponent(name)}/tags`, { name: tag });
      if (r.ok) invalidate();
      return json(
        res,
        r.ok ? 200 : r.status || 502,
        envelope(r.ok ? 200 : r.status || 502, schema("write"), { op: "tag-add", name, tag, upstream: r.body }, { errors: r.errors, warnings: r.warnings }),
      );
    }

    if (c === "tags" && req.method === "DELETE" && seg.length === 5) {
      const tag = decodeURIComponent(d);
      const r = await write("DELETE", `/api/vps/${encodeURIComponent(name)}/tags/${encodeURIComponent(tag)}`);
      if (r.ok) invalidate();
      return json(
        res,
        r.ok ? 200 : r.status || 502,
        envelope(r.ok ? 200 : r.status || 502, schema("write"), { op: "tag-remove", name, tag, upstream: r.body }, { errors: r.errors, warnings: r.warnings }),
      );
    }

    return notFound(res, `没有这个写端点：${req.method} /${seg.join("/")}`);
  }

  return notFound(res, `没有这个写端点：${req.method} /${seg.join("/")}`);
};

createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? "/", "http://localhost");
    await route(req, res, url);
  } catch (e) {
    json(res, 500, envelope(500, schema("error"), {}, { errors: [oneLine(e)] }));
  }
}).listen(PORT, () => console.log(`vq0f03 vps 控制台: http://127.0.0.1:${PORT}  upstream=${NW}`));
