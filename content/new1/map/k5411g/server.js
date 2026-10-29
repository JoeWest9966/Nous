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
//   4. 登记（POST /v1/register）= 面板的 POST /api/model，薄壳一根：只做「明显的错不值得
//      跑一趟上游」那种本地校验（baseUrl 语法 / 方言值域 / 名字字符 / 密钥非空），
//      其余全交给 nw 的五道闸 —— 别在这里复刻闸的语义，复刻的那份迟早跟闸漂。
//      闸簿（data.gates[]）原样回给前端，失败也回（失败时 gates 里那道是 fail）。
//
// 接口：
//   GET    /                       这一页
//   GET    /healthz
//   GET    /v1/state               编目全量（条目 + 模型 id + usedBy + 密钥形状）
//   GET    /v1/secret/:name        明文，唯一出口（眼睛按钮才点）
//   POST   /v1/recheck  {name?}    编目自己跟上游比；不带 name = 全量
//   POST   /v1/delete   {name}     删一条（服务端核 name 非空，其余交给面板）
//   POST   /v1/register {baseUrl, api, key, name?}   登记一条，回闸簿（ok:false 也回 200，
//                                  带着 data.gates 让前端把哪道闸没过摆出来）
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
    err.data = data; // 失败时那道 fail 的闸在 data.gates[] 里，别半路扔掉
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

const gateRows = (list) =>
  (Array.isArray(list) ? list : []).map((g) => ({
    id: String(g?.id ?? ""),
    status: String(g?.status ?? "skip"),
    message: String(g?.message ?? ""),
  }));

// 登记：把面板的 POST /api/model 包一层。这里**不复刻五道闸**，只挡住「注定白跑一趟
// 上游 + 打一次 commit」的明显错；真闸的结论（含跳过的那几道）从 data.gates[] 回来。
async function doRegister(input) {
  const baseUrl = String(input.baseUrl ?? "").trim();
  const api = String(input.api ?? "").trim();
  const name = String(input.name ?? "").trim();
  const key = String(input.key ?? "").trim();
  const fail = (error) => ({ ok: false, error, gates: [] });

  if (!baseUrl) return fail("没给 baseUrl");
  if (!/^https?:\/\/[^\s/]+/i.test(baseUrl)) return fail(`baseUrl 要 http(s)://<host>…：${baseUrl.slice(0, 80)}`);
  if (api !== "openai-completions" && api !== "anthropic-messages")
    return fail(`方言 ${api || "(空)"} 不认：只有 openai-completions / anthropic-messages（手选，不探测）`);
  if (name && (/\s|\//.test(name) || name === "." || name === ".."))
    return fail(`条目名不合法：${name.slice(0, 40)}（非空、无空白、无斜杠、不是 . 或 ..）`);
  if (!key) return fail("没给密钥：register 只从 stdin 收，这一页就是那根 stdin");

  const payload = { baseUrl, api, key };
  if (name) payload.name = name;

  try {
    const data = await nw("/api/model", { method: "POST", body: payload, timeout: WRITE_MS });
    const models = Array.isArray(data.models) ? data.models : [];
    return {
      ok: true,
      name: String(data.name ?? name),
      baseUrl: String(data.baseUrl ?? baseUrl),
      api: String(data.api ?? api),
      written: Boolean(data.written),
      updated: Boolean(data.updated),
      action: data.action ?? null,
      catalogPath: data.catalogPath ?? null,
      modelCount: models.length,
      modelIds: models.map((m) => String(m?.id ?? m)).slice(0, 8),
      gates: gateRows(data.gates),
      partial: Boolean(data.partial),
      warnings: data.warnings ?? [],
    };
  } catch (error) {
    // 闸没过 = data.gates 里那道 fail，其余 skip —— 原样带回，前端照着摆
    return { ok: false, error: String(error.message ?? error), gates: gateRows(error.data?.gates) };
  }
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

  if (path === "/v1/register" && req.method === "POST") {
    const body = await readBody(req);
    // 200 也用于 ok:false：前端要的是闸簿，不是一个状态码
    return send(res, 200, await doRegister(body));
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
