// app.js 的数据层：ctx 解析、错误信封、409 分支、类别 debounce、发布后重读。
//
// 不测 DOM（spec §9）。`createApi()` 收一个假 fetch，所以这里测的是「这一页跟 qrd3wh
// 说了什么」：请求的顺序、地址从哪来、错误长什么样。

import test from "node:test";
import assert from "node:assert/strict";

import {
  ApiError,
  CATEGORY_DEBOUNCE_MS,
  CONFLICT_MESSAGE,
  ConflictError,
  createApi,
  createCategoryDebounce,
  hostBase,
  publishWithRefresh,
  readJson,
} from "../public/api.js";

/** 假 fetch：按 [method 路径] 记调用顺序，用 handler 决定回什么。 */
function fakeFetch(handler = () => ({ status: 200, body: {} })) {
  const calls = [];
  const fetch = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const body = init.body ? JSON.parse(init.body) : null;
    calls.push({ method, url, body });
    const reply = await handler({ method, url, body });
    return {
      ok: reply.status >= 200 && reply.status < 300,
      status: reply.status,
      json: async () => {
        if (reply.json !== undefined) return reply.json;
        return reply.body ?? {};
      },
    };
  };
  fetch.calls = calls;
  return fetch;
}

const jsonResponse = (body, status = 200) => ({ status, body });

/** 宿主 `__nous/ctx` 的形状（server/ctx.ts 的 flatten）：blocks 是数组，qrd3wh 在 id 上。 */
const CTX = {
  board: "new1",
  host: "http://127.0.0.1:4790",
  base: "http://127.0.0.1:4790/p/new1/68kazk",
  blocks: [
    { id: "n45", name: "Nous", path: "Nous", kind: "group", url: "http://127.0.0.1:4790/p/new1/n45" },
    {
      id: "qrd3wh",
      name: "session",
      path: "Nous/Nous-Functions/session",
      kind: "group",
      url: "http://127.0.0.1:4790/p/new1/qrd3wh",
    },
    { id: "68kazk", name: "skills", path: "Nous/Nous-Functions/skills", kind: "group", url: "http://127.0.0.1:4790/p/new1/68kazk" },
  ],
};

/** qrd3wh 的地址 = ctx 里那一条，测试里也不自己拼 —— 拼错了就等于没测 ctx。 */
const QR = CTX.blocks.find((block) => block.id === "qrd3wh").url + "/";

const connected = async (handler) => {
  const fetch = fakeFetch((call) => {
    if (call.url.endsWith("__nous/ctx")) return jsonResponse(CTX);
    return handler(call);
  });
  const api = createApi({ fetch, self: "/p/new1/68kazk/" });
  await api.connect();
  return { api, fetch };
};

test("4xx/5xx 带 {error} → 抛出来的消息就是那一行，不套第二层前缀", async () => {
  for (const status of [400, 409, 500]) {
    const res = {
      ok: false,
      status,
      json: async () => ({ error: `code-review: description 是空的` }),
    };
    await assert.rejects(readJson(res), (error) => {
      assert.equal(error.message, "code-review: description 是空的");
      assert.ok(!error.message.startsWith("Error:"), "消息被套了两层前缀");
      assert.equal(error.status, status);
      return true;
    });
  }

  // 响应体不是 JSON 也别丢状态码。
  const broken = { ok: false, status: 502, json: async () => { throw new SyntaxError("bad json"); } };
  await assert.rejects(readJson(broken), (error) => {
    assert.equal(error.message, "HTTP 502");
    return true;
  });

  // 走完整的那条：POST 打到一个 400，页面拿到的就是服务端那一行。
  const { api } = await connected(() => jsonResponse({ error: "名字撞了：code-review" }, 400));
  await assert.rejects(api.post("v1/skills/create", { name: "code-review" }), (error) => {
    assert.equal(error.message, "名字撞了：code-review");
    assert.equal(error.status, 400);
    assert.ok(!error.conflict, "400 不该落进冲突分支");
    return true;
  });
});

test("409 走 base 冲突分支：ConflictError.conflict，不是通用错误", async () => {
  const { api } = await connected(() => jsonResponse({ error: "文件已被改" }, 409));

  await assert.rejects(
    api.put("v1/skills/file", { name: "code-review", path: "SKILL.md", text: "x", base: "old" }),
    (error) => {
      assert.ok(error instanceof ConflictError);
      assert.ok(error instanceof ApiError);
      assert.equal(error.conflict, true);
      assert.equal(error.status, 409);
      // 服务端那一行还留着，页面负责把它换成「文件已被改，重新加载」+ 重读按钮。
      assert.equal(error.message, "文件已被改");
      return true;
    },
  );

  // 500 同一个端点：不是冲突，页面该走大红色错误分支。
  const { api: bad } = await connected(() => jsonResponse({ error: "写盘失败" }, 500));
  await assert.rejects(bad.put("v1/skills/file", { name: "x", path: "SKILL.md", text: "", base: "" }), (error) => {
    assert.ok(!(error instanceof ConflictError));
    assert.equal(error.conflict, undefined);
    assert.equal(CONFLICT_MESSAGE.includes("重新加载"), true);
    return true;
  });
});

test("类别 debounce：连续三次输入只发一次，flush 立刻把攒着的发掉", async () => {
  const posts = [];
  // 假计时器：记下延迟和回调，由测试决定什么时候到点。
  let nextId = 1;
  const timers = {
    hits: [],
    cleared: [],
    setTimeout(fn, ms) {
      const id = nextId++;
      this.hits.push({ id, fn, ms });
      return id;
    },
    clearTimeout(id) {
      this.cleared.push(id);
    },
  };
  const flush = createCategoryDebounce({
    post: async (path, body) => {
      posts.push({ path, body });
      return { skill: { name: body.name }, skills: [], categories: [] };
    },
    timers,
  });

  flush.schedule("code-review", ["Basics"]);
  flush.schedule("code-review", ["Basics", "工具"]);
  flush.schedule("code-review", ["工具"]);

  assert.equal(posts.length, 0, "debounce 到点之前一个请求都不该发");
  assert.equal(timers.hits.length, 3);
  assert.deepEqual(timers.cleared, [1, 2], "每次新输入重置上一次的计时");
  assert.equal(timers.hits.at(-1).ms, CATEGORY_DEBOUNCE_MS);
  assert.equal(CATEGORY_DEBOUNCE_MS, 8000);

  // 到点了：攒着的只发一条，内容是最后一次的选择。
  timers.hits.at(-1).fn();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(posts, [{ path: "v1/skills/category", body: { name: "code-review", category: ["工具"] } }]);

  // 没攒着时 flush 不制造空请求。
  await flush.flush();
  assert.equal(posts.length, 1);

  // 离开详情页：立刻 flush，不等计时器。
  flush.schedule("code-review", ["工具", "Basics"]);
  await flush.flush();
  assert.equal(posts.length, 2);
  assert.deepEqual(posts[1].body, { name: "code-review", category: ["工具", "Basics"] });
  // flush 之后计时器被扔掉，不会再补发一条。
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(posts.length, 2);
  assert.equal(flush.hasPending(), false);
});

test("ctx 里缺 qrd3wh → 错误消息里点名 qrd3wh", async () => {
  const noHost = { ...CTX, blocks: CTX.blocks.filter((block) => block.id !== "qrd3wh") };
  assert.throws(() => hostBase(noHost), (error) => {
    assert.match(error.message, /qrd3wh/);
    return true;
  });

  // 空 ctx / 没 blocks 也一样，别退化成「加载失败」。
  assert.throws(() => hostBase({}), /qrd3wh/);
  assert.throws(() => hostBase(null), /qrd3wh/);

  const fetch = fakeFetch(() => jsonResponse(noHost));
  const api = createApi({ fetch, self: "/p/new1/68kazk/" });
  await assert.rejects(api.connect(), (error) => {
    assert.match(error.message, /qrd3wh/);
    return true;
  });

  // 对象形态的 blocks 也认（文档里的 `ctx.blocks[id].url`）。
  assert.equal(hostBase({ blocks: { qrd3wh: { url: "http://x/p/new1/qrd3wh" } } }), "http://x/p/new1/qrd3wh/");
});

test("publish 成功后重读当前文件（假 fetch 记录调用顺序），base 换成新 etag", async () => {
  const { api, fetch } = await connected(({ method, url }) => {
    if (method === "POST" && url.endsWith("/v1/skills/publish")) {
      return jsonResponse({ ok: true, commit: { hash: "abc1234", subject: "skills: 改正文" }, pushed: true, warnings: [] });
    }
    if (method === "GET" && url.includes("/v1/skills/file?")) {
      return jsonResponse({ name: "code-review", path: "SKILL.md", text: "# 新正文\n", etag: "etag-2" });
    }
    throw new Error(`没预料到的请求：${method} ${url}`);
  });

  const { result, file } = await publishWithRefresh(api, { name: "code-review", path: "SKILL.md" });

  assert.equal(result.pushed, true);
  assert.equal(file.etag, "etag-2");
  // ctx 之后：先 publish，再重读文件 —— 反了的话基线还是旧的，diff 永远不归零。
  const order = fetch.calls.filter((call) => !call.url.endsWith("__nous/ctx")).map((call) => `${call.method} ${call.url}`);
  assert.deepEqual(order, [
    `POST ${QR}v1/skills/publish`,
    `GET ${QR}v1/skills/file?name=code-review&path=SKILL.md`,
  ]);
  assert.deepEqual(fetch.calls[1].body, {});
});
