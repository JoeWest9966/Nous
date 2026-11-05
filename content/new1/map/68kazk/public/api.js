// 68kazk 的数据层：qrd3wh 的 `v1/skills*`。
//
// 这文件里没有 DOM、没有 localStorage、没有副本 —— 所以能裸跑 `node --test`，
// 也所以「仓库是唯一真相」（spec I2）在这一层是结构上成立的：页面除了内存里的响应，
// 没有第二份能拿来冒充清单的东西。
//
// 三条硬规矩：
//   · 地址只从宿主的 `__nous/ctx` 里找 qrd3wh，别处概不拼（I5）。
//   · 写操作只有 qrd3wh 一个入口，这一层不跑 git、不读盘（I4）。
//   · 服务端说什么就是什么：4xx/5xx 的 `{error}` 一行原样往上抛，前端不重写、不加前缀（I8）。

/** 会话引擎在宿主里的 id —— 不是名字：名字能改，id 建好就不动。 */
export const HOST_BLOCK_ID = "qrd3wh";

/** 类别改动攒多久再发。手点一下 chip 不该立刻写盘（spec 票 7）。 */
export const CATEGORY_DEBOUNCE_MS = 8000;

/** 409 那一句：给编辑抽屉用，测试也认这一句。 */
export const CONFLICT_MESSAGE = "文件已被改，重新加载";

export class ApiError extends Error {
  constructor(message, { status = 0, code = "api" } = {}) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/**
 * 409 单独一个类：编辑抽屉要能分辨「别人先改了，重读一下就行」和「真的坏了」。
 * 靠 `status === 409` 散落各处判断迟早漏一处，所以在这儿定型。
 */
export class ConflictError extends ApiError {
  constructor(message) {
    super(message, { status: 409, code: "conflict" });
    this.name = "ConflictError";
    this.conflict = true;
  }
}

/** 响应 → JSON。失败时把服务端那一行 `{error}` **原样**当消息（不套 `Error: `，不加前缀）。 */
export async function readJson(res) {
  const body = await res.json().catch(() => null);
  if (res.ok) return body ?? {};
  const line = typeof body?.error === "string" && body.error.trim() ? body.error : `HTTP ${res.status}`;
  if (res.status === 409) throw new ConflictError(line);
  throw new ApiError(line, { status: res.status });
}

/** `ctx.blocks` 可能是数组（现在）也可能是以 id 为键的对象（文档里的写法），两种都认。 */
export function blocksOf(ctx) {
  const blocks = ctx?.blocks;
  if (Array.isArray(blocks)) return blocks;
  if (blocks && typeof blocks === "object") {
    return Object.entries(blocks).map(([id, block]) => ({ id, ...(block ?? {}) }));
  }
  return [];
}

/**
 * 在 ctx 里找 qrd3wh 的绝对地址。找不到要说清「是谁不在」——
 * 「加载失败」四个字没法让人去修任何东西。
 */
export function hostBase(ctx) {
  const host = blocksOf(ctx).find((block) => block?.id === HOST_BLOCK_ID);
  if (!host || typeof host.url !== "string" || !host.url) {
    throw new ApiError(
      `__nous/ctx 里没有 qrd3wh（宿主没给出它的地址）—— 技能的新建 / 类别 / 正文 / 发布全在它那儿，找不到就没得问`,
      { code: "no-host" },
    );
  }
  return host.url.endsWith("/") ? host.url : `${host.url}/`;
}

/**
 * 一页一个 api。`fetch` 可注入 —— 测试拿假 fetch 测的就是这一层（票 §测试）。
 *
 * @param {{ fetch?: typeof globalThis.fetch, self?: string }} options
 *   self = 这一页自己在宿主里的路径（`location.pathname`），只用来问 `__nous/ctx`。
 */
export function createApi({ fetch: doFetch = globalThis.fetch, self = "/" } = {}) {
  if (typeof doFetch !== "function") throw new TypeError("createApi 需要一个 fetch 函数");

  let base = null;

  const ctxUrl = () => {
    const root = self.endsWith("/") ? self : `${self}/`;
    return `${root}__nous/ctx`;
  };

  const absolute = (path) => {
    if (!base) throw new ApiError("还没连上 qrd3wh", { code: "unconnected" });
    return base + String(path ?? "").replace(/^\/+/, "");
  };

  const request = (method) => async (path, body) => {
    const url = absolute(path);
    const init = method === "GET"
      ? { method, headers: { accept: "application/json" } }
      : {
          method,
          headers: { "content-type": "application/json", accept: "application/json" },
          body: JSON.stringify(body ?? {}),
        };
    return readJson(await doFetch(url, init));
  };

  return {
    /** qrd3wh 的绝对地址；连上之前是 null。 */
    get base() {
      return base;
    },
    /** 问宿主「qrd3wh 在哪」，然后把 `v1/skills*` 的请求都指向它。 */
    async connect() {
      const ctx = await readJson(await doFetch(ctxUrl()));
      base = hostBase(ctx);
      return base;
    },
    get: request("GET"),
    post: request("POST"),
    put: request("PUT"),
  };
}

/**
 * 类别 debounce：连续点选只发最后一次，8 秒后落盘。
 * 下一次输入重置计时；离开详情页 / 发布前必须 `flush()` —— 攒着没发等于用户以为存了。
 *
 * @param {{ post: (path: string, body: unknown) => Promise<unknown>,
 *           delay?: number, timers?: { setTimeout: Function, clearTimeout: Function },
 *           onSent?: (info: object) => void, onError?: (error: Error, info: object) => void }} options
 */
export function createCategoryDebounce({
  post,
  delay = CATEGORY_DEBOUNCE_MS,
  timers = { setTimeout: (fn, ms) => setTimeout(fn, ms), clearTimeout: (id) => clearTimeout(id) },
  onSent,
  onError,
} = {}) {
  if (typeof post !== "function") throw new TypeError("createCategoryDebounce 需要一个 post 函数");

  const pending = new Map(); // name → 最新一次的 category[]
  let timer = null;
  let chain = Promise.resolve();

  const fire = () => {
    timer = null;
    const batch = [...pending];
    pending.clear();
    if (!batch.length) return chain;
    chain = chain.then(() =>
      Promise.all(
        batch.map(async ([name, category]) => {
          const info = { name, category };
          try {
            const result = await post("v1/skills/category", { name, category });
            onSent?.({ ...info, result });
            return result;
          } catch (error) {
            onError?.(error, info);
            return null;
          }
        }),
      ),
    );
    return chain;
  };

  return {
    /** 记下这一次选择；计时重来。返回还有几条没发。 */
    schedule(name, category) {
      const wanted = Array.isArray(category) ? [...category] : [];
      pending.set(name, wanted);
      if (timer !== null) timers.clearTimeout(timer);
      timer = timers.setTimeout(fire, delay);
      return pending.size;
    },
    /** 立刻把攒着的发掉（离开详情、发布前、关页面）。没攒着就什么也不发。 */
    flush() {
      if (timer !== null) {
        timers.clearTimeout(timer);
        timer = null;
      }
      return fire();
    },
    /** 有没有还没落盘的（UI 拿它显示「正在保存」）。 */
    hasPending: () => pending.size > 0,
    pendingFor: (name) => pending.get(name),
  };
}

/**
 * 发布 = 内容闸 + 重扫 + commit + push。成功后**必须重读这个文件**：
 * 基线跟着 HEAD 走了，不重读的话差异视图还在跟旧 HEAD 比（spec §6.3 第 4 条）。
 */
export async function publishWithRefresh(api, { name, path } = {}) {
  const result = await api.post("v1/skills/publish", {});
  if (!path) return { result, file: null };
  const file = await api.get(
    `v1/skills/file?name=${encodeURIComponent(name)}&path=${encodeURIComponent(path)}`,
  );
  return { result, file };
}
