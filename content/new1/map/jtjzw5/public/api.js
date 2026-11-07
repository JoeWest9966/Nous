// api.js —— 这一页的数据层。只做三件事：问宿主 qrd3wh 在哪、把 `v1/presets/<段>` 请求发过去、
// 把服务端那一行错误原样交出来。
//
// **不判断任何业务。** 「这条引用还在不在」「名字合不合规」全在引擎那一边（`qrd3wh/presets.js`），
// 这一页只翻译、只显示。本地再写一套规则 = 第二份真相。

/**
 * 这一页管的是哪一段。`Nous-Template` 一个仓放两段，段名 = 文件夹名 = URL 的第一段。
 * 这一段叫 `Ultimate-Template`；会话预设那一段归 `3699b9`，两页不互相窜。
 */
export const SECTION = "Ultimate-Template";

/** 宿主把 qrd3wh 挂在别的块底下，路径只能从 ctx 里拿 —— 这里不许出现硬编码的块地址。 */
export const HOST_BLOCK_ID = "qrd3wh";

export class ApiError extends Error {
  constructor(message, { status = 0, code = "api", body = null } = {}) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.body = body;
  }
}

/** 段还没装好（`sections.json` 里 `checker: null`）。这不是错误，是「还没长出来」。 */
export class SectionNotReady extends ApiError {
  constructor(message = "这一段还没装好") {
    super(message, { status: 503, code: "section-not-ready" });
    this.name = "SectionNotReady";
  }
}

/** 服务端说啥就是啥：`{error}` 里那一行不加工、不加前缀（那是唯一一处人话）。 */
export async function readJson(response) {
  const text = await response.text();
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    throw new ApiError(text.slice(0, 200) || `HTTP ${response.status}`, { status: response.status, code: "not-json" });
  }
  if (!response.ok) {
    const message = String(body?.error ?? `HTTP ${response.status}`);
    if (response.status === 503) throw new SectionNotReady(message);
    throw new ApiError(message, { status: response.status, body });
  }
  return body;
}

/** 宿主的 ctx 里，块是数组或者 `{id: block}` 两种形状都见过 —— 都认。 */
export const blocksOf = (ctx) => {
  const raw = ctx?.blocks;
  if (Array.isArray(raw)) return raw;
  if (raw && typeof raw === "object") return Object.entries(raw).map(([id, block]) => ({ id, ...block }));
  return [];
};

export const blockUrl = (blocks, id) => {
  const block = blocks.find((entry) => entry?.id === id);
  const url = String(block?.url ?? "").trim();
  return url || null;
};

/** 绝对地址 + 尾斜杠（拼路径时永远 `base + "v1/…"`，所以在源头就补齐）。 */
export const hostBase = (ctx) => {
  const url = blockUrl(blocksOf(ctx), HOST_BLOCK_ID);
  if (!url) throw new ApiError(`没找到会话引擎（${HOST_BLOCK_ID}）—— 它没起来，还是被删了？`, { code: "no-host" });
  return url.endsWith("/") ? url : `${url}/`;
};

/**
 * 造一个 api。`self` = 这一页自己在宿主里的路径（`location.pathname`），只用来问 `__nous/ctx`。
 */
export function createApi({ fetch: doFetch = globalThis.fetch, self = "/" } = {}) {
  if (typeof doFetch !== "function") throw new TypeError("createApi 需要一个 fetch 函数");

  let base = null;
  let ctx = null;

  const ctxUrl = () => `${self.endsWith("/") ? self : `${self}/`}__nous/ctx`;

  // 段名在这里补齐，调用方只写 `v1/presets/…`：段名错了是全部端点一起 404，
  // 不会只坏一个角落 —— 集中一处才好查。
  const sectioned = (path) => {
    const clean = String(path ?? "").replace(/^\/+/, "");
    if (!clean.startsWith("v1/presets")) return clean;
    const rest = clean.slice("v1/presets".length);
    // 三种尾巴：空（段的总览）/ `/file` / `?name=…`（查询串）。
    const tail = !rest ? "" : rest.startsWith("/") || rest.startsWith("?") ? rest : `/${rest}`;
    return `v1/presets/${SECTION}${tail}`;
  };

  const get = async (path) => {
    if (!base) throw new ApiError("还没连上 qrd3wh", { code: "unconnected" });
    return readJson(await doFetch(base + sectioned(path), { headers: { accept: "application/json" } }));
  };

  return {
    get base() {
      return base;
    },
    get ctx() {
      return ctx;
    },
    /** 问宿主「qrd3wh 在哪」。 */
    async connect() {
      ctx = await readJson(await doFetch(ctxUrl()));
      base = hostBase(ctx);
      return base;
    },
    get,
    /**
     * 这一段的总览。503 = 这一段还没装好（形状未定），调用方拿它切示例数据 ——
     * **但要明着说**，不能悄悄拿假数据糊过去。
     */
    async overview() {
      return get("v1/presets");
    },
  };
}
