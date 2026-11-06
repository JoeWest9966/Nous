// api.js —— 3699b9 的数据层：只跟 qrd3wh 的 `/v1/presets*` 说话。
//
// 这个文件里没有 DOM、没有 localStorage、没有副本 —— 所以能裸跑 `node --test`，
// 也所以「仓库是唯一真相」在这一层是结构上成立的：页面除了内存里的响应，
// 没有第二份能拿来冒充清单的东西。
//
// 四条硬规矩：
//   · 地址只从宿主的 `__nous/ctx` 里找 qrd3wh，别处概不拼。
//   · 写操作只有 qrd3wh 一个入口，这一层不跑 git、不读盘。
//   · 服务端说什么就是什么：4xx/5xx 的 `{error}` 一行原样往上抛，前端不重写、不加前缀。
//   · **校验规则一条都不在这里。** 「名字合不合法」「引用还在不在」全问服务端
//     （`/v1/presets/validate` / `resolve`）；本地多一套就是第二份真相。

/** 会话引擎在宿主里的 id —— 不是名字：名字能改，id 建好就不动。 */
export const HOST_BLOCK_ID = "qrd3wh";

/** 校验 / 解析 debounce：打字时不要一次一颗请求。 */
export const VALIDATE_DEBOUNCE_MS = 600;

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

/** 响应 → JSON。失败时把服务端那一行 `{error}` **原样**当消息（不套前缀）。 */
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
 * 在 ctx 里找某一块的绝对地址。找不到要说清「是谁不在」——
 * 「加载失败」四个字没法让人去修任何东西。
 */
export function blockUrl(ctx, id, hint) {
  const block = blocksOf(ctx).find((item) => item?.id === id);
  if (!block || typeof block.url !== "string" || !block.url) {
    throw new ApiError(`__nous/ctx 里没有 ${id}${hint ? `（${hint}）` : ""}`, { code: "no-block" });
  }
  return block.url.endsWith("/") ? block.url : `${block.url}/`;
}

/** 会话引擎的地址。找不到时那句话说清「预设的新建 / 编辑 / 发布 / 启动全在它那儿」。 */
export function hostBase(ctx) {
  try {
    return blockUrl(ctx, HOST_BLOCK_ID);
  } catch {
    throw new ApiError(
      "__nous/ctx 里没有 qrd3wh（宿主没给出会话引擎的地址）—— 预设的新建 / 编辑 / 发布 / 启动全在它那儿，找不到就没得问",
      { code: "no-host" },
    );
  }
}

/** 兄弟块的地址（详情里那些「去那边修」的链接）。找不到给 null —— 只是少一条链接，不是错误。 */
export function siblingBase(ctx, id) {
  try {
    return blockUrl(ctx, id);
  } catch {
    return null;
  }
}

/**
 * 一页一个 api。`fetch` 可注入 —— 测试拿假 fetch 测的就是这一层。
 *
 * @param {{ fetch?: typeof globalThis.fetch, self?: string }} options
 *   self = 这一页自己在宿主里的路径（`location.pathname`），只用来问 `__nous/ctx`。
 */
export function createApi({ fetch: doFetch = globalThis.fetch, self = "/" } = {}) {
  if (typeof doFetch !== "function") throw new TypeError("createApi 需要一个 fetch 函数");

  let base = null;
  let ctx = null;

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
    const init =
      method === "GET"
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
    /** 宿主的 ctx 原文（兄弟块地址从它来）。连上之前是 null。 */
    get ctx() {
      return ctx;
    },
    /** 问宿主「qrd3wh 在哪」，然后把 `v1/presets*` 的请求都指向它。 */
    async connect() {
      ctx = await readJson(await doFetch(ctxUrl()));
      base = hostBase(ctx);
      return base;
    },
    get: request("GET"),
    post: request("POST"),
    put: request("PUT"),
  };
}

/**
 * 校验 / 解析 debounce：连续打字只发最后一次，服务端说啥就是啥。
 *
 * 跟技能 / 模板那两页的「类别 8 秒 debounce」不是一回事：那个是**写盘**（攒够了才保存），
 * 这个是**只读校验**（每次都要最新答案，只是别一个字一颗请求）。所以它不 flush 到磁盘，
 * 也没有「有没有没保存的东西」这种状态。
 *
 * @param {{ validate: (payload: object) => Promise<object>, delay?: number,
 *           timers?: { setTimeout: Function, clearTimeout: Function } }} options
 */
export function createValidateDebounce({ validate, delay = VALIDATE_DEBOUNCE_MS, timers = globalThis } = {}) {
  if (typeof validate !== "function") throw new TypeError("createValidateDebounce 需要一个 validate 函数");

  let timer = null;
  let seq = 0;
  let latest = 0;

  return {
    /**
     * 排一次校验。只有**最后一次**的结果会被 `onResult` 收到（乱序返回不会盖掉新的）。
     * @returns {Promise<object|null>}
     */
    schedule(payload, { onResult, onError } = {}) {
      const mine = ++seq;
      if (timer !== null) timers.clearTimeout(timer);
      return new Promise((resolve) => {
        timer = timers.setTimeout(async () => {
          timer = null;
          try {
            const result = await validate(payload);
            if (mine < latest) return resolve(null);
            latest = mine;
            onResult?.(result);
            resolve(result);
          } catch (error) {
            if (mine < latest) return resolve(null);
            latest = mine;
            onError?.(error);
            resolve(null);
          }
        }, delay);
      });
    },
    cancel() {
      if (timer !== null) timers.clearTimeout(timer);
      timer = null;
    },
    get pending() {
      return timer !== null;
    },
  };
}

/**
 * 发布 = 形状闸 + 重扫 + commit + push。成功后**必须重读当前文件**：
 * 基线跟着 HEAD 走了，不重读的话差异视图还在跟旧 HEAD 比。
 */
export async function publishWithRefresh(api, { name, path } = {}) {
  const result = await api.post("v1/presets/publish", {});
  if (!name || !path) return { result, file: null };
  const file = await api.get(`v1/presets/file?name=${encodeURIComponent(name)}&path=${encodeURIComponent(path)}`);
  return { result, file };
}

/**
 * 启动一条预设：建会话（**同一个 commit 落定**）→ 把盘上那只眼睛切过去 → 新标签页开会话。
 *
 * 为什么不用 `window.open` 之后再 POST：那样弹窗会被拦，而且「开了个空的新对话」比
 * 「报一句错」更糟。所以先建后开，失败就只报错、不开口子。
 *
 * 四条都不能省：
 *   1. `preset` 一定要发 —— 它是这条会话的出处；
 *   2. 认服务端回来的**生效值**（thinkingLevel / skills / template / missing[]），不认我们发的；
 *   3. `missing*` 要点名，不然人以为勾上了；
 *   4. `selection` 要在开会话之后 —— 那是「现在盯着哪条」，建完才有 id。
 */
export async function launchPreset(api, { name, sessionName, open = (url) => globalThis.open(url, "_blank", "noopener") } = {}) {
  const created = await api.post("v1/sessions", {
    preset: name,
    ...(sessionName ? { name: sessionName } : {}),
  });
  await api.post("v1/selection", { id: created.id });
  const url = api.base;
  open(url);
  return created;
}
