// k5411g —— Nous Warehouse 模型编目。只看编目，不掺本机。
//
//   编目 = j1900 上的 Nous Warehouse（/opt/Data-001/models.json），REST 面 /api/model。
//
// 三条硬规矩（改代码前先读）：
//   1. 这一页没有任何本机配置的读或写。上一版拿 ~/.pi/agent/models.json 做对账，
//      本机那半边已经整个拿掉了 —— 别再加回来，除非人明确要求「再对比本机」。
//   2. 密钥只从 /api/model/{名}/secret 现拿，拿下就显示、不缓存、不落盘。
//      列表里的 apiKeyRef 永远是引用或 <redacted:…>，那不是 bug。
//   3. DELETE 是零确认直通 + 打一个 git commit，不可逆。所以前端用弹窗把后果摆清（列的
//      是「改哪个文件、会打 commit、谁还在引用」），点确认才执行；服务端再核一次 name 非空。
//      弹窗自己画，不用 window.confirm / prompt。
//
// 接口：
//   GET    /                       这一页
//   GET    /healthz
//   GET    /v1/state               编目全量（条目 + 模型 id + usedBy + 密钥形状）
//   GET    /v1/secret/:name        明文，唯一出口（眼睛按钮才点）
//   POST   /v1/recheck  {name?}    编目自己跟上游比；不带 name = 全量
//   POST   /v1/delete   {name}     删一条（服务端核 name 非空，其余交给面板）
//
// 环境：PORT（默认 4780）、NW_BASE（默认 tailnet 8443）、NW_TOKEN（走 panel-gate 时才要）

import http from "node:http";

const PORT = Number(process.env.PORT || 4780);
const NW_BASE = (process.env.NW_BASE || "https://j1900.tail8d0018.ts.net:8443").replace(/\/+$/, "");
const NW_TOKEN = process.env.NW_TOKEN || "";
const READ_MS = 15000;
const WRITE_MS = 130000; // register/rm 会扫上游 + git commit，面板自己 2 分钟就放弃
const RECHECK_MS = 130000;

const nwHeaders = () => (NW_TOKEN ? { authorization: `Bearer ${NW_TOKEN}` } : {});

// 面板的 envelope 有两种「失败」：真失败，和「落账了但 git push 没推上去」。
// 后者 ok:false 但 data.written === true —— 当成失败报出去，人会以为没写、反复重跑。
const unwrap = (body, status) => {
  const data = body?.data ?? {};
  const errors = (body?.errors ?? []).map((e) => String(e?.message ?? e));
  if (body?.ok === false && (data.written === true || data.written === undefined && data.action && !errors.length)) {
    return { ...data, partial: errors.length > 0, warnings: errors };
  }
  if (body?.ok === false) {
    if (data.written === true) return { ...data, partial: true, warnings: errors };
    const err = new Error(errors[0] || `编目回了 HTTP ${status}`);
    err.status = status;
    throw err;
  }
  return { ...data, warnings: errors };
};

async function nw(path, { method = "GET", body, timeout = READ_MS } = {}) {
  const response = await fetch(`${NW_BASE}${path}`, {
    method,
    headers: { ...nwHeaders(), ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(timeout),
  });
  const text = await response.text();
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error(`编目回的不是 JSON（HTTP ${response.status}）：${text.slice(0, 160).replace(/\s+/g, " ")}`);
  }
  return unwrap(parsed, response.status);
}

const isRef = (s) => /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(s) || /^\$\{/.test(s);
const isCmd = (s) => /^!/.test(s);

function keyShape(apiKeyRef) {
  const raw = String(apiKeyRef ?? "");
  if (!raw) return { shape: "none", text: "" };
  if (raw.startsWith("<redacted")) return { shape: "literal", text: "字面量（编目不回显）" };
  if (isCmd(raw)) return { shape: "cmd", text: raw };
  if (isRef(raw)) return { shape: "ref", text: raw };
  return { shape: "literal", text: "字面量" };
}

// ── 编目 ──────────────────────────────────────────────────────────────────
async function loadLedger() {
  try {
    const data = await nw("/api/model");
    const rows = Array.isArray(data.models) ? data.models : [];
    const entries = rows.map((row) => ({
      name: String(row.name ?? ""),
      baseUrl: String(row.baseUrl ?? ""),
      api: String(row.api ?? ""),
      key: keyShape(row.apiKeyRef),
      models: Array.isArray(row.models) ? row.models.map(String) : [],
      usedBy: Array.isArray(row.usedBy) ? row.usedBy.map(String) : [],
    }));
    entries.sort((a, b) => a.name.localeCompare(b.name));
    return { ok: true, base: NW_BASE, entries };
  } catch (error) {
    return { ok: false, base: NW_BASE, entries: [], error: String(error.message ?? error) };
  }
}

// ── 动作 ──────────────────────────────────────────────────────────────────
async function doRecheck(name) {
  const data = await nw("/api/model/recheck", {
    method: "POST",
    body: name ? { name } : {},
    timeout: RECHECK_MS,
  });
  // 202 的作业：面板已经把 nw 跑完了才回，所以这里拿到的是终局。
  const drifts = Array.isArray(data.drifts) ? data.drifts : Array.isArray(data.diff) ? data.diff : [];
  return {
    ok: true,
    scope: name || "全量",
    drifts: drifts.map((d) => ({
      kind: String(d.kind ?? d.type ?? "?"),
      model: d.model ?? d.id ?? "",
      name: d.name ?? "",
    })),
    summary: data.summary ?? null,
  };
}

async function doDelete(name) {
  const clean = String(name ?? "").trim();
  if (!clean) throw new Error("没给名字");
  const data = await nw(`/api/model/${encodeURIComponent(clean)}`, { method: "DELETE", timeout: WRITE_MS });
  const outcome = data.rmOutcome ?? data;
  return {
    ok: true,
    name: outcome.name ?? clean,
    action: outcome.action ?? null,
    remaining: outcome.remaining ?? null,
    partial: Boolean(data.partial),
    warnings: data.warnings ?? [],
  };
}

async function doSecret(name) {
  const data = await nw(`/api/model/${encodeURIComponent(name)}/secret`);
  const key = data.key ?? data.apiKey ?? "";
  return { ok: true, name, key: String(key), shape: keyShape(key).shape };
}

// ── HTTP ──────────────────────────────────────────────────────────────────
const readBody = (req) =>
  new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (c) => {
      size += c.length;
      if (size > 64 * 1024) reject(new Error("body 太大"));
      else chunks.push(c);
    });
    req.on("end", () => {
      const text = Buffer.concat(chunks).toString("utf8").trim();
      if (!text) return resolve({});
      try {
        resolve(JSON.parse(text));
      } catch {
        reject(new Error("body 不是 JSON"));
      }
    });
    req.on("error", reject);
  });

const send = (res, code, body) => {
  const text = JSON.stringify(body);
  res.writeHead(code, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
  res.end(text);
};

const stripPrefix = (url) => {
  // 宿主把这一块挂在 /p/<board>/<id>/ 下面，但进程只认自己那截路径。
  const self = process.env.NOUS_SELF;
  if (self && url.startsWith(self)) {
    const rest = url.slice(self.length);
    return rest.startsWith("/") ? rest : `/${rest}`;
  }
  return url;
};

async function route(req, res) {
  const url = stripPrefix(req.url || "/");
  const path = url.split("?")[0].replace(/\/+$/, "") || "/";

  if (path === "/healthz") return send(res, 200, { ok: true, ledger: NW_BASE });

  if (path === "/v1/state" && req.method === "GET") {
    const ledger = await loadLedger();
    return send(res, 200, {
      at: new Date().toISOString(),
      ledger,
      summary: {
        entries: ledger.entries.length,
        models: ledger.entries.reduce((n, e) => n + e.models.length, 0),
      },
    });
  }

  if (path === "/v1/recheck" && req.method === "POST") {
    const body = await readBody(req);
    return send(res, 200, await doRecheck(body.name ? String(body.name) : ""));
  }

  if (path === "/v1/delete" && req.method === "POST") {
    const body = await readBody(req);
    return send(res, 200, await doDelete(body.name));
  }

  const secret = path.match(/^\/v1\/secret\/(.+)$/);
  if (secret && req.method === "GET") return send(res, 200, await doSecret(decodeURIComponent(secret[1])));

  if (path === "/" && req.method === "GET") {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
    return res.end(await page());
  }

  send(res, 404, { ok: false, error: `没有这条路：${req.method} ${path}` });
}

const page = async () => {
  const { readFile } = await import("node:fs/promises");
  const { fileURLToPath } = await import("node:url");
  const dir = fileURLToPath(new URL(".", import.meta.url));
  return readFile(`${dir}index.html`, "utf8");
};

http
  .createServer((req, res) => {
    route(req, res).catch((error) => {
      const code = error?.status && error.status >= 400 && error.status < 600 ? error.status : 502;
      send(res, code, { ok: false, error: String(error?.message ?? error) });
    });
  })
  .listen(PORT, "0.0.0.0", () => {
    console.log(`k5411g 模型编目 :${PORT} → ${NW_BASE}`);
  });
