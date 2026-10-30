# qrd3wh · session 引擎

一个磁盘一个 sqlite，里面 N 个 conversation。会话不是列表，是一棵树：`ConversationRecord.parent`
就是那根枝，不自己记。

## 模型从哪儿来：**Nous Warehouse 编目，不读本机文件**

```
https://j1900.tail8d0018.ts.net:8443/api/model?models=full   ← 编目（唯一真相）
        ↓ catalog.js 拉一次 + 字段映射 + 连密钥
pi-ai provider（内存）                                        ← 运行时只读它
```

上一版读 `~/.pi/agent/models.json`（`models-json.js`，**已删**）。改的原因不是文件不好用，
是**真相有两份**：那台机器上谁手写的 23 个模型，跟编目里登记过的 148 个，永远是两份会漂的事实。
现在编目是唯一真相，Nous 是它的投影。

三条硬规矩：

1. **密钥只在内存。** 启动时 `GET /api/model/{名}/secret` 拉一次，`auth.resolve()` 之后从 Map 取。
   不落盘、不进日志、不进 session 记录。代价说清：**编目里轮换了 key，本进程不重启就一直用旧的**
   —— 用 `POST /v1/catalog/refresh` 抵消（重拉清单 + 密钥，不用真重启）。
2. **编目说不出来的字段靠注解，不靠猜。** 上游 `/models` 不报「能不能思考」（volc-ark 一个字段都没有），
   所以 `reasoning` / `thinkingLevelMap` / `cost` / `compat` / `input` 是编目里**人写的注解**，
   `nw model register` 覆盖写会保住它们（见 Nous-Warehouse `preserveAnnotations`）。
   `catalog.js` 只做**格式映射**，绝不按 id 猜能力 —— 猜错一个，面板上就多一档点得下去、跑起来 400 的思考。
3. **编目挂了要说清。** `/v1/models` 的 `catalog` 块带 `error` / `at` / `skipped[]`：
   只回 `items` 的话，「编目挂了」在页面上长得跟「编目里没模型」一模一样，而这两种要干的事完全不同。

### 字段映射（别自己发明）

| pi-ai 要的 | 编目里可能长的样子 |
|---|---|
| `contextWindow` | `contextWindow` / `context_length` / `max_allowed_size` / `token_limits.context_window` |
| `maxTokens` | `maxTokens` / `max_output_tokens` / `token_limits.max_output_token_length` |
| `reasoning` | `reasoning`（注解）/ `supports_reasoning` / `only_reasoning` |
| `thinkingLevelMap` | `thinkingLevelMap`（注解）/ `reasoning_supported_efforts` / `supported_reasoning_levels[].effort` |
| `input` | `input`（注解）/ `modalities.input[]` / `modalities.input_modalities[]` / `supports_images` |

`xhigh` / `max` 只有写进 `thinkingLevelMap` 才算 —— pi-ai 的规矩，别给它补默认值。

**别名**：Ark 那类供应商的 `/models` 只列带日期的正式 id，人用的是 `glm-5.3` / `ark-code-latest`。
这些在编目里带 `local: true`，自扫不删。`catalog.js` 照单收，不需要特判。

## 思考等级

档位表**是模型的**，不是我们的：`levelsOf(model) = getSupportedThinkingLevels(model)`。
非 reasoning 只有 `["off"]`；reasoning 的上 `low..high` 默认都在。服务端收档位时用
`fitLevel()` 夹回模型真有的那一档再落盘（客户端发的值不可信）—— 面板显示 A、库里存 B 是要防的病。

## 接口

```
GET  /                            那棵树
GET  /v1/device                   我是谁、模型、编目状态
GET  /v1/models                   items[]（含档位表/窗口/输出上限）+ catalog{base,at,error,providers[],skipped[]}
POST /v1/catalog/refresh          重拉编目与密钥（换密钥 / 编目里加了供应商之后）
GET  /v1/sessions                 会话树
POST /v1/sessions                 {name, model, thinkingLevel}
POST /v1/sessions/:id/config      {readOnly, thinkingLevel}
POST /v1/sessions/:id/messages    {content, requestId?, waitMs?}
GET  /v1/sessions/:id/events      SSE
```

环境：`PORT`（默认 4780）、`NOUS_DATA`、`NW_BASE`（默认 tailnet 8443）、`NW_TOKEN`（走 panel-gate 才要）。

## 不做什么

- 不读 `~/.pi/agent/models.json` —— 那是 pi CLI 的配置，跟 Nous 无关。
- 不把密钥写进 `data/` —— 那目录会被 `cp -r` 带走，而块目录不该装秘密。
- 不按 id 猜 `reasoning` / 上下文窗口。宁可少一档思考，不要连着 400。
