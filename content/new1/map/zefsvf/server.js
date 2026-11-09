// tailnet 面板的后端。
//
// 数据的唯一来源是本机的 tailscaled，走 `tailscale status --json`：
// 不要 API key、不联网、不多一份事实源。tailscaled 是本机的真身，
// 云端的 control plane 只是它的上级 —— 页面要看的是「此刻这台机器能连到谁」。
//
// 宿主给四个环境变量：PORT / NOUS_DATA / NOUS_ORIGIN / NOUS_SELF。
// 这里只用前两个。
//
//   GET /api/status[?fresh=1]    整张网：nodes / services / daemon
//   GET /api/ping?ip=<ip|id>     问一次「通不通、几毫秒、直连还是 DERP」
//   GET /api/tcp?ip=<vip>&port=  服务 VIP 的端口开没开（它的广告端口之一）
//   GET /api/registry            服务台账：data/registry.json（人写的那一层；每条带 doc + docExists）
//   GET /api/probe?node=&port=   台账里的某条现在通不通（白名单，不是扫描器）
//   GET /healthz                 活着吗
//   GET /…                        public/ 里的静态文件
//
// 所有地址都是相对的：宿主把 /p/<board>/<id>/ 反代进来，写绝对路径就逃出前缀了。

import { execFile } from "node:child_process";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC = path.join(HERE, "public");
const DATA = process.env.NOUS_DATA ?? path.join(HERE, "data");
const PORT = Number(process.env.PORT ?? 4781);
const TS = process.env.TAILSCALE_BIN ?? "tailscale";

/** 两个标签页同时轮询时，别让它们各打一次 tailscaled。 */
const STATUS_TTL = 2_500;
/** ping 结果留一会儿：它是网络状况，不是密码。 */
const PING_TTL = 30_000;

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
};

/** 跑一条命令，永远 resolve —— 失败也是一种答案，别让页面因为 spawn 炸掉。 */
const exec = (args, timeout) =>
  new Promise((resolve) => {
    execFile(TS, args, { timeout, maxBuffer: 8 << 20 }, (err, stdout, stderr) =>
      resolve({ err, stdout: stdout ?? "", stderr: stderr ?? "" }),
    );
  });

const trimDot = (s) => String(s ?? "").replace(/\.$/, "");
const first = (list, needle) => (list ?? []).find((i) => String(i).includes(needle)) ?? "";
const num = (n) => (typeof n === "number" && Number.isFinite(n) ? n : 0);
/** tailscale 用 0001-01-01 表示「没有」，别把它渲染成 1754 年前。 */
const zeroTime = (s) => !s || String(s).startsWith("0001");

const nodesOf = (raw) => {
  const users = raw.User ?? {};
  const one = (p, self) => ({
    id: p.ID ?? p.HostName ?? "",
    host: p.HostName ?? "",
    dns: trimDot(p.DNSName),
    ip4: first(p.TailscaleIPs, "."),
    ip6: first(p.TailscaleIPs, ":"),
    os: p.OS ?? "",
    self: !!self,
    online: !!p.Online,
    active: !!p.Active,
    tags: p.Tags ?? [],
    owner: users[p.UserID]?.LoginName ?? "",
    exitNode: !!p.ExitNode,
    exitNodeOption: !!p.ExitNodeOption,
    relay: p.Relay ?? "",
    curAddr: p.CurAddr ?? "",
    lastSeen: zeroTime(p.LastSeen) ? "" : p.LastSeen,
    expires: zeroTime(p.KeyExpiry) ? "" : p.KeyExpiry,
    rx: num(p.RxBytes),
    tx: num(p.TxBytes),
  });

  const list = [one(raw.Self ?? {}, true), ...Object.values(raw.Peer ?? {}).map((p) => one(p, false))];
  // 本机钉在最上面，然后在线的，然后按名字。滚动的列表里顺序不能每 5 秒跳一次。
  return list.sort(
    (a, b) => Number(b.self) - Number(a.self) || Number(b.online) - Number(a.online) || a.host.localeCompare(b.host),
  );
};

/**
 * `svc:` 服务不是设备：它没有机器、没有 OS、也没有「在线」，只有一个 VIP。
 * 把它混进 peer 表里，就是在骗人以为它是台机器 —— 所以它单独一段。
 * 名字和端口来自两处：ExtraRecords（DNS → VIP）和 CapMap（services/* → 端口）。
 */
const servicesOf = (raw) => {
  const out = new Map();
  const slot = (short) =>
    out.get(short) ?? { name: short, dns: "", ip4: "", ip6: "", ports: [] };

  for (const r of raw.ExtraRecords ?? []) {
    const dns = trimDot(r.Name);
    if (!dns) continue;
    const short = dns.split(".")[0];
    const addr = String(r.Value ?? "");
    const s = slot(short);
    s.dns ||= dns;
    if (addr.includes(".")) s.ip4 ||= addr;
    else if (addr.includes(":")) s.ip6 ||= addr;
    out.set(short, s);
  }

  // CapMap 不在顶层，在 Self 和每个 Peer 自己身上。端口信息（tcp:443）只在 CapMap 里。
  const caps = [raw.Self, ...Object.values(raw.Peer ?? {})].map((p) => p?.CapMap ?? {});
  for (const capMap of caps) {
    for (const [key, value] of Object.entries(capMap)) {
      if (!key.startsWith("services/")) continue;
      for (const svc of value ?? []) {
        const short = String(svc?.Name ?? "").replace(/^svc:/, "");
        if (!short) continue;
        const s = slot(short);
        for (const addr of svc?.Addrs ?? []) {
          if (addr.includes(".")) s.ip4 ||= addr;
          else if (addr.includes(":")) s.ip6 ||= addr;
        }
        for (const p of svc?.Ports ?? []) {
          const port = String(p).split(":").pop();
          if (port && !s.ports.includes(port)) s.ports.push(port);
        }
        out.set(short, s);
      }
    }
  }

  return [...out.values()]
    .filter((s) => s.ip4 || s.ip6)
    .sort((a, b) => a.name.localeCompare(b.name));
};

const snapshot = (raw) => {
  const nodes = nodesOf(raw);
  return {
    v: 1,
    ok: true,
    at: new Date().toISOString(),
    stale: false,
    error: null,
    tailnet: {
      name: raw.CurrentTailnet?.Name ?? "",
      suffix: raw.MagicDNSSuffix ?? "",
      magicdns: !!raw.CurrentTailnet?.MagicDNSEnabled,
    },
    daemon: {
      version: raw.Version ?? "",
      backend: raw.BackendState ?? "",
      tun: !!raw.TUN,
      health: raw.Health ?? [],
    },
    self: nodes.find((n) => n.self) ?? null,
    nodes,
    services: servicesOf(raw),
    counts: { online: nodes.filter((n) => n.online).length, total: nodes.length },
  };
};

// ── 快照：tailscaled 挂了也得给你看上次的样子，而不是一片空白 ──────────────
//
// 落 NOUS_DATA（= 这个文件夹下的 data/），所以 `cp -r map/zefsvf ~/zefsvf` 连历史一起走。

const SNAP = path.join(DATA, "status.json");
let lastWritten = "";
const persist = async (payload) => {
  const body = JSON.stringify(payload, null, 2);
  if (body === lastWritten) return;
  try {
    await mkdir(DATA, { recursive: true });
    await writeFile(SNAP, body);
    lastWritten = body;
  } catch {
    // 盘写不进去不该让页面变瞎。
  }
};

const restore = async () => {
  try {
    return JSON.parse(await readFile(SNAP, "utf8"));
  } catch {
    return null;
  }
};

const empty = (error) => ({
  v: 1,
  ok: false,
  stale: true,
  error,
  at: null,
  tailnet: { name: "", suffix: "", magicdns: false },
  daemon: { version: "", backend: "", tun: false, health: [] },
  self: null,
  nodes: [],
  services: [],
  counts: { online: 0, total: 0 },
});

const cache = { at: 0, payload: null };

const load = async (fresh = false) => {
  if (!fresh && cache.payload && Date.now() - cache.at < STATUS_TTL) return cache.payload;

  const { err, stdout, stderr } = await exec(["status", "--json"], 8_000);
  let raw = null;
  if (stdout.trim()) {
    try {
      raw = JSON.parse(stdout);
    } catch {
      raw = null;
    }
  }

  if (!raw) {
    const reason =
      err?.code === "ENOENT"
        ? "这台机器上没有 tailscale 命令"
        : err?.killed
          ? "tailscale status 超时（tailscaled 没回话？）"
          : (stderr.trim().split("\n")[0] || err?.message || "tailscale status 没给出 JSON");

    const back = await restore();
    if (back) {
      return { ...back, ok: false, stale: true, error: reason, ageMs: Date.now() - Date.parse(back.at ?? 0) };
    }
    return empty(reason);
  }

  const payload = snapshot(raw);
  cache.at = Date.now();
  cache.payload = payload;
  persist(payload);
  return payload;
};

// ── ping：这一页存在的理由 ────────────────────────────────────────────────
//
// `tailscale ping` 说的是真话：是直连（`via 1.2.3.4:41641`）还是绕 DERP（`via DERP(hkg)`），
// 以及多少毫秒。两发、每发 1 秒封顶 —— 离线的机器不能把页面卡 6 秒。

const pingCache = new Map();

const parsePong = (text) => {
  const hits = [];
  for (const line of text.split("\n")) {
    const m = /pong from (\S+) \((\S+)\) via (.+?) in ([\d.]+)(ms|µs|s)\b/.exec(line);
    if (!m) continue;
    const [, host, ip, via, value, unit] = m;
    const ms = unit === "s" ? Number(value) * 1000 : unit === "µs" ? Number(value) / 1000 : Number(value);
    const derp = /^DERP\((.+)\)$/.exec(via.trim());
    hits.push({
      host,
      ip,
      direct: !derp,
      region: derp ? derp[1] : "",
      addr: derp ? "" : via,
      ms: Math.round(ms * 10) / 10,
    });
  }
  // 直连优先，其次最快。第一发常常还挂在 DERP 上，第二发就切直连了 —— 我们要的是好结果。
  hits.sort((a, b) => Number(b.direct) - Number(a.direct) || a.ms - b.ms);
  return hits[0] ?? null;
};

const doPing = async (ip) => {
  const hit = pingCache.get(ip);
  if (hit && Date.now() - hit.at < PING_TTL) return hit;

  const { err, stdout, stderr } = await exec(["ping", "-c", "2", "--timeout=1s", ip], 6_000);
  const text = `${stdout}\n${stderr}`;
  const pong = parsePong(text);

  const out = pong
    ? { ok: true, at: Date.now(), ip, ...pong }
    : {
        ok: false,
        at: Date.now(),
        ip,
        // 第一行不是 pong 的话，它就是原因（`ping "100.x" timed out` / `no matching peer` / …）。
        error:
          text
            .split("\n")
            .map((l) => l.trim())
            .filter(Boolean)
            .find((l) => !/^pong from /.test(l)) || err?.message || "没有回音",
      };

  pingCache.set(ip, out);
  return out;
};

// ── 服务 VIP 探活 ──────────────────────────────────────────────────────────
//
// `svc:` 不是 peer，`tailscale ping` 对它只会超时 —— 别拿一个注定失败的按钮糊弄人。
// 它真正能回答的问题是「那个端口开没开」，那就真的去 connect 一下。

const tcpCache = new Map();

const tcpProbe = (host, port, timeout = 3_000) =>
  new Promise((resolve) => {
    const t0 = performance.now();
    const sock = net.connect({ host, port });
    let settled = false;
    const done = (ok, error) => {
      if (settled) return;
      settled = true;
      sock.destroy();
      resolve({ ok, ms: Math.round(performance.now() - t0), error: error ?? null });
    };
    sock.setTimeout(timeout, () => done(false, "超时"));
    sock.once("connect", () => done(true));
    sock.once("error", (e) => done(false, e.code ?? e.message));
  });

// ── 服务台账：tailscaled 不知道「哪个端口值得点」 ────────────────────────────
//
// tailscaled 说的是「谁在、IP 是什么」；它不会告诉你 j1900 的 8443 上是 nw 面板、
// 该用 https 还是 http、要不要带 token、重启命令是什么。这一层是人写下来的，
// 住 data/registry.json —— 和 status.json 一样落 NOUS_DATA，
// 所以 `cp -r map/zefsvf ~/zefsvf` 连台账一起走。
//
// 台账同时是**白名单**：/api/probe 只许探台账里写过的 (node, port)，
// 这个端点因此仍然是探活，不是端口扫描器。

const REGISTRY = path.join(DATA, "registry.json");

/**
 * 台账里的 `doc` 是给 agent 读的那份说明（skill）的绝对路径。
 * 顺手确认它还在：复制一个死路径给 agent，比没有按钮更糟。
 *
 * 路径只有 registry.json 一个来源，不接受请求参数 —— 否则这就成了「任意文件存不存在」
 * 的探针，和 /api/probe 的白名单是同一条纪律。
 */
const withDoc = async (e) => {
  if (typeof e.doc !== "string" || !e.doc) return e;
  const docExists = await stat(e.doc).then((s) => s.isFile()).catch(() => false);
  return { ...e, docExists };
};

const readRegistry = async () => {
  try {
    const doc = JSON.parse(await readFile(REGISTRY, "utf8"));
    const live = (doc.entries ?? []).filter((e) => e?.name && e?.node && e?.port);
    const entries = await Promise.all(live.map(withDoc));
    return { ok: true, registryPath: REGISTRY, entries };
  } catch (e) {
    return {
      ok: false,
      registryPath: REGISTRY,
      entries: [],
      // 台账不存在是正常状态，不是故障：这一页本来就能只看机器。
      error: e?.code === "ENOENT" ? "还没有台账" : (e?.message ?? String(e)),
    };
  }
};

// ── HTTP ──────────────────────────────────────────────────────────────────

const json = (res, code, body) => {
  const data = JSON.stringify(body);
  res.writeHead(code, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(data),
  });
  res.end(data);
};

const serveStatic = async (res, rel) => {
  // 反代进来的路径长这样：/app.js、/、/style.css —— 先剥掉开头的斜杠，再落到 public 里。
  // 注意别用 path.resolve(PUBLIC, "/app.js")：绝对路径会直接把 PUBLIC 顶掉。
  const clean = path.normalize(decodeURIComponent(rel)).replace(/^(\.\.[/\\])+/, "").replace(/^[/\\]+/, "");
  const file = path.join(PUBLIC, clean);
  // 反代进来的路径可能是 /x/y/app.js，normalize 之后仍要在 public 里面。
  if (file !== PUBLIC && !file.startsWith(PUBLIC + path.sep)) {
    return json(res, 403, { error: "不许出这个文件夹" });
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, {
      "content-type": TYPES[path.extname(file)] ?? "application/octet-stream",
      // 块一改代码宿主就把进程收掉，所以这里不需要缓存策略，不缓存最省心。
      "cache-control": "no-store",
      "content-length": body.length,
    });
    res.end(body);
  } catch {
    res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    res.end("404");
  }
};

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");

  try {
    if (url.pathname === "/healthz") return json(res, 200, { ok: true, pid: process.pid });

    if (url.pathname === "/api/status") {
      const payload = await load(url.searchParams.get("fresh") === "1");
      return json(res, 200, payload);
    }

    if (url.pathname === "/api/tcp") {
      const snap = await load();
      const target = url.searchParams.get("ip") ?? "";
      const svc = snap.services.find((s) => s.ip4 === target || s.ip6 === target || s.name === target);
      if (!svc) return json(res, 404, { ok: false, error: "这张网里没有这个服务" });

      const wanted = url.searchParams.get("port") ?? "";
      // 只许探它自己广告过的端口：别把这个端点变成内网的端口扫描器。
      const port = svc.ports.includes(wanted) ? Number(wanted) : Number(svc.ports[0] ?? 443);
      const host = svc.ip4 || svc.ip6;
      const key = `${host}:${port}`;
      const hit = tcpCache.get(key);
      if (hit && Date.now() - hit.at < 15_000) return json(res, 200, hit);

      const out = { at: Date.now(), ip: host, port, name: svc.name, ...(await tcpProbe(host, port)) };
      tcpCache.set(key, out);
      return json(res, 200, out);
    }

    if (url.pathname === "/api/registry") {
      const reg = await readRegistry();
      return json(res, 200, { at: Date.now(), ...reg });
    }

    if (url.pathname === "/api/probe") {
      const node = url.searchParams.get("node") ?? "";
      const port = Number(url.searchParams.get("port") ?? "");
      const reg = await readRegistry();
      const entry = reg.entries.find((e) => e.node === node && Number(e.port) === port);
      if (!entry) return json(res, 404, { ok: false, error: "台账里没有这一条" });

      const key = `${entry.node}:${port}`;
      const hit = tcpCache.get(key);
      if (hit && Date.now() - hit.at < 15_000) return json(res, 200, hit);

      const out = { at: Date.now(), name: entry.name, ip: entry.node, port, ...(await tcpProbe(entry.node, port)) };
      tcpCache.set(key, out);
      return json(res, 200, out);
    }

    if (url.pathname === "/api/ping") {
      const target = url.searchParams.get("ip") ?? "";
      if (!target) return json(res, 400, { ok: false, error: "缺 ip" });

      // 只 ping 这张网里真实存在的地址：别把这个端点变成任意地址探测器。
      const snap = await load();
      const node =
        snap.nodes.find((n) => n.ip4 === target || n.ip6 === target || n.id === target) ??
        snap.services.find((s) => s.ip4 === target || s.ip6 === target || s.name === target);

      const ip = node?.ip4 || node?.ip6 || "";
      if (!ip) return json(res, 404, { ok: false, error: "这张网里没有这个地址" });

      return json(res, 200, { ...(await doPing(ip)), node: node.host ?? node.name ?? "" });
    }

    if (url.pathname.startsWith("/api/")) return json(res, 404, { ok: false, error: "没有这个接口" });

    return serveStatic(res, url.pathname === "/" ? "index.html" : url.pathname);
  } catch (e) {
    return json(res, 500, { ok: false, error: e?.message ?? String(e) });
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`tailnet → http://127.0.0.1:${PORT}  (bin=${TS}, data=${DATA})`);
});
