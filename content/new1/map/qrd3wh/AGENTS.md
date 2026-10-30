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

## 技能：内容在盘上，选择在会话里

`skills.js`。跟 pi CLI 同一套规矩（同一份 `<available_skills>` XML、同样让模型用 `read`
去读正文、同样按 SKILL.md 所在目录展开相对路径），所以给 pi 写的技能直接能用。

**三层，只有第三层跟代码有关系：**

| 层 | 是什么 | 在哪 |
|---|---|---|
| 内容 | `SKILL.md` + `scripts/` + `references/` | `data/skills/<名>/` 真文件（模型 `read` 它，`bash` 也能跑它） |
| 名册 | `{name, description, 路径}` + 来源收据 | 内存 Map（启动/装卸时扫一次）+ `data/skills/<名>/.nw.json` |
| 钩子 | `defineExtension({ name: "skill:<名>" })` | **空名牌**，不含一个字的技能文本，只为在 `agent.extensions` 里占个名字 |

为什么不自己建一张「哪条会话会哪些技能」的表：`pi.agent.extensions` 就是那张表，而且
durable 白送 fork 继承、重启存活、热替换、**加一条只追加一行 `pi.system` 增量**（不重写历史，
provider 的提示词缓存还热着）。自己存一份 = 重造一个 fork 不继承、重启要迁移的轮子。

三条不许踩：

1. **`render()` 每轮请求都跑**，所以它只查内存 Map —— 不碰文件系统、不碰网络、不看时间、
   顺序按名字排。漏一样进去，就是每轮追加一条 `pi.system` 并打掉提示词缓存。
2. **空名牌装了但不许进默认选择**（`settings.extensions`）。进了 = 每条新会话一出生就吃
   38 条描述，progressive disclosure 当场作废。默认只放真扩展（`CodingTools` / `Nous` / `skills`）。
3. **装失败要说清到哪一步**（编目 / 版本号 / tarball / 仓库里没这条路径）。只回一个红条，
   人得自己猜。

按 sha 记收据（编目**没有版本层**，nw 自己说的：不含 ref/@tag —— 所以「上次跑的是哪一版」
只有我们记）。已经装过同一版就不重拉。

## 接口

```
GET  /                            那棵树
GET  /v1/device                   我是谁、模型、编目状态
GET  /v1/models                   items[]（含档位表/窗口/输出上限）+ catalog{base,at,error,providers[],skipped[]}
POST /v1/catalog/refresh          重拉编目与密钥（换密钥 / 编目里加了供应商之后）
GET  /v1/skills                   已装 + 编目里能装的（按 repo 去重后的 38 条）+ catalog{base,at,error}
POST /v1/skills/install           {source, entry}（或 {repo, path}）→ 拉到 data/skills/
POST /v1/skills/uninstall         {name}（会话还选着它的，静默失效，不报错）
POST /v1/skills/refresh           重拉技能编目
GET  /v1/sessions                 会话树
POST /v1/sessions                 {name, model, thinkingLevel, skills}
POST /v1/sessions/:id/config      {readOnly, thinkingLevel, skills}
POST /v1/sessions/:id/messages    {content, requestId?, waitMs?}
GET  /v1/sessions/:id/events      SSE
GET  /v1/sessions/:id/entries     ?system=1 把 pi.system 的 sections 原样放出来（调试提示词用）
```

环境：`PORT`（默认 4780）、`NOUS_DATA`、`NW_BASE`（默认 tailnet 8443）、`NW_TOKEN`（走 panel-gate 才要）。

## 不做什么

- 不读 `~/.pi/agent/models.json` —— 那是 pi CLI 的配置，跟 Nous 无关。
- 不把密钥写进 `data/` —— 那目录会被 `cp -r` 带走，而块目录不该装秘密。
- 不按 id 猜 `reasoning` / 上下文窗口。宁可少一档思考，不要连着 400。
- 不读本机任何 `skills.json` / `~/.pi/agent/skills` —— 技能编目也是 nw 一份，本机那两份
  （`/var/opt/Data-001/skills.json` 只有 1 条，8443 上有 38 条）是会漂的第二真相。
- 不把技能做成树节点。技能有上游（repo@sha），树只有四个字段、装不下来源；把别人的
  38 份 README 导进盘面，盘就不是你的了。
