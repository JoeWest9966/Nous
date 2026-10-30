// 编目 → pi-ai provider。**这是 qrd3wh 唯一的模型来源**。
//
// 上一版读 `~/.pi/agent/models.json`（models-json.js，本文件的前身，已删）。改的原因
// 不是「文件不好用」，是「真相有两份」：那台机器上谁手写的 23 个模型，跟 Nous Warehouse
// 里 148 个登记过的，永远是两份会漂的事实。现在编目是唯一真相，Nous 是它的投影。
//
// 三条纪律（改之前先读）：
//
//  1. **密钥只在内存里。** 启动时向面板要一次（`/api/model/{名}/secret`），之后
//     provider 的 `auth.resolve()` 每次从这张 Map 里取 —— 不落盘、不进日志、不进
//     session 记录。代价说清：**换密钥要重启这个进程**（编目里轮换了 key，这里不
//     重启就一直用旧的，直到 401）。为此留了 `POST /v1/catalog/refresh`。
//  2. **编目说不出来的字段靠注解，不靠猜。** 上游 /models 不报「能不能思考」，
//     所以 `reasoning` / `thinkingLevelMap` / `cost` / `compat` / `input` 是编目里
//     人写的注解（`nw model register` 覆盖写会保住它们）。这里只做**格式映射**，
//     绝不按 id 猜能力 —— 猜错一个，面板上就多一档点得下去、跑起来 400 的思考。
//  3. **不认识的方言跳过并说清。** 编目的 `api` 值域就两个，但这里是另一个进程，
//     值域漂了要看得见（跳过 + 原因），不是静悄悄少一个供应商。

import { createProvider } from "@earendil-works/pi-ai/models";

// 编目在哪儿：tailnet 上那把 8443（免 token）。走 panel-gate（8080）才需要 NW_TOKEN。
const DEFAULT_BASE = "https://j1900.tail8d0018.ts.net:8443";

// api 字符串 → 实现模块。用到了才加载：一个模块拖着整个 sdk。
const LOADERS = {
  "openai-completions": () => import("@earendil-works/pi-ai/api/openai-completions"),
  "openai-responses": () => import("@earendil-works/pi-ai/api/openai-responses"),
  "anthropic-messages": () => import("@earendil-works/pi-ai/api/anthropic-messages"),
  "google-generative-ai": () => import("@earendil-works/pi-ai/api/google-generative-ai"),
};

const DEFAULT_COST = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

/** 只认真整数（`"16384"` 这种上游偶发的字符串也收）；其它一律 undefined，不四舍五入。 */
const asInt = (v) => {
  if (typeof v === "number" && Number.isFinite(v)) return Math.trunc(v);
  if (typeof v === "string" && /^\d+$/.test(v)) return Number(v);
  return undefined;
};
const pickInt = (...candidates) => {
  for (const v of candidates) {
    const n = asInt(v);
    if (n) return n;
  }
  return undefined;
};

const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/**
 * 能不能思考。三种上游写法，全是**显式字段**：
 *   - `reasoning: true` —— 编目注解（volc-ark 那种什么都不报的，全靠这个）
 *   - `supports_reasoning: true` / `only_reasoning: true` —— workbuddy 那种网关
 * 没有这三样就是不能 —— 不按 id 里有没有 `r1` / `thinking` 猜。
 */
const isReasoning = (m) =>
  m.reasoning === true || m.supports_reasoning === true || m.only_reasoning === true;

/**
 * 档位映射。pi-ai 的规矩（`getSupportedThinkingLevels`）：非 reasoning 只有 off；
 * reasoning 的上 low..high 默认都在，**`xhigh` / `max` 必须显式写进 map 才算**。
 * 所以这里只搬上游真报了的那几档，不补默认值 —— 补了就等于替供应商承诺它没承诺的档。
 */
const thinkingMapOf = (m) => {
  if (m.thinkingLevelMap && typeof m.thinkingLevelMap === "object") return m.thinkingLevelMap;
  const raw = Array.isArray(m.reasoning_supported_efforts)
    ? m.reasoning_supported_efforts
    : Array.isArray(m.supported_reasoning_levels)
      ? m.supported_reasoning_levels.map((x) => x?.effort)
      : null;
  if (!raw) return undefined;
  const out = {};
  for (const level of raw) if (THINKING_LEVELS.includes(level)) out[level] = level;
  return Object.keys(out).length ? out : undefined;
};

/** 模态。三种形状：注解的 `input[]`、上游 `modalities.input[]`、上游 `modalities.input_modalities[]`。 */
const inputOf = (m) => {
  if (Array.isArray(m.input) && m.input.length) return m.input;
  const mod = m.modalities;
  const raw = Array.isArray(mod?.input) ? mod.input : Array.isArray(mod?.input_modalities) ? mod.input_modalities : null;
  if (raw && raw.length) return raw;
  if (m.supports_images === true) return ["text", "image"];
  return ["text"];
};

/** 编目里的一个模型对象 → pi-ai 的 chat model。缺字段补保守默认值（宁可少给，不给错）。 */
const toModel = (providerId, entry, m) => {
  const map = thinkingMapOf(m);
  return {
    type: "chat",
    id: m.id,
    name: m.name ?? m.display_name ?? m.id,
    provider: providerId,
    api: entry.api,
    baseUrl: entry.baseUrl,
    reasoning: isReasoning(m),
    input: inputOf(m),
    cost: m.cost ?? DEFAULT_COST,
    contextWindow:
      pickInt(m.contextWindow, m.context_length, m.max_allowed_size, m.token_limits?.context_window) ?? 128_000,
    maxTokens:
      pickInt(m.maxTokens, m.max_output_tokens, m.token_limits?.max_output_token_length) ?? 8_192,
    ...(map ? { thinkingLevelMap: map } : {}),
    ...(m.compat ? { compat: m.compat } : {}),
    ...(m.inputLimits ? { inputLimits: m.inputLimits } : {}),
  };
};

/** 面板的 envelope → data；ok:false 也认（partial 时 data 里仍有一半事实），错误单独回。 */
const unwrap = (body) => {
  if (body && typeof body === "object" && "ok" in body) {
    if (body.ok) return { data: body.data ?? {}, error: null };
    const first = Array.isArray(body.errors) && body.errors[0] ? body.errors[0] : null;
    return { data: body.data ?? {}, error: first?.message ?? "面板回了 ok:false" };
  }
  return { data: body ?? {}, error: null };
};

/**
 * 拉一次编目 + 每条条目的密钥，把 provider 装进 `models`。
 *
 * 返回值刻意分成四块，因为面板要把「编目到底通不通」讲清楚，不能只说「0 个模型」：
 *   `installed` 装上了什么 / `skipped` 跳过了什么（带原因）/ `at` 什么时候拉的 /
 *   `error` 编目取不回来时的原话。
 */
export async function installCatalog(models, opts = {}) {
  const base = (opts.base ?? process.env.NW_BASE ?? DEFAULT_BASE).replace(/\/+$/, "");
  const token = opts.token ?? process.env.NW_TOKEN ?? "";
  const fetchImpl = opts.fetch ?? globalThis.fetch;
  const headers = token ? { authorization: `Bearer ${token}` } : {};
  const at = Date.now();

  /** 密钥只在调用方这份 Map 里：provider 的 resolve 每次从这里取。 */
  const keys = opts.keys ?? new Map();

  let body;
  try {
    const res = await fetchImpl(`${base}/api/model?models=full`, { headers });
    body = await res.json();
  } catch (error) {
    return { base, at, installed: [], skipped: [], keys, error: `编目取不回：${error.message ?? error}` };
  }
  const { data, error } = unwrap(body);
  const rows = Array.isArray(data.models) ? data.models : [];
  if (!rows.length) {
    return { base, at, installed: [], skipped: [], keys, error: error ?? "编目里一条供应商都没有" };
  }

  const installed = [];
  const skipped = [];
  for (const row of rows) {
    const id = String(row.name ?? "").trim();
    if (!id) continue;
    const loader = LOADERS[row.api];
    if (!loader) {
      skipped.push({ id, reason: `不认识的方言：${row.api || "（空）"}` });
      continue;
    }
    if (!row.baseUrl) {
      skipped.push({ id, reason: "没有 baseUrl" });
      continue;
    }
    let streams;
    try {
      streams = await loader();
    } catch (loadError) {
      skipped.push({ id, reason: `加载 ${row.api} 实现失败：${loadError.message}` });
      continue;
    }

    // 密钥：明文只在这次请求里过一手，落进内存 Map。
    let hasKey = false;
    try {
      const secretRes = await fetchImpl(`${base}/api/model/${encodeURIComponent(id)}/secret`, { headers });
      const secret = unwrap(await secretRes.json());
      const value = String(secret.data?.key ?? "").trim();
      if (value) {
        keys.set(id, value);
        hasKey = true;
      } else if (secret.error) {
        skipped.push({ id, reason: `密钥取不回：${secret.error}` });
      }
    } catch (secretError) {
      skipped.push({ id, reason: `密钥取不回：${secretError.message ?? secretError}` });
    }

    // 只收 id 非空的模型：编目里 `local: true` 的别名也在这里（它们上游不列，但真能调）。
    const list = (Array.isArray(row.modelsDetail) ? row.modelsDetail : [])
      .filter((m) => m && typeof m.id === "string" && m.id)
      .map((m) => toModel(id, row, m));

    models.setProvider(
      createProvider({
        id,
        name: id,
        baseUrl: row.baseUrl,
        auth: {
          apiKey: {
            name: `${id} 的密钥（编目）`,
            resolve: async () => {
              const value = keys.get(id);
              return value ? { auth: { apiKey: value }, source: "nous-warehouse" } : undefined;
            },
          },
        },
        models: list,
        api: { stream: streams.stream, streamSimple: streams.streamSimple },
      }),
    );
    installed.push({ id, api: row.api, models: list.length, hasKey });
  }
  return { base, at, installed, skipped, keys, error: error ?? null };
}
