// 把 pi 那份 models.json 变成 pi-ai 能用的 provider。
//
// models.json 的形状（pi 自己的约定）：
//   { providers: { <id>: { baseUrl, api, apiKey, compat?, models: [{id, name, contextWindow, …}] } } }
//
// 每个 provider 走 createProvider()，`api` 字段直接对应 pi-ai/api/<那个名字> 导出的
// { stream, streamSimple } —— 一一对得上，不用自己写传输层。

import { readFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { createProvider } from "@earendil-works/pi-ai/models";

export const DEFAULT_PI_MODELS = path.join(os.homedir(), ".pi", "agent", "models.json");

// api 字符串 → 实现模块。用到了才加载：一个模块拖着整个 sdk。
const LOADERS = {
  "openai-completions": () => import("@earendil-works/pi-ai/api/openai-completions"),
  "openai-responses": () => import("@earendil-works/pi-ai/api/openai-responses"),
  "anthropic-messages": () => import("@earendil-works/pi-ai/api/anthropic-messages"),
  "google-generative-ai": () => import("@earendil-works/pi-ai/api/google-generative-ai"),
};

const DEFAULT_COST = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

/** models.json 的一条模型 → pi-ai 的 chat model。缺字段就补保守默认值。 */
const toModel = (id, provider, model) => ({
  type: "chat",
  id: model.id,
  name: model.name ?? model.id,
  provider: id,
  api: provider.api,
  baseUrl: provider.baseUrl,
  reasoning: model.reasoning ?? false,
  input: Array.isArray(model.input) && model.input.length ? model.input : ["text"],
  cost: model.cost ?? DEFAULT_COST,
  contextWindow: model.contextWindow ?? 128_000,
  maxTokens: model.maxTokens ?? 8_192,
  // compat 是**模型级**的：provider 上写一份，模型上再覆盖。
  ...(provider.compat || model.compat ? { compat: { ...(provider.compat ?? {}), ...(model.compat ?? {}) } } : {}),
  ...(model.thinkingLevelMap ? { thinkingLevelMap: model.thinkingLevelMap } : {}),
  ...(model.inputLimits ? { inputLimits: model.inputLimits } : {}),
});

/** 密钥直接来自文件：不走环境变量，也没有登录流程。 */
const authFor = (id, readKey) => ({
  apiKey: {
    name: `${id} 的密钥（models.json）`,
    resolve: async () => {
      const key = await readKey();
      return key ? { auth: { apiKey: key }, source: "models.json" } : undefined;
    },
  },
});

/**
 * 读 models.json，把每个 provider 装进 `models`。
 * 返回装了哪些、跳过了哪些（缺密钥 / api 不认识 / 模块加载失败），好让调用方说清楚。
 */
export async function installProviders(models, { file = DEFAULT_PI_MODELS } = {}) {
  let config;
  try {
    config = JSON.parse(await readFile(file, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return { file, installed: [], skipped: [], missing: true };
    throw error;
  }

  const installed = [];
  const skipped = [];
  for (const [id, provider] of Object.entries(config.providers ?? {})) {
    const api = String(provider?.api ?? "");
    const loader = LOADERS[api];
    if (!loader) {
      skipped.push({ id, reason: `不认识的 api：${api || "（空）"}` });
      continue;
    }
    if (!provider.baseUrl) {
      skipped.push({ id, reason: "没有 baseUrl" });
      continue;
    }
    let streams;
    try {
      streams = await loader();
    } catch (error) {
      skipped.push({ id, reason: `加载 ${api} 实现失败：${error.message}` });
      continue;
    }

    const list = (Array.isArray(provider.models) ? provider.models : [])
      .map((model) => (typeof model === "string" ? { id: model } : model))
      .filter((model) => model && typeof model.id === "string" && model.id)
      .map((model) => toModel(id, provider, model));

    // 密钥可能在页面上被换掉：每次 resolve 重读文件，别把进程启动时的那份钉死。
    const readKey = async () => {
      try {
        const fresh = JSON.parse(await readFile(file, "utf8"));
        return fresh.providers?.[id]?.apiKey;
      } catch {
        return config.providers?.[id]?.apiKey;
      }
    };

    models.setProvider(
      createProvider({
        id,
        name: id,
        baseUrl: provider.baseUrl,
        auth: authFor(id, readKey),
        models: list,
        api: { stream: streams.stream, streamSimple: streams.streamSimple },
      }),
    );
    installed.push({ id, api, models: list.length, hasKey: Boolean(provider.apiKey) });
  }
  return { file, installed, skipped };
}
