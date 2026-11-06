// `api.js` 的数据层 —— 注入假 fetch，不碰真网络、不碰 DOM。
//
// 测的是契约：地址怎么找、错误怎么上抛、409 单列、启动那条路发的是哪几个请求。
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ApiError,
  blocksOf,
  ConflictError,
  createApi,
  createValidateDebounce,
  hostBase,
  launchPreset,
  publishWithRefresh,
  readJson,
  siblingBase,
} from "../public/api.js";

/** 假 fetch：记下每次请求，按 (method, path) 回响应。 */
const fakeFetch = (routes) => {
  const calls = [];
  const fn = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const path = String(url);
    calls.push({ method, url: path, body: init.body ? JSON.parse(init.body) : null });
    const hit = Object.entries(routes).find(([key]) => {
      const [rMethod, rPart] = key.split(" ");
      return rMethod === method && path.includes(rPart);
    });
    const [status, body] = hit ? hit[1] : [404, { error: `no route ${method} ${path}` }];
    return {
      ok: status >= 200 && status < 300,
      status,
      json: async () => body,
    };
  };
  fn.calls = calls;
  return fn;
};

const CTX = {
  blocks: [
    { id: "qrd3wh", name: "session", url: "/p/new1/qrd3wh" },
    { id: "6ysg01", name: "Session-Template", url: "/p/new1/6ysg01" },
  ],
};

test("connect：ctx 里找 qrd3wh，找不到时说清是谁不在", async () => {
  const fetch = fakeFetch({ "GET __nous/ctx": [200, CTX] });
  const api = createApi({ fetch, self: "/p/new1/3699b9/" });
  await api.connect();
  assert.equal(api.base, "/p/new1/qrd3wh/");
  assert.equal(fetch.calls[0].url, "/p/new1/3699b9/__nous/ctx");
});

test("hostBase：ctx 里没有 qrd3wh → 一行话点名（不是「加载失败」四个字）", () => {
  assert.throws(() => hostBase({ blocks: [] }), (error) => {
    assert.ok(error instanceof ApiError);
    assert.match(error.message, /qrd3wh/);
    assert.equal(error.code, "no-host");
    return true;
  });
});

test("siblingBase：兄弟块不在就给 null（只是少一条链接，不是错误）", () => {
  assert.equal(siblingBase(CTX, "6ysg01"), "/p/new1/6ysg01/");
  assert.equal(siblingBase(CTX, "nope"), null);
});

test("blocksOf：数组和 {id: block} 两种形状都认", () => {
  assert.equal(blocksOf(CTX).length, 2);
  assert.equal(blocksOf({ blocks: { a: { url: "/a" } } })[0].id, "a");
});

test("readJson：失败时把服务端那一行原样当消息（不套前缀）", async () => {
  await assert.rejects(
    readJson({ ok: false, status: 400, json: async () => ({ error: "名字不合格：Bad Name" }) }),
    (error) => {
      assert.equal(error.message, "名字不合格：Bad Name");
      assert.equal(error.status, 400);
      return true;
    },
  );
});

test("readJson：409 单独一个类（面板要能分辨「重读一下」和「真的坏了」）", async () => {
  await assert.rejects(
    readJson({ ok: false, status: 409, json: async () => ({ error: "文件已被改，重新加载" }) }),
    (error) => {
      assert.ok(error instanceof ConflictError);
      assert.equal(error.conflict, true);
      return true;
    },
  );
});

test("get / post：地址拼在 qrd3wh 后面，POST 带 JSON body", async () => {
  const fetch = fakeFetch({
    "GET __nous/ctx": [200, CTX],
    "GET v1/presets": [200, { presets: [] }],
    "POST v1/presets/create": [200, { preset: { name: "a" } }],
  });
  const api = createApi({ fetch, self: "/p/new1/3699b9/" });
  await api.connect();
  await api.get("v1/presets");
  await api.post("v1/presets/create", { name: "a" });
  const calls = fetch.calls.filter((call) => call.url.includes("v1/"));
  assert.equal(calls[0].url, "/p/new1/qrd3wh/v1/presets");
  assert.equal(calls[1].url, "/p/new1/qrd3wh/v1/presets/create");
  assert.deepEqual(calls[1].body, { name: "a" });
});

test("没连上就发请求 → 说清「还没连上」，不是发到一个拼错的地址", async () => {
  const api = createApi({ fetch: fakeFetch({}), self: "/" });
  await assert.rejects(api.get("v1/presets"), (error) => {
    assert.equal(error.code, "unconnected");
    return true;
  });
});

test("launchPreset：先建会话 → 再切 selection → 最后才开新标签页", async () => {
  const fetch = fakeFetch({
    "GET __nous/ctx": [200, CTX],
    "POST v1/sessions": [201, { id: 7, name: "weekly 11-06", template: "smart-ass", missing: [] }],
    "POST v1/selection": [200, { id: 7 }],
  });
  const api = createApi({ fetch, self: "/p/new1/3699b9/" });
  await api.connect();
  const opened = [];
  const created = await launchPreset(api, {
    name: "weekly",
    sessionName: "weekly 11-06",
    open: (url) => opened.push(url),
  });

  const order = fetch.calls.filter((call) => call.method === "POST").map((call) => call.url.split("qrd3wh/")[1]);
  assert.deepEqual(order, ["v1/sessions", "v1/selection"]);
  // `preset` 一定要发 —— 它是这条会话的出处。
  const body = fetch.calls.find((call) => call.url.endsWith("v1/sessions")).body;
  assert.equal(body.preset, "weekly");
  assert.equal(body.name, "weekly 11-06");
  // 认服务端回来的生效值。
  assert.equal(created.id, 7);
  assert.deepEqual(opened, ["/p/new1/qrd3wh/"]);
});

test("launchPreset：建会话失败就**不**开标签页（开了个空对话比报错更糟）", async () => {
  const fetch = fakeFetch({
    "GET __nous/ctx": [200, CTX],
    "POST v1/sessions": [400, { error: "没有这条预设：gone" }],
  });
  const api = createApi({ fetch, self: "/p/new1/3699b9/" });
  await api.connect();
  const opened = [];
  await assert.rejects(launchPreset(api, { name: "gone", open: (url) => opened.push(url) }));
  assert.deepEqual(opened, []);
});

test("publishWithRefresh：发布成功后重读那个文件（基线跟着 HEAD 走了）", async () => {
  const fetch = fakeFetch({
    "GET __nous/ctx": [200, CTX],
    "POST v1/presets/publish": [200, { ok: true, pushed: null }],
    "GET v1/presets/file": [200, { text: "{}", etag: "abc" }],
  });
  const api = createApi({ fetch, self: "/p/new1/3699b9/" });
  await api.connect();
  const { result, file } = await publishWithRefresh(api, { name: "a", path: "preset.json" });
  assert.equal(result.ok, true);
  assert.equal(file.etag, "abc");
});

test("校验 debounce：只把最后一次的结果交出去（乱序返回不盖掉新的）", async () => {
  const timers = {
    setTimeout: (fn) => {
      const id = setTimeout(fn, 0);
      return id;
    },
    clearTimeout: (id) => clearTimeout(id),
  };
  const answers = [];
  const debounce = createValidateDebounce({
    delay: 0,
    timers,
    validate: async (payload) => {
      answers.push(payload.spec.summary);
      return { ok: true, summary: payload.spec.summary };
    },
  });
  const seen = [];
  debounce.schedule({ spec: { summary: "a" } }, { onResult: (result) => seen.push(result.summary) });
  debounce.schedule({ spec: { summary: "ab" } }, { onResult: (result) => seen.push(result.summary) });
  debounce.schedule({ spec: { summary: "abc" } }, { onResult: (result) => seen.push(result.summary) });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(answers, ["abc"]); // 前两次被取消了
  assert.deepEqual(seen, ["abc"]);
});
