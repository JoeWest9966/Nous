// 取密钥必须宣告「我是面板」（X-NW-Reveal: 1）。
//
// 这条断过一次：上游把 apiKey 改成只回 apiKeyRef + ls 永不回显，secret 端点没有这个头
// 就回 PANEL_REVEAL_REQUIRED。症状极其阴 —— 编目照常（243 个模型全在），每条的
// hasKey 都是 false，点下去才 401。所以这里锁死的不是「请求带了个头」，
// 而是「装出来的 provider 真有一把钥匙」。

import test from "node:test";
import assert from "node:assert/strict";

import { installCatalog } from "../catalog.js";

/** 假编目：一个供应商、一条模型、一个要求 X-NW-Reveal 的 secret 端点。 */
const fakeFetch = (calls) => async (url, init = {}) => {
  const headers = Object.fromEntries(Object.entries(init.headers ?? {}).map(([k, v]) => [k.toLowerCase(), v]));
  calls.push({ url, headers });
  if (url.includes("/api/model?models=full")) {
    const body = {
      ok: true,
      data: {
        models: [
          {
            name: "fake-provider",
            baseUrl: "http://127.0.0.1:1/v1",
            api: "openai-completions",
            modelsDetail: [{ id: "fake-model", contextWindow: 8000, maxTokens: 1000 }],
          },
        ],
      },
    };
    return { json: async () => body };
  }
  if (url.includes("/secret")) {
    // 跟上游一样：缺头就明确拒绝，别回一个空 key 让人以为是「有供应商但没配」。
    if (headers["x-nw-reveal"] !== "1") {
      return {
        json: async () => ({
          ok: false,
          errors: [{ code: "PANEL_REVEAL_REQUIRED", message: "取秘密必须带意头头:缺 X-NW-Reveal: 1" }],
        }),
      };
    }
    return { json: async () => ({ ok: true, data: { key: "sk-fake" } }) };
  }
  throw new Error(`没预料的请求：${url}`);
};

const fakeModels = () => ({
  setProvider(provider) {
    this.provider = provider;
  },
});

test("取密钥带 X-NW-Reveal: 1，装出来的 provider 真能 resolve 出密钥", async () => {
  const calls = [];
  const models = fakeModels();
  const out = await installCatalog(models, { fetch: fakeFetch(calls) });

  assert.equal(out.error, null);
  assert.deepEqual(out.skipped, [], `不该跳过任何供应商：${JSON.stringify(out.skipped)}`);
  assert.equal(out.installed[0].hasKey, true);

  // 真的能取到钥匙 —— 这才是「装上了」的定义，hasKey 只是个摘要。
  const resolved = await models.provider.auth.apiKey.resolve();
  assert.equal(resolved.auth.apiKey, "sk-fake");
});

test("列编目的请求不带 reveal 头，只有 secret 带", async () => {
  const calls = [];
  await installCatalog(fakeModels(), { fetch: fakeFetch(calls) });

  const list = calls.find((c) => c.url.includes("/api/model?models=full"));
  const secret = calls.find((c) => c.url.includes("/secret"));
  assert.equal(list.headers["x-nw-reveal"], undefined);
  assert.equal(secret.headers["x-nw-reveal"], "1");
});

test("secret 端点拒绝时，原因原话进 skipped，不静默变 hasKey:false", async () => {
  const calls = [];
  // 一个不看头的服务端：模拟上游换成别的要求。
  const stubborn = async (url, init = {}) =>
    url.includes("/secret")
      ? { json: async () => ({ ok: false, errors: [{ message: "换了个说法" }] }) }
      : fakeFetch(calls)(url, init);

  const out = await installCatalog(fakeModels(), { fetch: stubborn });
  assert.equal(out.installed[0].hasKey, false);
  assert.match(out.skipped[0].reason, /换了个说法/);
});
