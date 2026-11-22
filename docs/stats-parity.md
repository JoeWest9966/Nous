# 数据监控室复原对照表（Desk ↔ 内核 stats）

> 这张表是 **Desk 数据监控室的验收基准**：TS 面板时代（`d6s4v1` + `mcfpqm`）Desk 那六屏
> 消费的每一个字段，现在 Rust 内核 `/v1/*` 到底给不给、给全不给全。
>
> 目标不是把面板的 5 个 API 原样搬回来，而是**让 Desk 的渲染层重新拿到当年那么全的数据**。
> Desk 的渲染层（`screen_*` 九个函数）几乎没删，掏空的是数据。所以「复原」= 把这些字段的
> 数据源在内核里补齐，Desk 重新接线。
>
> 图（mermaid：treemap / gitGraph / sankey / kanban …）**不在本表内**：Desk 当年就没渲过图，
> 只把源码收着。复原 Desk ≠ 画图。要画图是另一张票。

## 现状一句话

Rust 内核只有 3 条 stats 端点（`usage` / `tasks` / `db`），全是**汇总级**的；
TS 面板的 5 个 API 是**明细级**的（每条 entry 的形状、每个任务起止、每张表真页数、
prompt 重建）。Desk 现在把明细屏用汇总端点硬拼，拼不出来的地方全填 0 / 空 / 灰理由。

技术前提已实测（远程开发机）：`dbstat` 虚表在 `rusqlite 0.31 bundled` 下**可用**，
返回真页数。所以「解剖」屏最吃页数的那块没有技术坎。

---

## 屏 1 · 解剖（overview）

TS 源：`GET api/overview` → 现在的源拼：`/v1/device` + `/v1/capabilities` + `/v1/stats/db`

| 字段（Desk 渲染层消费） | TS 面板 | 现在内核 | 状态 |
|---|---|---|---|
| `file.db` 字节 | `stat(size)` | `/v1/stats/db.durable.bytes` | ✅ 有 |
| `file.wal` 字节 | `stat(wal)` | — | ❌ 填 0 |
| `file.shm` 字节 | `stat(shm)` | — | ❌ 填 0 |
| `file.mtime` / `walMtime` | `stat.mtimeMs` | — | ❌ 填 0 |
| `pragmas.pageSize` | `pragma page_size` | — | ❌ 填 0 |
| `pragmas.pageCount` | `pragma page_count` | — | ❌ 填 1（假） |
| `pragmas.freelist` | `pragma freelist_count` | — | ❌ 填 0 |
| `pragmas.journalMode` | `pragma journal_mode` | — | ❌ 填 "" |
| `pragmas.schemaVersion` | `durable_schema.version` | `/v1/stats/db.durable.schema` | ⚠️ 有值但没接进 pragmas |
| `bytes.free` | `freelist × pageSize` | — | ❌ 填 0 |
| `bytes.used` | `total - free` | — | ❌ 用整库字节充数 |
| `bytes.onDisk` | `db+wal+shm` | — | ❌ 用 db+plugins 充数 |
| `counts.<表>` 行数 | `count(*)` 每表 | `/v1/stats/db.durable.rows` | ✅ 有 |
| `tables[].bytes` 真字节 | `dbstat`（真页数） | — | ❌ 每表都填整库字节（错） |
| `tables[].pages` | `dbstat` | — | ❌ 填 0 |
| `tables[].rows` | `count(*)` | `rows` | ✅ 有 |
| `indexes[]`（字节/页/表） | `dbstat` + `sqlite_master` | — | ❌ 空 |
| `entriesByKind[]`（kind/条数/字节） | `json_extract(record,'$.kind')` 分组 | — | ❌ 空 |
| `integrity.checked/ok/messages` | `pragma integrity_check` | `quickCheck`（quick_check，更弱） | ⚠️ 语义不同，需补真 `integrity_check` |
| `attention[]`（WAL 大/空闲页多/schema 旧…） | 面板算的提示 | — | ❌ 空 |

**屏 1 缺口**：`dbstat`（表/索字节+页）、`pragma`（page_size/count/freelist/journal）、
WAL/SHM 文件大小、`entriesByKind`、真 `integrity_check`。都需要新端点或扩 `/v1/stats/db`。

---

## 屏 2 · 会话树（conversations）

TS 源：`GET api/conversations` → 现在的源拼：`/v1/sessions`

| 字段（Desk 渲染层消费） | TS 面板 | 现在内核 | 状态 |
|---|---|---|---|
| `count` | 行数 | `items.len()` | ✅ 有 |
| `list[].id` | 列 | `id` | ✅ 有 |
| `list[].title` | 首条 user 文本 | `name` | ⚠️ 语义不同（name vs 首条正文），够用 |
| `list[].ownerConv` / `ownerTask` | 列 | `parent` / `owner` | ⚠️ 有邻接，但结构不同（`parent{id,at}` / `owner{conversationId,taskId}`） |
| `list[].entries` 条数 | 按会话数 entries | — | ❌ 填 0 |
| `list[].tokens` | 每会话 token 合计 | — | ❌ 填 0 |
| `list[].toolCalls` | 工具调用次数 | — | ❌ 填 0 |
| `list[].models` | 用过的模型去重 | `model`（单个） | ⚠️ 只有一个，不是「用过的全部」 |
| `list[].firstAt` | 首条时间 | — | ❌ 填 0 |
| `list[].lastAt` | 末条时间 | `updatedAt` | ✅ 近似 |
| `list[].cost` | 每会话成本 | — | ❌ 无（Desk 结构里也没列，面板有） |
| `totals.entries` / `totals.tokens` | 聚合 | — | ❌ 空（拉片「convs」读数吃它） |

**屏 2 缺口**：每会话的 entries 条数、tokens、toolCalls、firstAt、totals。
需要按会话聚合 entry 的端点（现在 `usage?group_by=conversation` 只给 tokens/cost，
没有 entries 条数、toolCalls、firstAt）。

---

## 屏 3 · 回放（replay）

TS 源：`GET api/conversation?id=` + `api/prompt` → 现在的源拼：`/v1/sessions/:id/entries`

| 字段（Desk 渲染层消费） | TS 面板 | 现在内核 | 状态 |
|---|---|---|---|
| `entries[].seq` | `commit_seq` | — （用 enumerate 序号） | ⚠️ 用数组下标冒充，非真 commit_seq |
| `entries[].kind` | record.kind | `kind` | ✅ 有 |
| `entries[].at` | `model.timestamp` | `model[].timestamp`（entries 里带 model） | ✅ 有 |
| `entries[].tokens` | `usage.totalTokens` | `model[].usage.totalTokens` | ✅ 有 |
| `entries[].text` | record 正文 | `text` | ✅ 有 |
| `entries[].bytes` | record 长度 | — （用 model 序列化长度估） | ⚠️ 近似 |
| `entries[].is_error` | `stopReason=="error"` | `model[].stopReason` | ✅ 有 |
| `entries[].tool` | `model.toolName` | — （entries 顶层无 tool） | ❌ 无（需从 model 取） |
| `entries[].stopReason` | 有 | `model[].stopReason` | ✅ 有 |
| `tasks[]`（id/kind/status/startedAt/endedAt/ms） | `tasks` 表按会话 | — | ❌ 空（回放屏渲染「这条会话的任务」） |
| `documents[]`（id/kind/key/revisions/bytes/retired） | `documents`+`document_revisions` | — | ❌ 空（回放屏渲染 docs 计数） |
| `prompt.messages[]`（label/kind/bytes/chars/tokens/cost/head/text） | `promptAt` 重建 | `/v1/sessions/:id/entries?system=1` 只给「当前生效 system 文本」 | ❌ 空（回放屏渲染「prompt 条」） |

**屏 3 缺口**：会话内 tasks、documents、**prompt 重建整条消息流**。这是三块独立的子查询。
prompt 重建是面板 `promptAt()` 的核心价值（「此刻模型看到了什么」），现在内核只有 system 文本。

---

## 屏 4 · 账单（cost）

TS 源：`GET api/cost` → 现在的源拼：`/v1/stats/usage?group_by=day|model|conversation`（各拉一次）

| 字段（Desk 渲染层消费） | TS 面板 | 现在内核 | 状态 |
|---|---|---|---|
| `totals.tokens` | 合计 token | `usage.totals.totalTokens` | ✅ 有 |
| `totals.cost` | 合计成本 | `usage.totals.costTotal` | ✅ 有 |
| `totals.entries`（调用次数） | assistant entry 数 | `usage.totals.requests` | ✅ 有 |
| `totals.input` / `output` | usage 分项合计 | — | ❌ 无（拉片 cache 命中率吃它） |
| `totals.cacheRead` / `cacheWrite` | 缓存分项 | — | ❌ 无 |
| `totals.reasoning` | reasoning token | — | ❌ 无 |
| `byDay[]` | 按天桶 | `usage?group_by=day` | ✅ 有（但桶里只有 tokens/cost/calls，无 input/output/cacheRead） |
| `byHour[]` | 按小时桶 | — | ❌ 无（面板在「不足两天」时退到小时） |
| `byModel[]` | 按模型桶 | `usage?group_by=model` | ✅ 有 |
| `byConv[]` | 按会话桶 | `usage?group_by=conversation` | ✅ 有 |
| `note`（cost=0 不代表免费） | 面板写死一句 | — | ❌ 空 |

**屏 4 缺口**：`input`/`output`/`cacheRead`/`cacheWrite`/`reasoning` 五个分项（totals 和每桶都要）、
`byHour` 小时桶、`note`。现在桶里只有 `{tokens, cost, calls}`，面板是
`{tokens, cost, calls, input, output, cacheRead}`。

---

## 屏 5 · 维修（health）

TS 源：`GET api/health` → 现在的源拼：`/v1/stats/tasks` + `/v1/device`

| 字段（Desk 渲染层消费） | TS 面板 | 现在内核 | 状态 |
|---|---|---|---|
| `tasks.total` | tasks 行数 | `byState` 求和 | ✅ 有 |
| `tasks.byStatus[]`（status/count/sample） | 按状态分组 | `byState` | ⚠️ 有 count，无 sample |
| `tasks.stuck[]`（超 10min 未终态） | 面板算 | — | ❌ 空（拉片「卡住」读数吃它） |
| `tasks.sleeping[]` | sleeping 任务 | — | ❌ 空 |
| `tasks.avgMs` / `p95` | 按 record 起止算 | — | ❌ 填 0 |
| `tasks.slowest[]`（最慢 6 个） | 面板排 | — | ❌ 空（回放屏「任务」也吃它） |
| `submissions.total` / `byStatus[]` | submissions 表 | — | ❌ 空（拉片「待答输入」吃它） |
| `submissions.unanswered[]` | status==unanswered | — | ❌ 空 |
| `documents.documents` / `revisions` / `base` / `delta` / `deltaRatio` | `document_revisions` 分组 | — | ❌ 空（拉片「文档 REVISION」吃它） |
| `walBytes` | `stat(wal)` | — | ❌ 填 0 |
| `freelist.free` / `.bytes` | pragma | — | ❌ 填 0 |
| `maintenance[]`（维修工单：备份/checkpoint/VACUUM…） | 面板写好的命令清单 | — | ❌ 只剩一条「连接/版本」占位 |

**屏 5 缺口**：任务明细（stuck/sleeping/avgMs/p95/slowest）、submissions 全量、
documents revision 分组、WAL 字节、freelist、维修工单清单。

---

## 屏 6 · 归档（archive）

TS 源：`GET mcfpqm` 面板块 → 现在的源：`/v1/archive`（Nous#67 已落地）

| 字段 | 现在内核 | 状态 |
|---|---|---|
| 世代列表 / 墓碑 / live / parent / last / 四态 | `/v1/archive` 全有 | ✅ **已比 TS 更真**（当年走旧面板） |

归档屏**基本不用动**，是六屏里唯一数据反而更全的。

---

## 拉片十个读数（左列）

| 读数 | 吃什么 | 状态 |
|---|---|---|
| 库文件 `db` | `overview.file.db` + `pragmas` | ⚠️ 页数/页大小假（→屏 1） |
| WAL `wal` | `overview.file.wal` | ❌ 0（→屏 1） |
| ENTRIES | `overview.counts.entries` | ✅ 有 |
| 会话 `convs` | `overview.counts` + `convs.totals` | ⚠️ totals 空（→屏 2） |
| TOKENS | `cost.totals` | ✅ token 有（→屏 4 分项缺） |
| 缓存命中 `cache` | `cost.totals.input/cacheRead` | ❌ 无（→屏 4） |
| 归档 | `/v1/archive` | ✅ 有 |
| 待答输入 `waiting` | `health.submissions.unanswered` | ❌ 空（→屏 5） |
| 卡住的任务 `stuck` | `health.tasks.stuck` | ❌ 空（→屏 5） |
| 文档 REVISION `docs` | `health.documents.revisions` | ❌ 空（→屏 5） |

**当前实际表现**：拉片 10 个读数里 5 个显示 `—`（还没拿到），因为对应数据源根本不存在。

---

## 时间线（右列）

来源四类：归档 ✅（`/v1/archive` 有）· 会话 ⚠️（缺 tokens/entries）· 任务 ❌（缺 slowest）· 账本 ⚠️（缺 input/output）。
**当前实际表现**：只有「归档」和「会话（部分）」两类事件，任务/账本两类几乎空。

---

## 结论：缺口的形状

按「要补哪类数据」，缺口聚成 4 组，不是 6 屏 6 组：

1. **SQLite 内省**（屏 1）：`dbstat` + `pragma` + WAL/SHM stat + `entriesByKind` + 真 `integrity_check`。
2. **按会话明细聚合**（屏 2/3）：每会话 entries 条数/tokens/toolCalls/firstAt；会话内 tasks；会话内 documents；
   **prompt 重建**。
3. **用量分项**（屏 4）：`input/output/cacheRead/cacheWrite/reasoning` + `byHour`。
4. **健康明细**（屏 5）：stuck/sleeping/avgMs/p95/slowest、submissions 全量、revisions base/delta、WAL、freelist、维修工单。

Desk 侧：`load_console` / `assemble_*` 重新接线，渲染层基本不动。

## 落地地图（两边）

- **内核**（`/opt/Nous-src-stats-parity`，`feat/stats-parity`）：扩 `stats` 插件（`docs/plugins.md` 已归口「监控六屏」）。
  新增端点建议——`/v1/stats/overview`（屏 1）、`/v1/stats/conversations`（屏 2）、
  `/v1/stats/conversation/:id`（屏 3，含 prompt）、`/v1/stats/cost`（屏 4）、`/v1/stats/health`（屏 5）。
  `wire.md` 同步登记。
- **Desk**（`/opt/NousDesk-dataconsole-parity`，`feat/dataconsole-parity`）：`assemble_*` 改吃新端点，
  删掉占位零，`screen_*` 不动。`capability.rs` 里 `Feature::Stats` 的门保持不变。

## 已知边界

- **图不在此列**：mermaid 渲染是另一张票，Desk 当年也没画。复原数据 ≠ 复原图。
- **`dbstat` 已实测可用**（rusqlite bundled，远程机 spike 通过），不是风险。
- `cost` 全 0 是正常：provider 不回 price 时 pi-durable 记 0，`tokens` 才是真数（面板原话，别当 bug）。
