# 模型管家：看 provider、填密钥、刷模型

`templates/models/` —— 拷进一个块就是一个「模型配置台」。活样本在 `new1 / k5411g`：
**http://100.106.43.81:4790/p/new1/k5411g/**

管的是 pi 那份 **`~/.pi/agent/models.json`**（`PI_MODELS` 可以指到别处）：

```json
{
  "providers": {
    "volc-ark": {
      "baseUrl": "https://…/v1",
      "api": "openai-completions",
      "apiKey": "…",
      "compat": { "supportsDeveloperRole": false },
      "models": [{ "id": "…", "name": "…", "contextWindow": 262144, "maxTokens": 131072 }]
    }
  }
}
```

## 这页能干什么

| 动作 | 背后 |
|---|---|
| 看所有 provider（id / api / baseUrl / 模型数 / 密钥掩码） | `GET /v1/state` |
| **复制密钥** | 点「复制」→ `GET /v1/providers/:id/key` → 写剪贴板 |
| **填 / 换密钥** | 输入框 +「粘贴」→ `PUT /v1/providers/:id/key` |
| **更新模型** | 「刷新模型」→ 打上游 `<baseUrl>/models`，合并写回 |
| 试连通 | `POST /v1/providers/:id/ping`（只读，不写盘） |
| 新建 / 删掉整条 | `POST /v1/providers` · `DELETE /v1/providers/:id` |
| 点模型 id | 复制到剪贴板 |

## 三个决定

**一、密钥只以掩码出现在页面上。** `3Pzj…zLhv`。要明文得点「复制」—— 单独一个 `GET …/key`，
前端拿到就立刻写剪贴板，不驻留 DOM。

**二、写盘先备份再原子替换。** 写坏一份 `models.json` 等于把这台机器的模型入口焊死：

```js
await copyFile(FILE, `${FILE}.bak`)            // 旧的一份留着
await writeFile(tmp, …, { mode: 0o600 })       // 权限 600
await rename(tmp, FILE)                        // 原子替换
```

**三、别把 provider 原样塞回浏览器。** 第一版 `shape()` 里我带了个 `raw: provider` 图省事 ——
那里面就是明文 `apiKey`，掩码白做了。现在只回掩码和模型列表。

## 刷新模型的合并规则

上游 `/models` 只给 id，**旧条目原样留着**（它可能带着手写的 `contextWindow`、价格、`input` 类型）：

```
合并 = 上游的 id（有旧条目就用旧的） + 上游这次没报的旧条目
```

实测 `volc-ark`：上游 138 个，原有 23 个带 `contextWindow` 的条目**一个没丢**，新增 125 个最小记录，
总数 148。

## 实测（2026-10-28，本机）

| provider | 结果 |
|---|---|
| `volc-ark` | **通** · 991ms · 138 个模型 |
| `suanli` | **通** · 1319ms · 16 个模型 |
| `ocfree` | 401 `api_key_expired` —— 这个 key 过期了 |
| `tbtk` | 401 `Invalid token` |
| `stepfun` | 401 `Incorrect API key provided` |

前两个是能用的。后三个得换 key —— 这页就是干这个的。

## 还没做：让块真的用上这些 provider

现在这页只管文件。要让 agent 真跑在 `volc-ark` 上，得把这份 json 喂给 `createProvider`：

```js
import { createProvider } from "@earendil-works/pi-ai/models"
import * as openaiCompletions from "@earendil-works/pi-ai/api/openai-completions"

const provider = createProvider({
  id: "volc-ark",
  baseUrl: cfg.baseUrl,
  auth: { … apiKey … },         // ProviderAuth：还没接，下一步的事
  models: cfg.models,
  api: openaiCompletions,        // { stream, streamSimple } —— api 字段就是走到这里
})
models.setProvider(provider)
```

`api` 字段（`openai-completions` / `anthropic-messages` / `openai-responses`）对应
`pi-ai/api/<那个名字>` 导出的 `{ stream, streamSimple }`，一一对得上。这是下一刀。
