# wire — 本库的对外契约（宪法）

**这一份是权威版本。** 改契约只改这里；本文件就住在 Nous 仓根。

- 描述的是本库对 Nous 客户端（桌面 / 手机）暴露的 `/v1/*`。
  宿主那层 `__nous/*`（`resolve` / `doc` / `ctx`）随旧面板 2026-11-18 下线，本库不提供。
- 端点分两层：**内核端点**由 `nous-durable` 提供，**领域端点**（归档 / 模板 / 技能 / 提示词 / 机器 / 目录）由 Nous 的服务层提供 —— 但契约只有这一份。
- 字段只写"我们依赖的"；其余一律 `serde_json::Value` 收着透传，不 `deny_unknown_fields`。

---

# 契约正文（自 NousDesk `wire.md` 迁入，2026-11-16）

引擎是 `qrd3wh`（Nous 的 session 块）。这份抄的是**此刻**的协议，
字段是从引擎 `server.js` 和真实消费方 `bjes6b` 里读出来的，不是文档里的。

**这一份会漂。** 引擎加字段是常态。我们只依赖下面「我们依赖」列出来的字段，
其余字段一律 `serde_json::Value` 收着透传，不碰、不 `deny_unknown_fields`。

---

## 通用

### 配置：四条私库路径（#63）

服务端按 TS 版 `server.js:77-83` 解析下面四个变量，变量名与默认值**只有这一份真相**
（代码落在 `crates/nous/env/src/content.rs`，别在各端点里自己 `std::env::var`）：

| 变量 | 默认值 | 指向 |
|---|---|---|
| `NOUS_SKILLS` | `/opt/Nous-skills` | 技能条目 |
| `NOUS_PROMPTS` | `/opt/Nous-prompts` | 提示词条目 |
| `NOUS_PRESETS` | `/opt/Nous-Template` | 模板条目（#60 的数据根；不得写死别的目录） |
| `NOUS_EXTEND` | `/opt/Nous-extend` | 自研扩展源码（`ext` 仍 `frozen`，只登记路径） |

**部署时 unit 里不写这四个变量**：默认值就够，写了就成了两份真相。旧的
移植期的旧变量名已删除，不做双名兼容。

- 会话 id 是 **数字**（不是字符串）。`GET /v1/sessions/:id/...` 里的 `:id` 是数字。
- 列表里 `busy` 是 `"queued" | "running" | <task state kind> | null`，不是布尔。
- 错误形状统一 `{ error: string }`，状态码跟着语义走（400 / 404 / 409 / 500…）。

## 我们用到的端点

### 调度节拍（#73）

生产进程在 app 装配处起一条常驻节拍：默认 60s 一拍，每拍写一条 durable tick 记录；
`scheduler.lastTickAt` 从库里的 tick 记录读，不从内存读。测试可用
`NOUS_DISPATCHER_INTERVAL_MS` 缩短间隔（默认 60000，毫秒），不改产品默认值。
进程退出时节拍任务随 Tokio runtime 收敛，不留下孤儿 tick。

## 幂等回执的提交边界（review #10）

`requestId` 的回执必须与它描述的变更在**同一提交 / 同一临界区**：
- 库上（trigger PATCH）：定义变更与 receipt 行同一 `BEGIN IMMEDIATE`；回执写失败 → 整批回滚，`version` 不涨。
- 盘上（combos PATCH）：回执先原子写就位，再改 `preset.json`；后者失败 → 删回执，盘面不变。

**选定语义：重放回第一次回执；若第一次整体失败，则盘/库未变，重放当作全新请求执行。**
不许出现「变更落了、回执没落 → 重放 412」或「回执落了、变更没落 → 重放假成功」。
反悔点见 `docs/decisions.md` 2026-11-24 条。

### PATCH /v1/triggers/{code}

#73：改触发器自己的 `schedule` / `params` / `enabled`。`code` 是不可变 slug（`nous-store`
的库级触发器挡着），请求体里出现 `code` 就 400。

- body 是逐键覆盖的稀疏对象：出现的键才改，未出现的键不动。`schedule` 是五段 cron 字符串
  或 `null`（取消定时），`params` 是字符串/数字/布尔组成的对象，`enabled` 是布尔。
  `schedule`/`params` 归 `spec` 管，`enabled` 是触发器行上的列，不是 `spec` 里的一个键。
- `version` 是触发器行上的 ETag（整数），改动一次加 1；请求体必带当前 `version`（字符串
  形式也认），不匹配 → 412。请求另带 `If-Match` 时，它也必须和 `version` 一致。
  回包是改后的生效定义 + 新 `version`。
- `requestId` 是可选幂等锚：重放回第一次回执，`version` 不涨、库不再改。回包里的
  `definition` 是从库里读回的生效值，不是把请求体原样吐回来。
- 不存在 / 已墓碑的 `code` → 404；坏 body（未知键、错类型、坏 cron、空对象、改 `code`）→ 400。

### 触发器管理面（#80：trigger-catalog）

`PATCH /v1/triggers/{code}` 见上（#73，本票不重做）。#80 补新建 / 整份改定义 / 归档 / 捞回 / 真删 / 类别。
实现仍在 `nous-server` 的 `triggers` 面，定义与账仍只在 `nous-store` 的 `triggers` / `fires` 两张表；
不新增 crates、不新增数据目录、不另开写通道。

| 方法 + 路径 | 语义 |
|---|---|
| `POST /v1/triggers` | 新建。201 回 `{code, name, category, spec, version, enabled, definition}`。 |
| `PUT /v1/triggers/{code}` | 整份改定义：稀疏覆盖 `schedule` / `on` / `params` / `session` / `enabled`。 |
| `POST /v1/triggers/{code}/archive` | 归档墓碑；账不动。回 `{code, archivedAt}`。 |
| `POST /v1/triggers/{code}/restore` | 抹掉归档墓碑；回 `{code, restoredAt}`。 |
| `POST /v1/triggers/{code}/purge` | 真删：先归档才能 purge。回 `{code, purgedAt, fires}`（`fires` 是删掉的账行数）。 |
| `GET /v1/triggers/categories` | 类别词表：`{categories: [...]}`（词表 ∪ 在用的，排序）。 |
| `POST /v1/triggers/category` | body `{code, category}`；`null` 清除。回 `{code, category}`。 |

- `code` 必须匹配 `^[a-z][a-z0-9_]{2,63}$`，且创建后不可变。**写死**：改端点（PUT）
  body 里出现 `code` 键一律 **400**（不是忽略）—— 静默忽略会让人以为改名成功。
  POST 用 body 的 `code` 建、路径不带 `code`；重名 / 墓碑占用的 `code` 回 409。
- `schedule` 是五段 cron（`分 时 日 月 周`，数字或 `*` / `*/n`）或 `null`。`on` 是 `null`
  或 `kind` / `kind:ref` 形式的 slug（`kind ∈ cron|clock|event|agent|human|ext`）。
  **发布闸**：两者在写入时校验，坏定义 → 400 且库里一行都不写（负向哨兵见票面判据 4）。
- `params` 是字符串 / 数字 / 布尔组成的对象；`session`（整数字节 id 或 `null`）是落点简写，
  `null` = 新会话；也接受显式 `target: {kind:"new"|"conversation", id?}`。两者同时出现 → 400。
- PUT 是**逐键覆盖**：出现的键才改，未出现的键不动；显式 `null` 能清掉 `schedule` / `on` /
  `session`。`version` 是 ETag：出现必须与当前一致（不一致 412），每次有效写入 +1。
  改定义 / 归档 / 捞回 / 真删都由 `version` 守着；归档与捞回不改 `version`（ETag 只描述定义）。
- `GET /v1/triggers` 的 `triggers[]` **不含**已归档 / 已真删的触发器，但 `fires` 账仍在；
  每条多一个 `category`（字符串或 `null`）。archive → restore → 原样回来。
- **purge 的账怎么处理（拍板）**：真删该 `code` 的**全部 `fires` 行**（账随触发器一起销毁，
  不留在孤儿表里），触发器行留墓碑并带 `purgedAt`，`code` 永不复用；对已真删的行再 purge → 409。
  未归档先 purge → 409（与会话归档同规矩）。这与会话「撕原件」不同：会话的 gen 复印件是
  独立文件，触发器的定义只有 `triggers` 一行，留行只为占住 `code`。
- 类别词表是字符串数组，**没有 id**。`POST category` 给一个触发器挂单值类别；类别不在词表里
  时自动登记（与 skills 的「词表 ∪ 在用」同规矩），`null` 只清该行的挂值、不删词表条目。

### GET /v1/sessions

会话**树**（不是列表）。返回 `{ items: [...] }`，**新的在前**。

我们依赖：`id` `name` `parent` `model` `thinkingLevel` `readOnly` `busy` `updatedAt` `last` `archived`。

字段形状以 `crates/nous/server/tests/endpoints.rs::get_sessions_returns_the_wire_shape_newest_first` 为准（本文件只索引端点与约束）。

- `archived` 是**归档墓碑**，`null` = 没归档过（捞回之后也回到 `null`），对象 = `{at, gen, purged}`，
  `at` 是 epoch **毫秒**。老引擎整个字段缺席时也是 `null` —— 别当布尔用，也别拿它判断
  「这一行要不要显示」（那个由引擎默认藏掉，见下面 `archive`）。

- `updatedAt` 是这条会话**最后一次落库**的时刻，epoch **毫秒**，可能是 `null`。
  `null` 不是 bug：引擎只记「自己看着它写的那些」，重启前的老会话就是空的 ——
  显示相对年龄（5m / 3h / 2d）就用它，**别用 `last.id` 假装时间**，也别拿文件 mtime 冒充
  （拷贝一份盘会让每条会话都变成「刚刚」）。

- `parent != null` = 这是一条 **fork 分支**。bjes6b 手机端把它全滤掉，只认主线。
  桌面端要不要显示分支是产品决定，别在客户端里猜 —— 真要做 fork 走 `POST /v1/sessions/:id/fork`。
- `last` 是最后一条 entry 的摘要（`{id, kind, text}`），用来画列表预览。它可能为 null。
- `name` 是会话名，**可能是空串 `""`** —— 含义是「这条会话从没被命名过」，不是错误。
  **本库不生成默认名**，没有「对话 44」这种东西：目录里没这个名字就是 `""`。
  要显示占位就客户端自己拼（「对话 12」），**别把占位写回库里** —— 写回等于把「没命名」
  变成「命名为『对话 12』」，改名历史从此脏掉。
  这是本库与旧引擎的**故意差别**（旧 `qrd3wh` 会给「对话 N」），依据见 `docs/fixture-map.md`：
  fixture 的 `names` 覆盖全部 189 条活会话，`""` 只出现在建了没命名那一段。
- `skills` / `template` / `thinkingLevels` / `extensions` 是**生效值**，不是请求值 —— 拿来画勾，
  别自己猜。引擎改这些字段，列表预览不受影响，只有详情面板受影响。

### GET /v1/stats/usage?from=&to=&group_by=day|model|conversation

用量与账本。`totals` 与 `rows[]` 的四元组完全相同：`totalTokens` / `costTotal` /
`requests` / `errors`；额外 `rows[].key`（桶名）与 `rows[].label`（可直接上屏）。
`group_by=day` 用 UTC 日期 `YYYY-MM-DD`（按 entry 的 `model[].timestamp` 归桶）；
`model` 用 `provider/modelId`（缺失记 `unknown`）；`conversation` 用会话 id 字符串。
- `from` / `to` 都是 **epoch 毫秒**，闭开区间 `[from, to)` —— `from` 命中、`to` 不命中。
  缺省 `from` = 全量起点、缺省 `to` = 全量终点；无时间戳的 entry 不进任何桶、也不进 totals。
- `requests` 计有时间戳的 `pi.assistant` entry，`errors` 计 `model[].stopReason == "error"`。
- 只聚合 durable 库里现存 entry；归档/fixture 副本不在内。只读，无分页。

### GET /v1/stats/tasks

任务计数：`{byState, active}`。`byState` 的键全集是 `pending` / `running` / `waiting` /
`sleeping` / `completing` / `terminal`，值从 `tasks` 表 `GROUP BY status` 现算（与 triggers 的
`counts` 同规矩，**不许内存计数器**）；`active = byState` 中除 `terminal` 外之和。

### GET /v1/stats/db

两个库各一份读数：`{ durable: {...}, plugins: {...} }`，每份含 `bytes`（主文件长度）、
`rows`（关键表计数对象）、`quickCheck`（`"ok"` 或诊断串）、`schema`（整数版本）。
durable 这一份与 `nous-app --check` 共用 `nous_durable::ops::check` 的读口；
plugins 这一份与 `nous-app --check` 之后那条插件库检查共用 `Store` 的只读读口。
`nous-plugins.db` 不存在时按空插件库读（schema/rows 为 0），不 404/500。

### GET /v1/sessions/:id/entries

历史。返回 `{ items: [{id, kind, text, parent}], next }`，**新的在前**，客户端要翻正。

- `kind` 有 `pi.user` / `pi.assistant` / `pi.system` / `pi.tool-result` 等。桌面端至少处理
  `pi.user`（右气泡）和 `pi.assistant`（左气泡），其余降级显示，别崩。
- `parent` 是 entry 的父指针，和会话树的 `parent` 不是一回事。
- `next` 是分页游标，`?limit=` 上限 1000。
- `?system=1` 是回放开关：额外回 `system`（回放全部 `pi.system` 后**当前生效**的提示词，
  可能为空串），并给每条 `pi.system` 的 `items[]` 补 `delta`（它自己带来的文本）。
  不传时仍过滤 `pi.system`，且**没有** `system` 键。

### POST /v1/sessions/:id/messages

消息 body 的字段形状暂无测试背书（现有测试只钉关键字段的语义，不钉完整 body）；
这里只留语义，不带形状，也不假装有权威测试指针。

- `requestId` 是幂等锚：重连重发同一句拿到同一次提交，不会答两遍。**桌面端一定要传**。
- 不传 `waitMs` 时立即回 `202 { submissionId, status: "queued" }`，真正的流走 SSE。
  桌面端就按这条来：发消息 → 拿 202 → 靠 SSE 收内容。别用 `waitMs` 等同步。
- #70：任何 202 之后都会起一轮（真 provider 与 faux 一样），submission 必须离开 `queued`
  并收敛到 `done` 或 `unanswered`。模型解析顺序写死：请求体 `model` → 会话当前的
  `model` → 编目 `default` → 都没有就 **503 `{error:"没有可用模型"}`**，不许静默受理。
- `waitMs`：`0`（或省略）回 202；`>0` 最多等这么久，轮次完成回 **200 且带答案 entry**，
  超时回 202（答案仍走 SSE）。真实等待绑在轮次完成信号上，不是 sleep 轮询。
- `whenBusy`：`queue`（默认，忙时排队）或 `interrupt`（先停当前轮次再受理）；
  非法值 400。忙时判定以服务端会话状态为准，客户端不自己猜。

### GET /v1/sessions/:id/events —— SSE

连上先推一帧 `snapshot`，然后是增量事件。**这是桌面的心跳，也是最容易写坏的一层。**

事件类型（`event.type`）：

| type | 我们依赖什么 | 备注 |
|---|---|---|
| `snapshot` | `generation?.message`；`entries[].model[].timestamp` | 重连第一帧。有正在生成的消息就续上，没有就只当「连上了」。**`entries` 是唯一的时间源**：`/v1/entries` 不给时间（引擎的 `activity.json` 只按会话记一个时刻），按天分组目录读这里 |
| `message_start` | `message.role === "assistant"` | user/system 也发，只有 assistant 该开口一个气泡 |
| `message_update` | `changes[]` | 见下 |
| `message_end` | `entry.kind === "pi.assistant"` 时收尾 | 助手气泡结束，把攒的字定稿 |
| `entry_appended` | `entry.kind` | 别的标签页 / 客户端加的东西；`pi.user` `pi.tool-result` 才补 |
| `submission` | `record.status === "unanswered"` 时弹错误 | `record.reason` / `record.detail` 是「没答上来」的原因 |
| `task_failed` | `message` `taskId` | 只在任务进入终态且 `outcome.status == "failed"` 时发；中途状态与成功的终态都不发 |
| `tool_execution_start` | `toolCallId` `toolName` | 开一个工具卡片 |
| `tool_execution_update` | `toolCallId` `output` | `output.append` / `output.set` 追加 / 覆盖 |

`message_update` 的 `changes[]` —— **全集来自 pi-durable 的 `MessageChange`**
（`node_modules/@earendil-works/pi-durable/dist/harness/events.d.ts`），不是猜的：

```
{ type: "text_start" | "thinking_start" | "toolcall_start", contentIndex, block }
{ type: "text_delta" | "thinking_delta", contentIndex, delta }
{ type: "toolcall_delta", contentIndex, path, delta }
{ type: "block", contentIndex, block }     → 整块替换，该块后续变化都在这一批里
{ type: "message", message }               → 整条替换，以它为准
```

**只在 `text_delta` 上动字符串是不够的**：块的开头走 `text_start`（`block.text` 已有全文），
跳过它就等于把第一段吞掉。`thinking_*` 是思考不是正文，别往气泡里塞。

其他事件的全集（同一个 d.ts 里的 `AgentEvent`）还有 `run_start` / `run_end` / `turn_start` /
`turn_end` / `inbox_update` / `agent_changed` / `usage_changed` / `auto_retry_start` /
`auto_retry_end` / `deferred_poll` / `tool_execution_end` / `compaction_start` / `compaction_end`。
我们不认识的走 `Other`（`#[serde(other)]`），加事件不该让我们编译失败。

**合帧铁律**：`text_delta` 只改内存里的 `Map<contentIndex, String>`，16ms 定时器读一次再
`cx.notify()`。一个 token 一次 notify 会在 `Entity<T>` 不匹配报错之前，先把 CPU 烧穿。

### POST /v1/sessions/:id/abort

返回 `{ status: "aborted" }`。当前这轮生成停下。

### GET /v1/device

`{ self, origin, node, startedAt, model, faux, data, catalog, available }`。

我们依赖：`model`（默认模型）+ `faux`（假 provider 标志，本地验流式时用）。About 面板显示引擎版本用 `node`。

#79：`catalog` 现在是 device 自己那份诊断对象（`state` / `source` / `path` / `fetchedAt` / `error`），
不是 `null`。`fetchedAt` 是最近一次成功刷新的时刻，`error` 是最近一次失败的原因；
两者都可能为 `null`（缺席）。字段形状以 `crates/nous/server/tests/catalog_refresh.rs` 为准。

### GET /v1/selection · POST /v1/selection

`{ id }` 或 `{ id: null }` —— 「现在盯着哪个对话」。桌面端三栏同屏时，左栏高亮当前会话就用它，
**别为左栏开 N 条 SSE**。SSE 只连当前会话，其余靠 `GET /v1/sessions` 低频轮询。

本库把这两条归到 `sessions` 模块（selection 是两行状态，不是独立能力面）。状态落 durable
会话库（`qrd3wh.sessions` 文档），重启后还在；指向已删会话时 `GET` 回 `null` 并清掉死指针。

### POST /v1/sessions/:id/config

逐键覆盖会话配置：`readOnly` / `model`（`provider/modelId`）/ `thinkingLevel` / `skills` /
`extensions` / `template`（单值或 `null`）/ `machine` / `cwd`。**未出现的键不动**；
`skills` / `extensions` 是相对增删，不是整份覆盖。换模型按新模型的 `thinkingLevels` 重新夹
档位（`off` 一定要在表里，没有就退成 `off`）。回包是生效值。

### GET /v1/unread

每条会话的未读数，**平铺的 map**，一个会话一个键（**没有 `items` 包一层**）：

```
{ "14": { unread: 3, readAt: 1794212000000 }, "2": { unread: 0, readAt: null }, ... }
```

我们依赖：`unread` `readAt`。

- `unread` 是引擎按「EntryId 水位之后还有几条」数好的，**客户端不自己数**
  （条数会被 fork / compaction 弄错，水位不会）。
- `readAt` 是上次标已读的时刻（epoch ms），可能为 `null` —— 那是引擎种的水位，
  `null` 不是「读到过」，只是一个还没被 `POST /v1/read/:id` 推进过的水位。
- 覆盖**所有**会话；没有水位的会话给 `{ unread: 0, readAt: null }`。
- 跟 `GET /v1/sessions` **一起拉**（同一帧并发），别一会拉一个。

### GET /v1/origins

会话血缘反查：`{ items: [{ id, ultimate, trigger, at, forced }] }`。
`id` 是会话 id（数字），`ultimate` 是终极模板/模板 ref 名，`trigger` 是点火的触发器 slug，
`at` 是点火时刻（epoch 毫秒），`forced` 只给手动 launch —— 触发器点起来的恒 `false`。
`?trigger=<code>` 按键过滤；手动建的会话**没有**条目。
状态与 `names` / `pins` 同住 `qrd3wh.sessions` 文档的 `origins` 键，不建表；重启后仍可反查。

### POST /v1/read/:id

把这条会话标成读到最新，body 空。回 `{ id, readAt, unread: 0 }`。

- 不存在的会话 → 404；只有 `POST`，别的动词 → 405。
- 打开会话时打它；水位只进不退（引擎那边钉住了）。
- **别自己推水位**，也别把「已读」写本地就完事 —— 引擎才是水位的事实源，重装客户端不该丢未读。

### POST /v1/sessions/:id/fork

body `{ entryId?, name? }`。`entryId` 省略 = **从最后一条分叉**。回 201 `{ id, at }`，
`id` 是新会话，`at` 是在父会话的哪条 entry 上分的叉。

- 分出来的会话在列表里带 `parent: {id, at}`。
- 名字不给就是空串 `""`（本库不生成默认名，见上面 `GET /v1/sessions` 的 `name` 那条），要区分就自己带 `name`。

### POST /v1/sessions/:id/rename

body `{"name":"…"}`。回 200 `{"id":514,"name":"新的名字"}`。

- **名字是引擎说了算的**：它 trim（`"  改过的名字  "` → `"改过的名字"`），trim 完是空
  就 400 `{"error":"name 是空的"}`。回包里的 `name` 才是生效值，客户端拿它覆盖本地。
- 空名字不能靠「发个空串试试」来判 —— 前端提交前就拦住，不然白跑一趟还弹一句错。
- 改名**没有回执的时间字段**：回包里只有 `{id, name}`，客户端就地改本地那一行的 `name`
  即可（`updatedAt` 该不该跟着动没验过，别在这里编）。

### 会话归档：`archive` / `purge` / 归档库 / `restore`

**这一版引擎起有**：内核 2026-11-10 起（ADR 0001）；客户端 v0.4.0 起接。
旧引擎把会话复印进 `data/archives/gen-YYYYMMDD.sqlite` + 写墓碑，**不是删**；
撕原件 = 真删（活库那行 + 那份复印件）。本库同语义，但 gen 文件后缀是 `.db`
（`archives/gen-YYYYMMDD.db`，见 `docs/decisions.md` 的 #67 决策）；四个方法在本库
分别是 session 命令 `archive_conversation` / `purge_session` / `list_archives` /
`restore_conversation`，UI 层一行 HTTP 都不碰。

#### GET /v1/openapi.json

只读机读视图：从 `meta::ROUTES` 生成 OpenAPI `paths`，与 `/v1/capabilities` / `/v1/_routes`
三处同源。加端点先改 `meta::ROUTES`，这里自动跟着变；不手抄第二份路径清单。

#### GET /v1/capabilities

一个发行版一张清单：`modules[]` 每个模块声明 `name` / `enabled` 与 `endpoints` 或 `reason`，
顶层给 `contract` / `server` / `runtime` / `services`。字段形状以
`crates/nous/server/tests/capabilities_shapes.rs::capabilities_declares_modules_and_reasons` 为准。

- #88：`modules[]` 每个模块（含未启用的）另带版本、类型、层、必需、来源 crate、说明六列，
  取自 `docs/plugins.md` 同一张表；判据 `capabilities_manifest.rs::manifest_metadata_matches_the_plugins_table_cell_by_cell`
  逐格比对编译后的清单与文档表。`crate` 可能是 `—` 或 `(待建) xxx`，不可当成可运行的包名。
- `layer` 四值封闭、`kind` 三值封闭；`enabled:false` 的模块用 `reason.code` 四值封闭解释原因。
- 六列是加法：老引擎没有这些字段时，桌面端降级成「版本 = server.version，类型 / 层 = —」，
  不据此判模块不存在。新字段一律只读，`endpoints` / `reason` 的既有语义不变。

#### POST /v1/sessions/:id/archive

body 空。回 200 `{"gen":"20261113","ids":[699],"rows":17}`。

- `gen` 是 8 位 `YYYYMMDD`，`rows` 是复印进去的条数。**客户端不读回包**（`Value` 收着），
  刷新靠重拉 `/v1/sessions` 和 `/v1/archive` —— 别拿回包自己改本地列表。
- **本库口径（#67）**：归档后 `/v1/sessions` **仍返回该会话**，带 `archived` 墓碑
  `{at, gen, purged:false}`；捞回后墓碑回到 `null`。旧引擎的「默认隐藏 + `?archived=1`」
  在本库不实现 —— 墓碑是事实源，显示/隐藏由客户端按墓碑决定，别在这里悄悄滤行。
- 失败：忙（会话有非终态任务）/ 有活后代 → 409 `{error}`，正文列出活分支。原样显示，别改写。

#### POST /v1/sessions/:id/purge

body 空。回 200 `{"purged":[699],"gen":"20261113","rows":6}`。**真删**，不可撤销。

- **必须先归档**；会话闲置、分支里没有活后代也是引擎的前置条件。不满足返回 409 / 400，
  形状还是 `{error}`，照原文显示。

本库口径（#64 + #67）：**必须先归档**（有 `archived[id]` 墓碑，否则 409）。一次事务删干净
conversations / entries / tasks / submissions / documents / document_revisions 与该会话的
pins / unread 条目，墓碑改写成 `archived[id]={at, gen, purged:true}`；`record_ids` 不删（id
永不复用，见 `docs/decisions.md`）。会话不存在 → 404，已 purged → 409。

#### GET /v1/archive

回 `{"gens":[{"gen":"YYYYMMDD","items":[…]}]}`，一个 `gen` = 一份 `gen-YYYYMMDD.db`，
组标题就写 `11月10日 · gen-20261110.db`。item 形状：

```
{ id, parent: {id, at} | null, entries: number, last: {id, kind, text} | null,
  name, archived: null | {at, gen, purged}, live: bool }
```

我们依赖：`id` `name` `archived` `live` `parent` `entries` `last`。

- `archived` 为 `null` ⇔ **没有墓碑**（捞回之后被抹掉）；有对象才谈「归档过 / 撕过」。
- `live` 是「活库里这行还在」，**引擎算好的**，客户端不推。
- 四态判据（客户端依赖它，照面板页 `mcfpqm` 的 `stateOf`；四态在真引擎上都实测出现过）：

| 判据（按顺序判） | 态 | 界面 | 给什么动作 |
|---|---|---|---|
| `archived.purged` | 已销毁 | 「已销毁」 | 打捞 |
| `live && archived` | 原件在 | 「原件在」 | 撕原件 |
| `live && !archived` | 已捞回 | 「已捞回」 | 无 |
| 其余 | 无墓碑 | 「无墓碑」 | 打捞 |

`purged` 必须**先判**：已销毁那条 `live` 也是 false 且 `archived` 也在，顺序反了会推成「无墓碑」。
四态**只看 item 自己**，别拿「id 在不在主列表里」当判据 —— 两份判据一定会分叉。

#### POST /v1/archive/:id/restore

body `{"gen":"YYYYMMDD"}`（哪一份复印件，取自 `GET /v1/archive`）。
回 200 `{"restored":[699],"gen":"20261113","rows":17}`。

- 捞回来之后 `/v1/sessions` 里那条**真的回来了**；`/v1/archive` 里它的 `archived` 变 `null`、
  `live` 仍是 true（「已捞回」）。
- **对「原件还在」（`here`）的会话返回 500，不是 409**：
  `{"error":"会话 699 已经在活库里 —— 不用捞"}`。所以 UI 只在「已销毁 / 无墓碑」上给「打捞」——
  这个约束是引擎强制的，不是产品挑的。
- 四个动作的回包都是同一形状 `{<动作过去分词>: [ids], gen, rows}`，客户端一律不读。

### presets：模板 / 预设（#60）

`presets` 是资源化的端点族（13 条 + 生成式 OpenAPI），**盘是唯一真相**，文件住在模板私库
（`NOUS_PRESETS`，默认 `/opt/Nous-Template`）：`<root>/<section>/<name>/preset.json`。
段只有两个：`Session-Template`、`Ultimate-Template`，**没有第三种**。没有进程内常驻的
`name → preset` map —— roster 由扫目录得出，盘上有目录 = 这条存在。

端点表（OpenAPI 生成物 `/v1/openapi.json` 是它的机器可读版本，字段形状以
`crates/nous/server/tests/presets_endpoints.rs` 为准）：

| 方法 + 路径 | 语义 |
|---|---|
| `GET /v1/presets` | 段清单，每段带 `count` |
| `GET /v1/presets/:section` | roster：`name` / `sha` / `version` / `category` / `order` / `archived` / `problem`（**清单即详情**） |
| `GET /v1/presets/model` | 预设能引用的模型清单（只读，沿用原状） |
| `GET /v1/presets/:section/categories` | 段内类别名册 |
| `PUT /v1/presets/:section/order` | 整段顺序，一次写全（幂等） |
| `GET /v1/presets/:section/:name` | 单条原文 + `ETag`（= `version`） |
| `PUT /v1/presets/:section/:name` | 建或覆盖；`If-Match` 缺失=新建、匹配=覆盖、不匹配=412；`?dry_run=1` 校验不落盘（validate 走同一条路） |
| `PATCH /v1/presets/:section/:name` | 只改元数据位：`{archived}` / `{name}` / `{category}` / `{order}` |
| `DELETE /v1/presets/:section/:name` | 删（软删即 `archived`） |
| `POST /v1/presets/:section/:name/copy` | 复制成新名，body `{name, requestId}` |
| `GET /v1/presets/:section/:name/revisions` | 修订清单：`n` / 时间 / `actor` / `sha` / 摘要 |
| `GET /v1/presets/:section/:name/revisions/:n` | 某一版原文（diff 由 Desk 算，无状态） |
| `POST /v1/presets/Ultimate-Template/launch` | #82：手动起一条终极模板（跳过触发器判定，`forced:true`） |
| `POST /v1/presets/:section/:name/rollback` | 回到第 `n` 版，原子写一次修订，body `{n, requestId}` |
| `GET /v1/openapi.json` | 从路由注册表生成的机器可读契约（presets 第一个接入方） |

- `sha` 是 `preset.json` **文件字节**的 sha256（不是清单里某个字段），roster 的 sha 必须
  等于读文件算出的值。
- `version` 是乐观锁的 ETag；`If-Match` 不匹配 → 412 且磁盘字节不变。
- `?dry_run=1` 与真 PUT 共用同一份校验：坏 JSON 都是 400，区别只在 dry_run 不写盘。
- **`spec` 可缺省**：老条目（0.1.7 之前写下的）没有这个键，PUT 会补一个 `{}` 再存；
  `name` / `summary` / `category` 仍然必需。不把「再存一遍老模板」卡在 400。
- 写端点整段串行锁；`requestId` 是幂等锚：重放回第一次的回执，`version` 不涨，
  幂等证据看 `version` 与磁盘，不看 `changed`。
- 修订落在 `nous-plugins.db` 的 `presets_revisions` 表（`section` / `name` / `n` / `sha` /
  `body` / `actor` / `at` / `request_id`），**不是私库 git 历史**（那个是 #75 的 `history`）。
  不 spawn `git`；`rollback` 替代 `git revert`。
- 人手 `scp` 改文件仍可行：那条路走内部重扫，下一版 PUT 看到新盘面。
- `publish` 不在这里：`POST /v1/repos/presets/commit` 是 #75。
- 字段形状以 `crates/nous/server/tests/presets_endpoints.rs::put_writes_real_file_and_roster_sha_matches_file_bytes` 为准（本文件只索引端点与约束）。

下面这段是 2026-11-19 前的旧形状，留作迁移背景；**新契约以上表为准**，
旧 `files` / `pending` / `history` / `POST rename` / `GET|POST order` 已删：

### 旧 GET /v1/presets/:section（存档）

模板（通讯录）那一页的数据。**段（section）不是只有一个**：同一个私仓按段放，
`Session-Template`（会话模板）和 `Ultimate-Template`（终极模板）各自一套端点、一套数据、
**一套类别词表**（`categories.json` 住在段目录里）。

返回 `{ section, title, repo, presets: [...], archived, categories, ... }`。

每条 preset：

```
{ name, summary, description, category, spec, specError, tracked, dir, resolved }
```

我们依赖：`name` `summary` `category` `spec` `specError` `tracked` `resolved`
\+ 顶层的 `categories` `repo`。

- `categories` 是**这一段的全部类别路径**，引擎已排好序（父在子前、逐段 `localeCompare('zh-CN')`），
  里面**包含零计数的类别**（词表里先建好、还没挂东西的）—— 客户端不许滤掉。
- `category` 是字符串数组，`/` 表示层级：`写作/周报` 自动蕴含父级 `写作`。一条可以挂多个。
- `repo` 是**这一段的**仓状态 `{ head, dirty, ahead, behind, error }`；`dirty` 按段算，
  `ahead`/`behind` 是整仓的。`head` 可能是 `null`（仓读不了）—— 别拿它当字符串硬切。
- `specError` → 这条自己写坏了，红（字段名 + 一句原因）。本库按形状校验填，不 spawn git。
- `tracked` 在本库恒 `null`：不 spawn git，无从判「仓里有没有」；`null` 不是 `false`。
  `dir` 是文档根下的真实绝对路径（`<root>/<kind>/<section>`）。
- `resolved.*.state === "invalid"` → 引用失效，现在开不了。
- `tracked === false` → 盘上有、仓里没。
- **能否启动的判据全是引擎算好的**，客户端不自己拼规则。加了规则就等于可以跟引擎吵起来。
  （唯一客户端自己算的是**类别计数**：引擎不给，算法是「含后代 + 同一条预设按路径去重」。
  那是派生量，不是第二份事实源。）
- 能开的行：`POST /v1/sessions { preset: "Session-Template/<name>", name }`，`name` 带时间。

#### 终极模板那一段是另一种东西（`spec` 里长这样）

```
{ v, name, summary, description, category, trigger, params, session, enabled }
```

- `trigger` 是触发器名字，`params` 是它的参数（对象：字符串 / 数字 / 布尔）。
- `session` 指向 `Session-Template` 段里的一条会话模板。
- **这一段现在只读**：`enabled` 怎么切、`trigger` 用哪个端点启用，协议都还没定。
  桌面端只显示（`⚡ trigger` / `→ session`），点它什么也不发生。要动它先定端点。

#### POST /v1/presets/:section/rename

改一条模板的名字。body `{ name, to }`。

名字 = 目录名 = 主键（形状校验要求 `preset.json` 里的 `name` **等于**目录名），所以
**一次改名引擎同时动三处**，客户端一件都不许自己动手：

1. 目录 —— `git mv`，git 认得出是改名（历史不断）；未发布的（盘上有、HEAD 里没）退回直接搬。
2. 清单里的 `name`。
3. **引用了它的终极模板的 `session`** —— `ultimate.json` 的 `session` 是**名字字符串**，
   不改的话那条终极模板立刻是「会话模板不在 Session-Template 里」，而改名的人不知道去哪修。

回包 `{ ok, commit, pushed, name, to, refs }`，`refs` 是**被动到的终极模板名**（连带改的）。
界面要把 `refs` 说出来 —— 改名顺手动了别人的东西，不说一声就是偷偷改。
**本库不 spawn git**：`commit` 与 `pushed` 一律 `null`，不拿 `{}` 冒充「有一笔提交」。
单条文档不存在与不认识的 kind 一律 `404`，body 带可用值清单，不许 `500`。

- 只有 POST；`to` 走和新建同一套名字规则（`400`）、目标已存在 `409`、源不存在 `404`。
#### GET / POST /v1/presets/:section/order

手动排序。**全局一份**（所有类别共用一套顺序），跟通讯录一样。

`order.json` 住在段目录里，`{ v: 1, order: ["第一条", "第二条", …] }`。

- **它不能写进 `preset.json`**：`PRESET_KEYS` / `ULTIMATE_KEYS` 是白名单，多一个键就报
  「不认识这个键」—— 而它们报得对（拼错的键等于那条设置静默没生效）。
- 是**文件不是目录**：`scan` 和仓里的 `index.mjs` 都只认目录，不会把它当成一条模板。
- `GET` 回 `{ order: [...] }`；`POST` 收 `{ order: [...] }` —— **完整顺序**，不是「谁挪到
  谁前面」。一次落盘、一次提交，服务端不重放移动。
- 回包的 `order` 是**盘上真正存下来的那份**（服务端会把盘上已经不存在的名字滤掉）。
- `GET /presets/:section` 的 `presets[]` **已经按它排好** —— 客户端别自己再排一遍。
  表里没有的条目排在后面、按名字（新模板位置稳定，不会每次刷新换一处）。
- 读不到 / 读坏的 `order.json` 一律当「没有顺序」：排序挂了不该让整段打不开。

- **索引不跟着这次提交走**：`regenIndex` 会把「新建了还没发布」的条目也扫进 `index.json`，
  一起提交就成了「仓说我有它、实际推不上去」。留成 `M index.json`，下次发布收。

---

### GET /v1/combos/:section · PATCH /v1/combos/:section/:name

组合（终极模板里「触发器 + 会话模板」那一条）的开关面。**段只有一个值：`Ultimate-Template`**
（`Session-Template` 的条目是「开一条会话用什么」，没有触发器，不是组合；传它一律 `400`，
不静默返回空清单）。名册由扫 `<root>/Ultimate-Template/<name>/preset.json` 得出
（`<root>` 是 `NOUS_PRESETS`，与 `presets` 共用数据根），进程内不留常驻名单。

`GET` 回 `{ combos: [...] }`，每条：

```
{ name, trigger, params, session, enabled, problem }
```

未知字段原样透传（`serde_json::Value` 收着，不 `deny_unknown_fields`）。

- `enabled` 是**引擎算的**：`spec.enabled !== false`（缺省 true）**且**引用的 trigger /
  preset 存在可用；算不出来则 `enabled=false` 且 `problem` 写人话。客户端只显示 + 写穿，
  不自己拼规则。
- `params` 是对象（字符串 / 数字 / 布尔）；`session` 指向 `Session-Template` 段的名字。
- `problem` 是字符串：写坏 / 引用没了都在这里说清楚，页面照原文显示。

`PATCH` 只改 `{ enabled: true|false }`，不碰别的字段、别的组合。必带 `If-Match`
（版本不匹配 → 412）与 `requestId`（幂等锚：重放回第一次回执，`version` 不涨）。
回包 `{ name, trigger, params, session, enabled, problem, version, requestId }`；
写的是 `spec.enabled` 那一位，不是新增一份开关表。
**文件里没有 `spec` 对象时补一个 `{}` 再写**（老条目就是这样），不回 400 ——
「没有 spec」是旧形状，不是坏数据。

> 旧版 `POST /v1/combos/:name/enable` 与 `/disable` 已取消：`enabled` 是一个字段，不是
> 两个动作，两条路径等于给同一个位开两个入口。只留一条 `PATCH {enabled}`。

### POST /v1/combos/:section/:name/fire（#82）

试跑一次组合：用 `#73` 的 `evaluate` **问一次触发器**，判定 `fire:true` 才建会话；
不 fire 就不建会话（`sessions` 条数不变），也不写血缘。只在 `Ultimate-Template` 段上成立。
body 必带幂等锚 `requestId`。回包给判定结果 `fired`、人话 `reason` 和可选 `sessionId`；
不 fire 时 `sessionId` 为 `null`。

起会话时写血缘 `origins`：`{ultimate, trigger, at, forced:false}`（#74，与点火路径同一条
`record_origin`）。`requestId` 重放回第一次回执，不新建会话、不追加血缘。

### GET /v1/models

返回默认模型 + 模型列表。

字段形状以 `crates/nous/server/tests/wire_shapes.rs::models_response_pins_default_and_item_field_shape` 为准（本文件只索引端点与约束）。

- `default` 是**盘上那只眼当前指着的**模型，不是我们的默认值。
- `thinkingLevels` 从少想到多（`off` … `max`），**每个模型不一样**。换模型必须按新模型的表
  重新夹档位（`off` 一定要在表里，没有就退成 `off`）。
- 242 个模型是常态 —— 模型那一列必须能滚，别想着一次画完。
- #65：多一个 `catalog` 诊断块（字段形状见 `crates/nous/server/tests/device_endpoints.rs`）。
  `missing` + `items: []` 是**诚实空表**（没有编目来源），客户端别把它当成「加载中」无限重试；
  `ok` 时 `source` 指明是内联（`NOUS_MODELS`）、文件（`NOUS_MODELS_FILE`）还是远端（`remote`）。
- #79：远端刷新成功后内存里的编目优先于文件来源；`fetchedAt` / `error` 由刷新写入。

### POST /v1/catalog/refresh

拉一次远端编目写进本地缓存，回 `{ default, items, fetchedAt, error? }`。
远端入口是 `NOUS_CATALOG_BASE`（`{base}/models.json`）+ `NOUS_CATALOG_TOKEN`（Bearer）。
字段形状以 `crates/nous/server/tests/catalog_refresh.rs` 为准（本文件只索引端点与约束）。

- 成功：远端三条就回三条，`fetchedAt` 非空，`error` 缺席；`/v1/models` 立即换成新表。
- 失败（含 HTTP 500）：**回 200 但带 `error`**，`items` 仍是旧表，绝不把模型表清空。
- 密钥只在内存的请求头里过一手：不落盘、不进日志、不进响应、不进会话记录。换密钥刷新即可。

### PUT /v1/default

body `{ provider, modelId }`，写回同一份编目的 `default` 字段，回 `{ default, wrote: true }`。

- 模型不在 `items` 里 → 400（`{ error: string }`），不许写一个不存在的模型。
- 写是原子的（临时文件 + rename）：写入过程中断，旧文件与 mtime 不变。
- 内联编目（`NOUS_MODELS`）没有文件路径可写 → 400，改用 `NOUS_MODELS_FILE`。

### GET /v1/skills

`{ root: string, present: boolean, missing: string | null, categories: string[], repo, skills: [...] }`。
`root` 永远是解析后的路径（未显式要求「不要数据根」时不会是 `null`）；目录不存在时
`present=false` 且 `missing` 给一句人话，`skills=[]`。服务端只读，不建目录、不写私库。
技能那条：

```
{ name, path, description, dir, file, hidden, category: string[], tracked, load: { state } }
```

- **只列 `load.state == "ok" && !hidden`**。`hidden` 是 `disable-model-invocation`，
  勾上也不会动；`state != ok` 的加载不出来。
- `categories` 只是词表，和技能身上挂的 `category` 取并集才是栏里该有的那些。
- `?usage=1`（#85）：反查模式，回 `{ <技能名>: [<会话 id>, ...] }` —— 谁在用这个技能。
  没人用就回 `{}`（空映射，**不是 404**）。不带该参数仍是上面的技能目录形状。

### 资源仓管理面：`/v1/{skills,prompts,extensions}/{files,file,create,publish}`

Desk 的三个编辑页（技能 / 提示词 / 扩展）共用这一副骨架：列清单 → 读 → 存 → 新建 → 发布。
文件读写与 git 语义都复用 `nous_repo`（`/v1/repos/:repo/*` 的同一套），这里只是按条目目录
切片的翻译层。**所有写端点都不自动 commit**：落盘归 PUT，提交归 publish。

| 方法 + 路径 | 语义 |
|---|---|
| `GET /v1/{kind}/files?name=` | 回 `{name, files:[{path, size}]}`。`path` 是**条目目录内**的相对路径，递归到底（不截层数），跳过点开头与 `node_modules`，单次最多 200 个、单文件 >512KB 的不列。条目不存在 404。 |
| `GET /v1/{kind}/file?name=&path=` | 回 `{name, path, text, etag}`。`etag` 是内容 sha256（写回时当 `base`）。二进制（含 NUL）/ 非 UTF-8 → 400，路径含 `..` / 绝对路径 → 400。 |
| `PUT /v1/{kind}/file` | body `{name, path, text, base?}`。`base` 给上就是 If-Match：与磁盘现状不符回 409（不静默覆盖）；省掉 = 新建/硬写。回 `{name, path, etag, created}`。 |
| `POST /v1/prompts/create` | body `{name, summary, category?, body?}`。落 `<name>/PROMPT.md`（不给正文就写 `# <name>`）+ `<name>/nous.json`（`summary` 在这里）。201 回 `{name, path, files}`；重名 409，`summary` 空 400。 |
| `POST /v1/skills/create` | body `{name, description, category?}`。落 `<name>/SKILL.md`（frontmatter 带 `name`/`description`）+ `<name>/nous.json`（`summary` 是空串）。201 回 `{name, path, files}`；重名 409，`description` 空 400。 |
| `POST /v1/{kind}/publish` | body `{message?}`（可省）。把私库里 dirty 的路径交给 `nous_repo::commit`（内容闸 + pathspec 防搭车 + push 都在那层）。回 `{commit, pushed, refs}`；只有「已提交没推」时只 push（`commit:null`）；没有改动回 400。默认文案随仓：`skill: 更新技能仓库` / `prompt: 更新模板仓库` / `extend: 更新扩展仓库`。 |

- `{kind}` 三值：`skills`（私库 `NOUS_SKILLS`）/ `prompts`（`NOUS_PROMPTS`）/
  `extensions`（私库键是 `extend`，根 `NOUS_EXTEND`）。扩展的 `create` 不在这里 ——
  它在 `/v1/extensions/create`（#84，带骨架与运行时），`files` / `file` / `publish` 共用本表。
- 名字规矩沿用旧面板 `checkName`：`^[a-z0-9][a-z0-9._-]*$`、≤64 字，且不许撞仓库根
  的保留名（`index.json` / `nous.json` / `SKILL.md` / `PROMPT.md` / `sources.json` / `node_modules` …）。
- 私库没装配（对应的 `NOUS_*` 目录不存在）→ 409，不退回空 200。
- 判据见 `crates/nous/server/tests/resource_admin_endpoints.rs`。

### GET /v1/extensions

正常回真装载清单（#81 解冻，不再是 `{ items: [] }` 的冻结占位）：

```
{ items: [{ name, version, kind, loaded, pid, state, error? }] }
```

- `name` / `version` / `kind` 来自扩展自己的 `nous.json`（`kind` 是 `category`，缺席给 `native`）。
- `loaded` = 清单 + 握手都过；装载失败也留一项（`loaded:false` + `error`），不静默消失。
- `pid` 是扩展子进程 pid（`u32`，没进程时 `null`）；`state` 是五态之一，进程死了但状态机
  还没推进时如实给 `crashed`（不是 `normal`）。`error` 只在失败/崩溃时有值。
- 源码根走 `NOUS_EXTEND`（默认 `/opt/Nous-extend`），产物在根下 `artifacts/`。
  没装配运行时（测试路由）仍回 `{ items: [] }`，不伪造内容。
- `?usage=1`（#85）回 `{ <扩展名>: [<会话 id>, ...] }` —— 扫会话记录里的 `extensions` 字段；
  没人用回 `{}`，不因为一个可选参数把端点判死。

### POST /v1/extensions/reload

按盘上现状重装所有已装载扩展（真起新进程，pid 会变），回 `{ reloaded: [name, ...] }`。
失败的那一项保留旧进程与旧状态，错误进扩展自己的诊断；不返回假成功。
文件管理面（`{file,files,publish,create}`）由 #75 的 repo 层提供，不在这里重复实现。

### 扩展管理面（#84）：create / duplicate / install / uninstall / 类别 / usage / pending

人手动作都写在 `NOUS_EXTEND` 这份私库里，git 发布与 pending 一律复用 #75 的
`/v1/repos/extend/*`，这里不重复实现。写端点都不自动 commit。

| 方法 + 路径 | 语义 |
|---|---|
| `POST /v1/extensions/create` | body `{name, category?}`。生成 `<name>/` 下三个文件：`nous.json` / `README.md` / `main.rs`。201 回 `{name, dir, files}`。名字已存在 409，非法名字 400。 |
| `POST /v1/extensions/duplicate` | body `{name, to, unbind?}`。整目录复制，只改 `nous.json` 的 `name`（`unbind=true` 时连 `file`/`entry` 一起改成 `to`）。其余文件字节不变。201 回 `{name, to, files}`。 |
| `POST /v1/extensions/uninstall` | body `{name}`。源码移到 `<data>/extend-trash/<name>-<epochms>/`，并把名字写进私库根的 `deleted-extensions.json`（防 git pull 复活）。运行时同步卸载。核心扩展 `nous`/`skills`/`prompts` → 400。回 `{name, trashed, deleted}`。 |
| `POST /v1/extensions/install` | body `{name}`。从垃圾站最新一份（没有就从 HEAD）装回 `<name>/`，经 #81 的装载入口起进程。回 `{name, state, from}`，`state` 是装载后的真状态。 |
| `GET /v1/extensions/categories` | 回 `{categories:[...]}`，读私库根的 `categories.json`（在私库里，随发布进历史）。文件不存在回空表。 |
| `POST /v1/extensions/categories` | body `{op:"add"\|"rename"\|"remove", name, to?}`。`rename` 同时改写所有挂它的扩展的 `nous.json.category`。回 `{categories, affected}`。 |
| `POST /v1/extensions/category` | body `{name, category}`。给一个扩展挂/改单值类别（`null` 清除），写 `nous.json`。回 `{name, category}`。 |
| pending 三件 | **没有** `/v1/extensions/pending` 端点：**直接复用** `/v1/repos/extend/pending` / `/pending/diff` / `/pending/commit`，不另开实现。 |

- `deleted-extensions.json` 是删除名单而不是墓碑表：名字在里面，扫描就跳过这个扩展，
  即使 `git pull` 把目录带回来也不会复活。名单随私库发布，重启后仍生效。
- `categories.json` 是字符串数组；`nous.json` 的 `category` 是单值字符串（见 SDK 清单），
  不是 skills 那种数组。词表与扩展字段共用同一套字符串，不另造 id。
- 判据见 `crates/nous/server/tests/extension_admin_endpoints.rs`。

### GET /v1/search

`GET /v1/search?q=&kind=&limit=`（#85）：服务端一次扫描完成，**客户端不许逐会话拉正文**。

- `kind=sessions` 搜会话名 + entry 正文；`kind=documents` 搜 durable 会话文档名
  （`kind/section/name`）与正文；`kind=all`（默认）两者合并，新的在前。后者是会话内部文档的
  只读投影，与 #90 下线的内容端点无关。
- 回 `{ items: [{kind, id, title, snippet, at}], total, truncated }`。`kind` 是
  `"sessions" | "documents"`；`id` 是会话 id 的字符串 / 文档地址 `kind/name`；
  `snippet` 带命中上下文（前后各 40 字符），`at` 是 epoch 毫秒，来源不明给 `null`。
- `total` 是**命中总数**（不受 `limit` 影响）；`truncated = total > items.len()`。
- `limit` 默认 20、上限 100、最小 1（越界夹到边界）。大小写不敏感；`q` 先 `trim`，
  少于两个字符回 `400 {error}`（不许全表扫）。

#71 把「盘上真相」补上（只解析 `SKILL.md` 的 frontmatter，不碰 git）：

- `name` 缺省用目录名；`description` 可为 `null`；`category` 是字符串数组；
  `hidden` 来自 `disable-model-invocation: true`（条目仍全部返回，隐藏是渲染层的事）。
- `attachments` 是目录内除正文外的文件数，跳过点开头、`node_modules`、`test`。
- `load.state` 为 `problem` 时带 `detail` 原文 —— 缺正文或 frontmatter 坏都不能恒报 ok。
- `tracked` **可空，当前不提供（恒 null）**。判定要问 git，而本引擎不 spawn git；
  填 `false` 是撒谎。
- `repo` 块只给 `root` 与 `present`（#63 形状），**没有 git 状态字段**。
- `categories` 取词表与技能 `category` 的并集去重（`categories.json` 存在就读，不写回）。
- 判据见 `crates/nous/server/tests/skills_endpoints.rs` 与 `crates/nous/skills/tests/scan.rs`。

#83：建会话 / `POST /v1/sessions/:id/config` 选的 `skills` 落在会话的 `pi.agent` 文档里，
起轮时按**选中顺序**读 `<skill>/SKILL.md` 正文（去掉 frontmatter）拼进 `pi.system` 的
`model[0].sections.prompts`。技能缺失 / frontmatter 坏只留 `skill: missing` / `skill: problem`
诊断，轮次照跑；只读会话照样注入。判据见 `crates/nous/server/tests/skill_injection.rs`。

### /v1/documents —— v1 不提供（#90 下线）

`documents` 的数据库文档与 12 条泛型端点已下线：内容真相只有四个私库的文件
（`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` / `NOUS_EXTEND`）。
本服务不注册任何 `/v1/documents/*` 路径，`/v1/_routes` 与 `/v1/capabilities` 不声明
`documents`；请求这些路径得到 404。判据见 `crates/nous/server/tests/documents_off.rs`。

### GET /v1/prompts

`{ default: string | null, root: string, present: boolean, missing: string | null, categories: string[], prompts: [...] }`。
`root` / `present` / `missing` 与 skills 同义：有路径但没目录是正常状态，不是 `null`。
模板那条至少有
`{ name, description?, category?, load: { state } }`。
字段形状以 `crates/nous/server/tests/prompts_endpoints.rs::prompts_lists_real_directories_and_names_an_unconfigured_default` 为准。

- 真读 `NOUS_PROMPTS`：每个子目录一条 `path`，至少要有 `PROMPT.md` + `nous.json`；
  缺哪份就在 `problem` 里点名，不许把坏目录从列表里抹掉。
- `settings.json` 的 `default` 指向不存在的模板时照样回那个名字（不变成 null），
  并在 `problem` 里说清“不存在”。
- `prompt:<名>` 会话模板在起轮时把 `PROMPT.md` 正文放进 `pi.system` 的
  `model[0].sections.prompts`（见 `crates/nous/server/tests/prompt_injection.rs`）；
  模板缺失时轮次照跑，诊断以 `template: missing` 写进同一段。

- 和技能不是一回事：**模板是单值**（每轮都带的那份规则），技能是集合。所以列表里
  模板用单选框、技能用复选框 —— 形状本身在说能选几个。
- 仓库 `settings.json` 的默认值只用来**预勾**，选不选仍然是人的事。

### GET /v1/machines

`{ at, machines: [{ name, user, port, ips: [{addr,label}], addr, note, tags }] }`。

- #65：机器编目优先读 `NOUS_MACHINES_FILE` 给的 JSON；没有就用内置两台（形状不变）。

- 没有 `machine` 字段 = **本机**。名字要和创建请求里的 `machine` 对得上。
- 本库的静态编目不知道具体 `user` / `port` / `ips` / `addr`，就回 `null` / `[]` / `""` ——
  不编造地址。旧字段（`hostname` / `os` / `arch` / `triples` / `rustc` / `roles` /
  `annotations` / `items`）保留一版，客户端按任一套都能读到。

## 私库读写与发布（#75）

四套私库（`:repo` ∈ `presets | skills | prompts | extend`）共用一套端点；文件 CRUD
在 `nous-repo` 的公共 API，HTTP 只暴露发布与读视图。字段形状以
`crates/nous/server/tests/repos_endpoints.rs` 为准（本文件只索引端点与约束）。

- `GET /v1/repos/:repo/status` → `{root, head, branch, ahead, behind, dirty, locked}`。
- `GET /v1/repos/:repo/pending` → `{units:[{name, state, aheadCommits, paths[]}]}`，按目录单元分组。
- `GET /v1/repos/:repo/pending/diff?path=` / `history/diff?name=&hash=&path=` → `{before, after}`（全文，不是 git patch）。
- `GET /v1/repos/:repo/pending/commit?hash=` → `{hash, message, files[]}`。
- `POST /v1/repos/:repo/commit` `{paths[], message, requestId}` → `{commit, pushed, refs}`。
  `pushed` 三态 `true | false | null`，推送失败不能报 true；pathspec 防搭车。
- `GET /v1/repos/:repo/history?name=&limit=` → `{items:[{hash, at, actor, message}]}`，是私库 git 历史，
  与 `#60` 的引擎内 `revisions` 不同，不许混。
- `POST /v1/repos/:repo/{archive,restore,purge}` `{name, requestId}` → `{commit, pushed}`。

#### 变体（#76，只给 `skills` / `extend`）

绑定表是数据文件 `data/skills.local.json` / `data/extensions.local.json`：
`{v:1, variants:{family:{id:{root,branch}}}}`，本地状态，不进私库发布。变体是仓库自己的
worktree + 分支：主干（main/master/HEAD）永远不许变。

- `GET /v1/repos/:repo/variants` → `{v, variants}` 原样回绑定表（缺文件回 `{v:1, variants:{}}`）。
- `POST /v1/repos/:repo/variants/bind` `{id, paths?}` → `{id, root, branch}`。分支存在就 checkout，
  不存在就 `-b`；未提交的改动从工作区拷到 worktree 并在变体分支提交。
- `POST /v1/repos/:repo/variants/unbind` `{id}` → `{id, removed:true}`；只解绑定，树还在。
- `POST /v1/repos/:repo/variants/branch` `{id, branch}` → `{id, branch, root}`；
  `main` / `master` / `HEAD` → 400。
- `POST /v1/repos/:repo/variants/publish` `{id, message, requestId?}` → `{commit, pushed, branch}`；
  只 commit/push 变体分支，主干不动。
- `POST /v1/repos/:repo/variants/prune` `{id, force?}` → `{unmerged, remoteBranch, removed:true}`；
  未合并且不带 `force:true` → 409，确认后删 worktree + 本地分支 + 远端分支。
- `GET /v1/repos/:repo/variants/usage?id=` → `[会话 id...]`：扫会话 `skills` / `extensions` 字段
  反查谁在用这个变体；没人用回空数组，不 404。

#### 上游追踪（#77，`:repo` 恒为 `skills`）

镜像是 `data/upstream/<name>/` 缓存（不进内核库），订阅清单 `sources.json` 在技能私库里，
`{v, sources:[{name,url,branch?,prefix?,version?}]}`，整份替换后走 `/commit` 才进历史。
`version` 只由 `pin` / `sync` 与首拉基线写，其余动作一律不改。

- `GET/POST /v1/repos/:repo/sources` → `{v, sources:[...]}`；GET 只读盘上正文，POST 整份替换（写盘不提交）。
- `GET /v1/repos/:repo/upstream` → `{sources:[{...,mirror,mirrorHead,pinned,newer,drift,bytes,entries}], errors}`。
  `pinned` 是认的 commit；`newer` = 远端 HEAD ≠ `pinned`（上次 check 的缓存）；
  `drift` = 镜像 HEAD ≠ `pinned`，或镜像工作区有手改（认的版本与看到的不是一版）。
  没有 check 过就 `newer:false`，打开面板不联网。
- `POST /v1/repos/:repo/upstream/{refresh,pin,check,sync}`：
  `refresh` 只拉不改 `pinned`；`pin {name,commit?,requestId}` 是日常唯一写 `version` 的入口；
  `check {name?}` 只 `ls-remote` 问 refs，写 `.git/nous-check.json` 缓存，不下载对象；
  `sync {name}` = refresh + pin 到远端 HEAD。首拉自动记基线是唯一一次非 pin 写入。
- `GET /v1/repos/:repo/upstream/tree?name=&path=` → `{items:[{path,sha,mode,size}]}`（只读上游 tree）。
- `GET /v1/repos/:repo/upstream/file?name=&path=` → `{path,text}`（只读 upstream blob）。
- `GET /v1/repos/:repo/upstream/files?name=&path=` → `{files:[{path,state,size}]}`；
  `state ∈ same|diff|missing`，按 git blob sha 比本地正文。
- `POST /v1/repos/:repo/upstream/{copy,adopt}` `{name,path,to?,requestId}`：
  抄一条上游技能（整目录 + 附件）；`adopt` 在 `nous.json` 写 `upstream` foot 标记
  `<source>/<path>@<sha>`，`copy` 只写 `copiedFrom` 便条、不写 foot。目标已存在 → 409，
  源不存在 → 400；写盘不提交，发布走 `/commit`。

- `:repo` 不在四值里 → 400；`If-Match` 不符 → 409（回包带磁盘当前正文）；内容闸失败 → 400
  且 `{field, problem}`，不 commit 不 push。推送失败不改盘上文件。

### GET /v1/fs/dirs?path=&machine=&hidden=

`{ path, dirs: [{ name, path, hidden }], truncated }`。**列的是引擎那台机器上的目录。**

- 不带 `path` = 引擎给的起点：`NOUS_START_DIRS` 的第一条（冒号分隔）或进程 cwd；
  `path: ""` 是起点，不是「没有路径」。**不做 home 展开**：`~` 由客户端展开，
  引擎只认绝对路径。
- 只列子目录（符号链接不算目录），按名字稳定排序；点开头的目录默认跳过，
  `?hidden=1` 时收录并标 `hidden: true`。最多 500 条，超出时 `truncated: true`。
- `machine` 省略 / 空 = 本机。远端机器（`machine` 不等于本机主机名）回 **501**
  `{ "error": "ssh 未实现" }`，**不许静默退回本机目录**（SSH 远端由 env-ssh 另票）。
- 错误：相对路径 / `~` 开头 **400**，绝对路径不存在 **404**，权限不足 **403**，
  路径指向文件 **400**，其余读失败 500；错误体统一 `{ error: string }`。
- 只读：这个端点不改目标目录（mtime 与内容都不变）。
- 浏览发生在引擎进程里：引擎以后跑在远程机器上，这一页一行都不用改。

### POST /v1/sessions —— 建会话

字段形状以 `crates/nous/server/tests/wire_shapes.rs::create_session_request_and_response_are_snapshotted` 为准（本文件只索引端点与约束）。

- `skills` 可省 = 不挂。
- 名字省略就是空串 `""` —— 本库不生成默认名；显示为「对话 N」是客户端自己的占位，别写回库里。
- `template` **总是发**：`null` 是「明确不要」。省略会被服务端当成使用仓库默认值，于是
  「点了无模板又被塞回来」。
- `extensions` **只能加，不能减**（见下面那条 ⚠）。
- 请求键是 **camelCase**（`thinkingLevel` / `cwd` / `machine`）。写成 snake_case
  不会报错，只会**悄悄不生效**。键名语义属于约束，不等于这里抄了完整 body 形状。
- 201 回来带 `missing` / `missingExtensions` / `missingTemplates`：引擎那边找不到的东西，
  客户端要如实说，别当成功。
- 建完再 `POST /v1/selection { id }` 把盘上那只眼切过去（切不过去不该把会话判死）。

- ⚠ **`extensions` 只能加，不能减**（2026-11-16 实测，不是推断）：内核
  `createConversation` 的 `settings.extensions` 无条件带
  `coding-tools + nous + skills + prompts`，请求里的 `extensions` 只用来**额外 add**。
  所以 `[]` 不是「一个都不要」而是「就用默认四个」，勾掉核心三条也不生效。
  只有**勾上非核心扩展**真会生效；`missingExtensions` 会如实点名找不到的 id。
  **别把空数组当「清空」写进文档或 UI** —— 桌面端 `ExtKind::Core` 那几行能取消勾选是
  一个「界面说能做、引擎做不到」的假动作，详见 `decisions.md` 那条。
  反悔点（要真支持减）：改 qrd3wh 的 `settings.extensions`，让它按请求清单来；
  改完回来把这句话和上面那段 body 注释一起改掉。

- **「复制设置」抄不了扩展**：`GET /v1/sessions` 的每一项里**没有**这条会话生效的扩展清单
  （只有建会话的 201 响应 `CreatedSession.extensions` 才有）。所以复制时只能省掉
  `extensions` 这个键 = 引擎给默认（全开），跟原来那条可能不一样。要补上得先让会话对象带上它。


## 引擎改了会怎么坏（按我们依赖的字段）

| 引擎改动 | 我们会怎样 |
|---|---|
| 删 `busy` / 换成布尔 | 列表忙闲角标消失或全红 |
| 删 `last` / 换形状 | 列表预览空了 |
| `parent` 语义变 | fork 过滤逻辑失效 |
| 删 `updatedAt` / 单位换成秒 | 列表时间全空，或者每条都显示成 1970 |
| `categories` 里把零计数的类别滤掉 | 通讯录里「建了但还没挂东西」的类别消失 |
| `category` 的 `/` 改成另一种分隔符 | 层级树断成一层，父级筛选丢失整枝 |
| `repo` 整块没了 / `head` 变必填 | 段脚注显示「仓状态未知」，最坏是解析失败 |
| 终极模板的 `trigger` / `session` 改名 | 那一段的 chip 没了（只是少显示，不报错） |
| `/v1/unread` 改回 `{items:[]}` / 改成数组 | 未读角标全灭（形状对不上） |
| `unread` 改成客户端自己数 | 水位退化成条数，fork 之后未读数永久错位 |
| `POST /v1/read/:id` 语义变（不再推到最新） | 未读点了不消，或者一次点掉不该消的 |
| SSE 事件改名 / `changes[]` 换结构 | 气泡不更新，流式死掉，症状是「能发出去但一个字不出来」 |
| `requestId` 幂等被去掉 | 重发会答两遍 |
| `/v1/selection` 语义变 | 三栏高亮错位 |
| `archive` 不再从 `/v1/sessions` 藏掉 | 归档后那一行不消失，看着像没成功（我们不在本地滤） |
| `archived` 墓碑字段没了 / 恒为 `null` | 四态全退化成「无墓碑」，「原件在」和「已销毁」都认不出来 |
| `archived.purged` 改名 / 换语义 | 撕过的显示成「原件在」，会让人重复撕 |
| `live` 没了 / 恒为 true | 「已捞回」和「无墓碑」颠倒，该给「打捞」的不给 |
| `restore` 收 `gen` 改成收别的 | 打捞 400，归档库页点了没反应 |
| `combos[].enabled` 改成客户端自己算 | 开关状态跟引擎分叉，Desk 灰掉的页会误亮 |
| `combos` 的 `problem` 不再是人话 / 恒空 | 引用失效的组合看着像能用，点了才报错 |
| `PATCH /v1/combos` 丢了 `If-Match` 或 `requestId` | 并发覆盖 / 重放翻两次开关，version 与盘面分叉 |

所以：启动时打 `GET /v1/device` 记引擎版本，About 里显示，出问题先对版本。

---

## 另一个来源：`d6s4v1`（`agent.sqlite` 的 X 光机）—— **已失效**

> **2026-11-18 起这一节只是存档**：`d6s4v1` 是旧面板的块，面板已下线，`/api/*` 本库一条都没实现。
> 桌面端的数据监控室因此只会拿到 404（`crates/nousdesk/src/data_console.rs` 的注释里记了这件事）。
> 下面留着是因为**等本库真出等价端点时**（监控库/文件/成本这类只读面），这几条教训仍然成立：
> `mtime` 是毫秒、WAL 下文件长度与页数不等。字段形状别照抄。

数据监控室有一半数据**不走引擎**，走的是面板 `d6s4v1`：`/p/<board>/d6s4v1/api/*`。

客户端在 `crates/nous-engine/src/console.rs`，五个端点：

**这段只做端点索引，不抄字段**：`GET api/overview`
（文件 / pragma / 字节统计 / 表与索引 / entries 的 kind 分布 / 完整性 / 目标库）、
`GET api/conversations`（会话树）、`GET api/conversation?id=N`（单条会话的 entry / task /
document / submission / prompt）、`GET api/cost`（按日 / 时 / 模型 / 会话的消耗），
`GET api/health`（任务 / 提交 / 文档 / WAL / freelist / 维护计划）。字段形状无测试背书，
暂不写在这里；真正用到时先补测试再把指针换上。

几条别忘：

- **`mtime` / `walMtime` 是 epoch 毫秒**（不是秒）。乘 1000 会得到 5 万年后的时间。
- **`file.db`（文件字节）和 `pageSize × pageCount` 不总相等**：WAL 模式下 db 文件长度会落后几页。
  真库上差 2 页，别断言相等。
- 每份响应都带一段 `diagrams`（mermaid 源码）。桌面端不渲 mermaid，用 `Value` / `BTreeMap`
  收着透传，别删字段。
- 面板崩的时候回的是**整页 HTML**（502 网关那种）。错误文案必须先压平去标签再上屏，
  否则一个红框能把整页顶下去（`console::flatten`）。
- 这些端点是**只读**的：没有 POST。归档 / 打捞 / 撕原件仍然走引擎 `/v1/*`。
