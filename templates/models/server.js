// k5411g —— 模型管家。
//
// 管的是 pi 那份 models.json（默认 ~/.pi/agent/models.json，PI_MODELS 可覆盖）：
// 看 provider、填/换密钥、从上游把模型列表拉回来。页面不显示明文，点了「复制」才给。
//
//   GET    /v1/state                    整份现状（密钥只给掩码）
//   GET    /v1/providers/:id/key        明文（只有点复制时才调）
//   PUT    /v1/providers/:id            改 baseUrl / api / compat
//   PUT    /v1/providers/:id/key        填/换密钥（空字符串 = 删）
//   POST   /v1/providers                新建
//   DELETE /v1/providers/:id            删
//   POST   /v1/providers/:id/refresh    从上游 <baseUrl>/models 拉一份列表
//   POST   /v1/providers/:id/ping       只测通不通（不写盘）
//
// 每次落盘前先把旧文件存成 models.json.bak，写是 tmp + rename（原子）。

import { copyFile, mkdir, readFile, rename, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";

import { builtinModels } from "@earendil-works/pi-ai/providers/all";

const PORT = Number(process.env.PORT ?? 4780);
const FILE = process.env.PI_MODELS ?? path.join(os.homedir(), ".pi", "agent", "models.json");
const TIMEOUT_MS = 20_000;

// ── 读 / 写 ──────────────────────────────────────────────────────────────

const empty = () => ({ providers: {} });

const load = async () => {
  try {
    const raw = await readFile(FILE, "utf8");
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return empty();
    if (!parsed.providers || typeof parsed.providers !== "object") parsed.providers = {};
    return parsed;
  } catch (error) {
    if (error.code === "ENOENT") return empty();
    throw error;
  }
};

/** 先备份再原子写：写坏一份模型配置，等于把整台机器的模型入口焊死。 */
const save = async (config) => {
  await mkdir(path.dirname(FILE), { recursive: true });
  try {
    await stat(FILE);
    await copyFile(FILE, `${FILE}.bak`);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const tmp = `${FILE}.tmp-${process.pid}`;
  await writeFile(tmp, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600 });
  await rename(tmp, FILE);
};

// ── 小工具 ───────────────────────────────────────────────────────────────

const mask = (key) => (typeof key === "string" && key ? (key.length <= 8 ? "…" : `${key.slice(0, 4)}…${key.slice(-4)}`) : "");

const trimSlash = (url) => String(url ?? "").replace(/\/+$/, "");

const isAnthropic = (api) => String(api ?? "").startsWith("anthropic");

const headersFor = (provider) =>
  isAnthropic(provider.api)
    ? { "x-api-key": provider.apiKey, "anthropic-version": "2023-06-01" }
    : { authorization: `Bearer ${provider.apiKey}` };

/** 上游 /models 的三种常见壳子，全试一遍。 */
const idsFrom = (body) => {
  const pick = (row) => (typeof row === "string" ? row : row?.id ?? row?.name);
  const list = Array.isArray(body) ? body : (body?.data ?? body?.models ?? body?.result ?? null);
  if (!Array.isArray(list)) return [];
  return [...new Set(list.map(pick).filter((id) => typeof id === "string" && id))];
};

const fetchModels = async (provider) => {
  const url = `${trimSlash(provider.baseUrl)}/models`;
  const started = Date.now();
  const response = await fetch(url, { headers: headersFor(provider), signal: AbortSignal.timeout(TIMEOUT_MS) });
  const ms = Date.now() - started;
  const text = await response.text();
  if (!response.ok) {
    const error = new Error(`上游 ${response.status}：${text.slice(0, 200).replace(/\s+/g, " ")}`);
    error.status = response.status;
    error.ms = ms;
    throw error;
  }
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    const error = new Error(`上游回的不是 JSON：${text.slice(0, 120).replace(/\s+/g, " ")}`);
    error.status = 502;
    error.ms = ms;
    throw error;
  }
  return { ids: idsFrom(body), ms };
};

const shape = (id, provider) => {
  const models = Array.isArray(provider.models) ? provider.models : [];
  return {
    id,
    baseUrl: provider.baseUrl ?? "",
    api: provider.api ?? "",
    compat: provider.compat ?? null,
    hasKey: typeof provider.apiKey === "string" && provider.apiKey.length > 0,
    keyMasked: mask(provider.apiKey),
    models: models.map((m) => (typeof m === "string" ? { id: m, name: m } : { id: m?.id ?? "", name: m?.name ?? m?.id ?? "" })),
    // 别把 provider 原样塞回浏览器：那里面就是明文 apiKey。
  };
};

// 内置 provider 里哪些现在真能用（环境里有凭据）。
const envAvailable = async () => {
  try {
    const available = await builtinModels().getAvailable();
    const byProvider = new Map();
    for (const model of available) byProvider.set(model.provider, (byProvider.get(model.provider) ?? 0) + 1);
    return [...byProvider].map(([id, count]) => ({ id, models: count }));
  } catch {
    return [];
  }
};

// ── HTTP ─────────────────────────────────────────────────────────────────

const json = (res, status, body) => {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "content-length": Buffer.byteLength(text) });
  res.end(text);
};

const readBody = (req) =>
  new Promise((resolve, reject) => {
    let raw = "";
    req.on("data", (chunk) => {
      raw += chunk;
      if (raw.length > 200_000) reject(new Error("body too large"));
    });
    req.on("end", () => {
      try {
        resolve(raw ? JSON.parse(raw) : {});
      } catch (error) {
        reject(error);
      }
    });
    req.on("error", reject);
  });

const stripPrefix = (parts) => (parts[0] === "p" && parts.length >= 3 ? parts.slice(3) : parts);

const route = async (req, res, url) => {
  const parts = stripPrefix(url.pathname.split("/").filter(Boolean));

  if (req.method === "GET" && parts.length === 0) {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(await readFile(new URL("./index.html", import.meta.url), "utf8"));
    return;
  }

  if (req.method === "GET" && parts[0] === "healthz") return json(res, 200, { ok: true, file: FILE });

  if (req.method === "GET" && parts[0] === "v1" && parts[1] === "state") {
    const config = await load();
    return json(res, 200, {
      file: FILE,
      providers: Object.entries(config.providers).map(([id, provider]) => shape(id, provider ?? {})),
      env: await envAvailable(),
    });
  }

  if (parts[0] === "v1" && parts[1] === "providers") {
    const id = parts[2] ? decodeURIComponent(parts[2]) : null;
    const action = parts[3] ?? "";

    if (req.method === "POST" && !id) {
      const body = await readBody(req);
      const newId = String(body.id ?? "").trim();
      if (!/^[a-z0-9][a-z0-9._-]{0,40}$/i.test(newId)) return json(res, 400, { error: "id 只能是字母数字和 . _ -" });
      if (!body.baseUrl) return json(res, 400, { error: "baseUrl 必填" });
      const config = await load();
      if (config.providers[newId]) return json(res, 409, { error: "这个 id 已经有了" });
      config.providers[newId] = {
        baseUrl: String(body.baseUrl).replace(/\/+$/, ""),
        api: String(body.api ?? "openai-completions"),
        apiKey: String(body.apiKey ?? ""),
        ...(body.compat && typeof body.compat === "object" ? { compat: body.compat } : {}),
        models: [],
      };
      await save(config);
      return json(res, 201, shape(newId, config.providers[newId]));
    }

    if (id && action === "key" && req.method === "GET") {
      const config = await load();
      const provider = config.providers[id];
      if (!provider) return json(res, 404, { error: "没有这个 provider" });
      res.setHeader("cache-control", "no-store");
      return json(res, 200, { id, apiKey: provider.apiKey ?? "" });
    }

    if (id && action === "key" && req.method === "PUT") {
      const body = await readBody(req);
      const config = await load();
      const provider = config.providers[id];
      if (!provider) return json(res, 404, { error: "没有这个 provider" });
      const key = String(body.apiKey ?? "").trim();
      if (key) provider.apiKey = key;
      else delete provider.apiKey;
      await save(config);
      return json(res, 200, shape(id, provider));
    }

    if (id && action === "ping" && req.method === "POST") {
      const config = await load();
      const provider = config.providers[id];
      if (!provider) return json(res, 404, { error: "没有这个 provider" });
      try {
        const { ids, ms } = await fetchModels(provider);
        return json(res, 200, { ok: true, ms, models: ids.length, sample: ids.slice(0, 5) });
      } catch (error) {
        return json(res, 200, { ok: false, ms: error.ms ?? null, error: String(error.message ?? error) });
      }
    }

    if (id && action === "refresh" && req.method === "POST") {
      const config = await load();
      const provider = config.providers[id];
      if (!provider) return json(res, 404, { error: "没有这个 provider" });
      let fetched;
      try {
        fetched = await fetchModels(provider);
      } catch (error) {
        return json(res, 502, { error: String(error.message ?? error), ms: error.ms ?? null });
      }
      // 上游有 id 就补一条最小记录；旧条目保留（它可能带着手写的 contextWindow / 价格）。
      const before = Array.isArray(provider.models) ? provider.models : [];
      const known = new Map(before.map((m) => [typeof m === "string" ? m : m?.id, typeof m === "string" ? { id: m, name: m } : m]));
      const added = fetched.ids.filter((id2) => !known.has(id2));
      const merged = [
        ...fetched.ids.map((id2) => known.get(id2) ?? { id: id2, name: id2 }),
        // 上游这次没给的条目留着：models 端点经常只报一部分。
        ...before.filter((m) => !fetched.ids.includes(typeof m === "string" ? m : m?.id)),
      ];
      provider.models = merged;
      await save(config);
      return json(res, 200, {
        ok: true,
        ms: fetched.ms,
        total: merged.length,
        added,
        kept: merged.length - added.length,
      });
    }

    if (id && !action && req.method === "PUT") {
      const body = await readBody(req);
      const config = await load();
      const provider = config.providers[id];
      if (!provider) return json(res, 404, { error: "没有这个 provider" });
      for (const field of ["baseUrl", "api"]) {
        if (typeof body[field] === "string" && body[field].trim()) provider[field] = body[field].trim().replace(/\/+$/, "");
      }
      if (body.compat === null) delete provider.compat;
      else if (body.compat && typeof body.compat === "object") provider.compat = body.compat;
      if (Array.isArray(body.models)) {
        provider.models = body.models
          .map((m) => (typeof m === "string" ? { id: m, name: m } : m))
          .filter((m) => m && typeof m.id === "string" && m.id);
      }
      await save(config);
      return json(res, 200, shape(id, provider));
    }

    if (id && !action && req.method === "DELETE") {
      const config = await load();
      if (!config.providers[id]) return json(res, 404, { error: "没有这个 provider" });
      delete config.providers[id];
      await save(config);
      return json(res, 200, { deleted: id });
    }
  }

  json(res, 404, { error: "没有这条路", path: url.pathname });
};

const server = createServer((req, res) => {
  route(req, res, new URL(req.url ?? "/", "http://localhost")).catch((error) => {
    console.error("[k5411g]", error);
    if (!res.headersSent) json(res, 500, { error: String(error?.message ?? error) });
    else res.end();
  });
});

server.listen(PORT, () => console.log(`[k5411g] 听 ${PORT} · 配置 ${FILE}`));

const shutdown = () => {
  server.close();
  process.exit(0);
};
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
