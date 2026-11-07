// api.js 的测试：段名怎么补、ctx 怎么找、错误怎么翻译。不发真请求。

import assert from "node:assert/strict";
import { test } from "node:test";

import { ApiError, SECTION, SectionNotReady, blockUrl, blocksOf, createApi, hostBase, readJson } from "../public/api.js";

const CTX = {
  blocks: [
    { id: "qrd3wh", url: "/p/new1/qrd3wh" },
    { id: "3699b9", url: "/p/new1/3699b9" },
  ],
};

/** 记录所有请求的假 fetch：`routes` 用 `"GET v1/presets"` 这种键。 */
const fakeFetch = (routes) => {
  const calls = [];
  const fn = async (url, init = {}) => {
    const method = init.method ?? "GET";
    const key = `${method} ${String(url).replace(/^.*?\/__nous\//, "__nous/").replace(/^.*?\/qrd3wh\//, "")}`;
    calls.push({ method, url: String(url), key, body: init.body ? JSON.parse(init.body) : null });
    const hit = routes[key];
    if (!hit) throw new Error(`no route ${key}`);
    const [status, body] = hit;
    return {
      ok: status >= 200 && status < 300,
      status,
      text: async () => JSON.stringify(body),
    };
  };
  fn.calls = calls;
  return fn;
};

test("段名：请求自动落到 Ultimate-Template（调用方只写 v1/presets）", async () => {
  const fetch = fakeFetch({
    "GET __nous/ctx": [200, CTX],
    "GET v1/presets/Ultimate-Template": [200, { presets: [] }],
  });
  const api = createApi({ fetch, self: "/p/new1/jtjzw5/" });
  await api.connect();
  await api.overview();
  assert.equal(fetch.calls[1].url, "/p/new1/qrd3wh/v1/presets/Ultimate-Template");
  assert.equal(SECTION, "Ultimate-Template");
});

test("ctx：块数组 / 对象两种形状都认；找不到 qrd3wh 时说清是谁没起来", () => {
  assert.equal(blockUrl(blocksOf(CTX), "qrd3wh"), "/p/new1/qrd3wh");
  assert.equal(blockUrl(blocksOf({ blocks: { qrd3wh: { url: "/x" } } }), "qrd3wh"), "/x");
  assert.throws(() => hostBase({ blocks: [] }), (error) => {
    assert.ok(error instanceof ApiError);
    assert.equal(error.code, "no-host");
    assert.match(error.message, /没找到会话引擎/);
    return true;
  });
  assert.equal(hostBase(CTX), "/p/new1/qrd3wh/");
});

test("503 = 这一段还没装好：折成 SectionNotReady，页面据此切示例数据", async () => {
  const fetch = fakeFetch({
    "GET __nous/ctx": [200, CTX],
    "GET v1/presets/Ultimate-Template": [503, { error: "这一段还没装好：Ultimate-Template" }],
  });
  const api = createApi({ fetch, self: "/p/new1/jtjzw5/" });
  await api.connect();
  await assert.rejects(api.overview(), (error) => {
    assert.ok(error instanceof SectionNotReady);
    assert.equal(error.status, 503);
    assert.match(error.message, /还没装好/);
    return true;
  });
});

test("readJson：服务端那一行原样当消息，不套前缀、不加工", async () => {
  await assert.rejects(
    readJson({ ok: false, status: 400, text: async () => JSON.stringify({ error: "名字不合格：Bad Name" }) }),
    (error) => {
      assert.equal(error.message, "名字不合格：Bad Name");
      assert.equal(error.status, 400);
      return true;
    },
  );
});

test("没连上就发请求 → 说清「还没连上」，不是发到一个拼错的地址", async () => {
  const api = createApi({ fetch: fakeFetch({}), self: "/" });
  await assert.rejects(api.overview(), (error) => {
    assert.equal(error.code, "unconnected");
    return true;
  });
});
