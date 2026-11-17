# fixture 元数据出处图（`fixtures/agent.sqlite` → `GET /v1/sessions`）

**这份文档只做调研，不改代码。** 对齐目标是本库 `wire.md` 的 `GET /v1/sessions`
（仓库根 `wire.md:29-57`，本库权威版本），**不是** `qrd3wh` 的 `server.js`，也不是
Nous 仓里的镜像。上游检出只读：`/tmp/pi-check` = `pi@v1.5.2`（`git describe` =
`v1.5.2`），`/tmp/pi-rust` = `pi-rust@v0.24.2`；没有改它们，没有把上游代码抄进本库。

## 0. 读库方法与库情

```python
import sqlite3
c = sqlite3.connect("file:fixtures/agent.sqlite?mode=ro", uri=True)
```

Python `sqlite3` 是这个环境的唯一入口（没有 `sqlite3` CLI）。全文所有样本都从这份
fixture 真读出来。文件 1,179,648 B；`pragma journal_mode = wal`，
`pragma schema_version = 33`，`pragma page_size = 4096`，`pragma page_count = 288`。
行数：`conversations` 189 / `entries` 209 / `documents` 961 /
`document_revisions` 969 / `tasks` 128 / `submissions` 56 /
`record_ids` 1609 / `durable_metadata` 1 / `durable_schema` 1。
`select version from durable_schema` = **2**，与上游
`packages/durable/src/storage/sqlite/migrations.ts:110-112` 的
`CURRENT_SQLITE_SCHEMA_VERSION`（两份迁移）一致；fixture 是 pi-durable 的真实库，
不是本库自己写的库（本库表结构由 `docs/storage.md` 定）。

**关键结构事实**：`conversations.record` 181/189 只有 `{"id":N}`，8/189 带
`{"id":N,"parent":{"conversationId":M,"at":E}}`（详见 §3）。会话级元数据**不在**
`conversations` 表，而是分两处：

1. 每个会话的五个 durable 文档（`pi.agent` / `pi.inbox` / `pi.live` /
   `pi.provider` / `pi.usage`，各 192 条，`scope_kind='conversation'`），
   内容在 `document_revisions.content`。`documents.record` 自身只有
   `{id,kind,scope,history,fork,createdAt}`（`documents.created_at` 是**提交序号 seq**，
   不是墙上时间；上游 `packages/durable/src/types.ts:769-779`）。
2. 一份 **`qrd3wh.sessions`** 文档（`documents.id = 8`，`scope_kind='session'`，
   `owner_id=0`，全库唯一），它的 base revision
   （`document_revisions.seq = 5072`, `kind='base'`, `version=1`）的 `content` 顶层是
   `{"names":{...},"machines":{...},"archived":{...}}` —— **名字、机器、归档墓碑
   都在这一份 JSON 里**。这是本调研最重要的发现：issue 描述里说「`qrd3wh.sessions`
   一条」但没展开它的内容，而三个看似无出处的字段正好都在里面。

`document_revisions` 969 条**全部** `kind='base'`（无 delta）；970 条按 document 分组后
只有 3 个文档有多条 revision：`document_id=7`（conv 2 的 `pi.agent`，7 条）、
`495`（2 条）、`1550`（2 条）。取生效值统一按
`max(seq)`（文档语义见上游 `packages/durable/src/storage/store/documents.ts:105-107`
的 `aliveAt`，以及 `document_revisions` 主键 `(document_id, seq)`）。

## 1. 主表：字段 × 出处 × 取值规则 × 缺失时行为

下表的「列」均取自本库 `pragma table_info(<表>)` 的真实列名，见 §4 的逐列清单。
`record` 列的 JSON 路径用 `$.a.b` 表示；`max(seq)` 指该文档下 `seq` 最大的那条
`document_revisions.content`。

| 字段 | 表 / 列 | JSON 路径 / 推导 | 本 fixture 真值情况 | 缺失时行为 |
|---|---|---|---|---|
| `id` | `conversations.id`（INTEGER PK） | 直接取；与 `conversations.record.$.id` 相同（189/189 相等） | 189 个数字 id：2, 14, 25, …, 1694 | 表里有行才有这一项；不做字符串化（`wire.md:23`） |
| `name` | `documents`（`kind` 列值 `"qrd3wh.sessions"`，`scope_kind='session'`）→ `document_revisions.$.names["<id>"]`，取 `max(seq)` 的 base | `names` 是一个以**会话 id 字符串为键**的 map；189/189 活会话都有键（另 18 个键属于已不在 `conversations` 的会话） | 真实名字例如 `"2": "真模型"`、`"44": "对话 44"`、`"699": "zz-探针（可删）· 归档往返"` | **本库不生成默认名**。没有键时给 `""`（空串），不编「对话 N」——理由见 §2 |
| `parent` | `conversations.record` | `$.parent` → `{id: $.parent.conversationId, at: $.parent.at}`；`at` 是**父会话里的 entry id**（不是时间戳），`wire.md` 的 `{id,at}` 与之同形 | 8/189 非空：如 44 的 `{"conversationId":14,"at":24}` | 无 `parent` 键或 `null` → 给 `null`。`conversations.owner_conversation_id` **全为 NULL**，不要拿它当 fork |
| `owner` | `conversations.record` | `$.owner?.conversationId` / `$.owner?.taskId`（上游 `types.ts:437-440`）；若只要一个标量，取 `owner?.conversationId` | 189/189 没有 `owner` 键；`conversations.owner_conversation_id` / `owner_task_id` 也全为 NULL | **给 `null` + 理由**：这一版 durable 建会话时是 `ownerless`（上游 `types.ts:426`），库里没有属主可报。不要把 `qrd3wh.sessions.machines` 当 owner |
| `model` | `documents`（`kind` 列值 `"pi.agent"`, `scope_kind='conversation'`, `owner_id=<id>`）→ `document_revisions.content`，取 `max(seq)` | `$.model.provider + "/" + $.model.modelId`；192/192 个 `pi.agent` 都有 `model` | 191 个是 `"volc-ark/deepseek-v4-flash-260425"`，会话 1639/1645/1662/1673/1694 是 `"Qoder/Fast"`（从 agent revision 真读） | 没有 `model` 键 → 给 `null`（fixture 内 0 例；上游 `AgentState.model?` 本就是可选，`harness/types.ts:464-465`）。不要猜测默认模型 |
| `thinkingLevel` | 同上（`pi.agent`） | `max(seq)` base 的 `$.thinkingLevel` | 26/192 的 agent 最新 base 有该键，值为 `"off"`；166/192 没有 | 键不存在 → 给 `null`（wire 没规定默认值；上游 `AgentState` 只在 `Agent` 解析层给默认值，`harness/types.ts:464-465` 与 `:526`，库里没有的不许补） |
| `thinkingLevels` | **无真值** | 上游存在 `GET /v1/models` 的模型能力表（`wire.md:270-278`），本 fixture 没有模型目录表 | 所有会话都没有；`pi.agent` 只记选中的 level，不记可选集合 | **给 `[]` + 理由**：这是模型能力，不是会话状态；fixture 里没有能力表，编一个会跟真实引擎不一致。`wire.md:55` 明确它是「生效值」 |
| `skills` | `pi.agent` → `document_revisions.content` | `$.extensions.add` 中 `"skill:<name>"` 前缀的项，去掉 `skill:`；按出现顺序 | 32 个 `skill:` 值（如 `skill:implement`、`skill:deep-dive`、`skill:code-review`），分布在若干会话 | 没有 `extensions` / 没有 `skill:` 项 → 给 `[]`（空集合是「没挂技能」，不是未知）。注意 `extensions` 在 durable 里是核心扩展名 + 技能/提示词的混合列表，本库只认 `skill:` 前缀 |
| `template` | `pi.agent` → `document_revisions.content` | `$.extensions.add` 中 `"prompt:<name>"` 前缀的项，去掉 `prompt:`；**单值**（wire 里模板是单值，多个时取第一个） | 37 个 `prompt:smart-ass`；无 `prompt:` 的会话占多数 | 没有 `prompt:` 项 → `null`（明确不要模板）。不要把 core 扩展名 `prompts` 当模板（库里它出现 3 次，是 qrd3wh 核心扩展 id，不是模板名） |
| `readOnly` | **无真值** | 库里没有 `readOnly` / `readonly` 列或 JSON 键；`documents.kind` 的五种 durable 文档、qrd 的 `names/machines/archived` 都没有这个维度 | 0 处 | **给 `false` + 理由**：`wire.md` 把它声明为必需 bool，fixture 里的会话全部可写（有 tasks/submissions/entries 的真实写入）。不能给 `null`，否则破坏 wire 形状 |
| `busy` | `pi.live` → `document_revisions.content`；`tasks.status` | `pi.live` 的 `$.generation` / `$.tools` 存在 → `"running"`；有 `pi.inbox` 项 → `"queued"`；`tasks.status != 'terminal'` → 该状态 kind；否则 `null` | 192/192 `pi.live` base 都是 `{}`；128/128 tasks 全 `terminal`；56 submissions 为 done/unanswered → 所有会话都空闲 | 无活任务、无 live 状态 → `null`（`wire.md:24` 明确 null 合法）。`busy` 不是布尔，不要输出 `false` |
| `updatedAt` | `tasks.record` → `$.endedAt`（列 `tasks.conversation_id` 过滤）；候选 `entries.record` → `$.model[*].timestamp` | **取该会话所有 terminal task 的 `max(endedAt)`，epoch 毫秒**；没有 task 时再退到 `max(entry timestamp)`。**不用** `documents.created_at` / `document_revisions.seq`（那是提交序号，不是墙上时间），不用文件 mtime | 17/189 会话有 `endedAt`（全部同时也有 entry timestamp，且 `endedAt > entry.ts` 17/17）；另 172/189 两者都没有 → `null`。真值例：会话 2 = 1794222429706，会话 44 = 1793173184104 | **给 `null`**，合法且是多数：`wire.md:47-51` 说「引擎只记自己看着它写的那些，重启前的老会话就是空的」，「别拿 `last.id` 假装时间，也别拿文件 mtime 冒充」。详见 §5 |
| `last` | `entries`（`id`, `conversation_id`, `record`） | 对会话取 `max(id)` 的 entry：`{id: entries.id, kind: record.$.kind, text: text(record.$.model)}`；`text()` = 拼接 `$.model[*].content`：字符串直接取，数组取 `type='text'` 的 `text`，跳过 `thinking`，`toolCall` 渲染成 `[toolCall <name>]`（预览语义，上游消息块见 `packages/ai/src/types.ts:430-434, 595`） | 17/189 有 entry（209 条 entry 全带 `timestamp`）：如会话 44 的 57 / `pi.assistant` / `"今天是星期三。"`；会话 2 的 698 / `pi.assistant` / `""`（最后一条只有 thinking/toolCall） | 会话没有 entry → `null`（`wire.md:54`）。`text` 为空串**也是真值**，不要因为空串就丢掉整个 `last` |
| `archived` | `qrd3wh.sessions` → `document_revisions.$.archived["<id>"]`，取 `max(seq)` base | map 值为 `{at, gen, purged}` 时直接输出；`at` 已是 epoch 毫秒，`gen` 是 `YYYYMMDD`，`purged` 是 bool | 46 条墓碑：15 条 `purged=true`（对应会话都已不在 `conversations`）、31 条 `false`（对应会话都仍在） | key 不存在 → `null`（没归档过 / 捞回之后墓碑被抹掉）。**不要**用 `documents.retired_at` 代替：fixture 中 961/961 都是 NULL，那是 document incarnation 换代，不是会话归档。详见 §3 |

主表里每个字段的取值都只依赖上表的列 / JSON 路径；**下一张票（nous-fixture 落地）
照着这份文档能直接写代码**——按 `conversations.id` 逐行，关联
`qrd3wh.sessions` 的 base JSON、关联 `pi.agent` 的最新 base、聚合 tasks 与 entries
即可，不需要猜测任何字段。确实无从推导的字段（`owner` / `thinkingLevels` /
`readOnly` / 无 entry 时的 `last`）已逐条写明给 `null` / `[]` / `false` 和原因。

## 2. `name` 回退决定

**库里有名字**——只是不在 `conversations.record`，而在 `qrd3wh.sessions` 这份
session-scope 文档的 base content 的 `names` map 里（键是会话 id 的字符串）。
189/189 活会话都有键：

```
id 2   -> "真模型"
id 14  -> "经宿主真模型"
id 44  -> "对话 44"
id 699 -> "zz-探针（可删）· 归档往返"   （已 purge，但名字仍在 map 里）
```

其中 18 条名字恰好是引擎的默认格式「对话 N」（`对话 44`、`对话 296`、
`对话 1639`…），说明**默认名是引擎在改名/建会话时就已经写进 `names` 的**，
不是列表时现拼的。`wire.md:160-162` 也只说建会话时 name 可省则引擎给「对话 N」，
这条语义落在引擎侧，不落在 durable 的 `conversations.record`。

**决定：本库不生成默认名。** 理由：

1. `wire.md` 的 `GET /v1/sessions` 把 `name` 列为依赖字段，客户端拿到的应该是
   **引擎的生效值**；本库是隔离实现，没有引擎的建会话/改名代码，也没有
   `POST /v1/sessions/:id/rename` 的时间语义（`wire.md:166-172` 明说回包才是生效值）。
   自己拼「对话 N」会造出一个引擎从未存过的名字，等于伪造事实源。
2. fixture 证明活会话 `names` 覆盖率 100%，正常路径下根本用不上回退。
3. 真缺键时给 `""`（空串，保持 `wire.md` 的 string 形状），不编中文默认名；
   可选地在 `nous-fixture` 的 README 里写一句「空串 = 上游没有存名字」。
   若将来产品明确要复现引擎默认名，那是一次**改契约**（先改 `wire.md`），
   不是 fixture 阅读器可以偷偷做的事。

## 3. `archived` / `parent` 的痕迹

### `archived {at, gen, purged}`：有，且是完整真值

唯一痕迹就是 `qrd3wh.sessions` base content 的 `archived` map（46 个键）：

```
"699": {"at": 1794569866329, "gen": "20261113", "purged": true}
"735": {"at": 1794570325133, "gen": "20261113", "purged": false}
```

`at` 是 epoch 毫秒，`gen` 是 `YYYYMMDD`（对应 `wire.md:174-196` 的
`gen-YYYYMMDD.sqlite` 复印件），`purged` 区分归档与撕原件。46 条墓碑里：
31 条 `purged=false` 的会话**仍在 `conversations`**（如 735、1639、1645、1694），
15 条 `purged=true` 的会话**已不在**（如 514、675、699、711、717、1563、1575、…、1683）。
这正是宪法里四态（`wire.md:186-196`）的数据底色：`archived.purged` 要先判，
`live && archived` 才是「原件在」。

**不是** `documents.retired_at`：fixture 中 961 行 `retired_at` 全 NULL，
它表达的是 document incarnation 的半开生命周期（上游 `types.ts:769-779`、
`storage/store/documents.ts:105-107`），与会话归档无关。`archived` 也不在
`conversations.record`（189/189 只有 `id` / 至多再加 `parent`）。

### `parent {id, at}`：有，8 条 fork

`conversations.record` 里 8/189 带 `$.parent`（上游字段定义
`packages/durable/src/types.ts:429-435`，`at` 是 `EntryId`，含继承历史的那条 entry）：

```
44  -> {"conversationId":14,  "at":24}
296 -> {"conversationId":44,  "at":50}
302 -> {"conversationId":296, "at":50}
331 -> {"conversationId":14,  "at":124}
337 -> {"conversationId":331, "at":20}
1639 -> {"conversationId":1624, "at":1638}
1645 -> {"conversationId":1624, "at":1638}
1694 -> {"conversationId":1624, "at":1638}
```

映射到 wire 就是 `{id: <conversationId>, at: <at>}`，`at` 是 **entry id 不是时间**；
8 条之外给 `null`。旁证：`conversations_by_owner_conversation` 索引对应的
`conversations.owner_conversation_id` 列 189/189 全是 NULL——fork 关系不在这两列，
别拿它们推 parent。`pi.agent` 文档是 `rewindable` / `fork: asOf`
（上游 `harness/agent.ts:45-53`），所以 fork 会话的 agent 状态从分叉点复制，
但列表字段只需要 `conversations.record` 这一处。

## 4. `pragma table_info` 逐列证据（真实返回）

以下列名逐字来自 `pragma table_info(<表>)`（格式：列名 / 类型 / notnull / pk）：

- `conversations`: `id` INTEGER/0/pk1, `owner_conversation_id` INTEGER/0/0,
  `owner_task_id` INTEGER/0/0, `record` TEXT/1/0。
- `entries`: `id` INTEGER/0/pk1, `conversation_id` INTEGER/1/0,
  `head` INTEGER/0/0, `commit_seq` INTEGER/1/0, `record` TEXT/1/0。
- `documents`: `id` INTEGER/0/pk1, `kind` TEXT/1/0, `family` INTEGER/1/0,
  `key_value` TEXT/1/0, `scope_kind` TEXT/1/0, `owner_id` INTEGER/1/0,
  `created_at` INTEGER/1/0, `retired_at` INTEGER/0/0, `record` TEXT/1/0。
- `document_revisions`: `document_id` INTEGER/1/pk1, `seq` INTEGER/1/pk2,
  `kind` TEXT/1/0（CHECK in ('base','delta')）, `version` INTEGER/1/0,
  `content` TEXT/1/0。
- `tasks`: `id` INTEGER/0/pk1, `conversation_id` INTEGER/1/0, `kind` TEXT/1/0,
  `status` TEXT/1/0, `abort_requested` INTEGER/1/0, `background` INTEGER/1/0,
  `record` TEXT/1/0。
- `submissions`: `id` INTEGER/0/pk1, `conversation_id` INTEGER/1/0,
  `request_id` TEXT/0/0, `status` TEXT/1/0, `record` TEXT/1/0。

对应上游 DDL：`packages/durable/src/storage/sqlite/migrations.ts:25-30`
（conversations 与两个 owner 列）、`:33-39`（entries）、`:66-76`（documents，
含 `retired_at`）、`:81-88`（document_revisions）、`:42-54`（tasks）。
上游记录类型：`ConversationRecord`（`types.ts:429-441`，含 parent/owner）、
`EntryRecord`（`types.ts:460-471`，`kind` / `model` / `data`）、
`DocumentRecord`（`types.ts:769-805`，`createdAt` / `retiredAt` 与 scope）、
`TaskRecord` 的 `startedAt` / `endedAt`（`types.ts:734-741`，明确是
"Wall-clock milliseconds … stamped by the Session"）。

## 5. `updatedAt` 的决定

**取 `tasks.record.$.endedAt` 的最大值（毫秒），不是 entry timestamp，也不是 seq。**

证据链：

- 上游 `packages/durable/src/types.ts:740-741` 明写 `endedAt` 是"Wall-clock
  milliseconds of the change to `terminal`, stamped by the Session"；
  `entries` 的 `timestamp` 在 `packages/ai/src/types.ts:590, 616-617`（用户消息 /
  请求开始时刻），是**消息**时刻而不是「会话最后一次落库」。
- fixture 中 209/209 条 entry 都带 `timestamp`，但只覆盖 17/189 会话；
  `tasks.endedAt` 也只覆盖同样 17 个会话。在这 17 个会话上逐条比对，
  `max(endedAt) > max(entry.timestamp)` **17/17**（没有一例反向或相等），
  因为生成任务在最后一条 assistant entry 之后才转 terminal。例：会话 2 的最后 entry
  是 698，最后 generation task 的 `endedAt = 1794222429706`。
- `documents.created_at` / `document_revisions.seq`（最大到 5072）是**提交序号**
  （上游 `types.ts:776-777`：`createdAt: Seq`），乘 1000 或当毫秒用会得到
  1970 年附近的错误时间；文件 mtime 也不可用（`wire.md:47-51` 明确禁止）。
- **172/189 会话 `updatedAt = null`**，合法且是常态：`wire.md:47-51` 说引擎只记
  「自己看着它写的那些」，重启前的老会话就是空的；「显示相对年龄就用它，
  别用 `last.id` 假装时间，也别拿文件 mtime 冒充」。fixture 中 172 个老会话
  既没有 task 也没有 entry，正好是这个语义。真没时间源时就给 `null`，
  显示层应显示空/未知，而不是 1970。

## 6. 真值样本（从 fixture 实读，非编造）

以下每行都由脚本从 `fixtures/agent.sqlite` 读出（只读 URI）；`updatedAt` 取
`max(tasks.record.endedAt)`，`model` 取 `pi.agent` 最新 base，`name` / `archived`
取 `qrd3wh.sessions` 最新 base。`thinkingLevels` / `skills` / `template` /
`readOnly` / `busy` 的「值」按主表的缺失规则填。

```json
{"id":2,"name":"真模型","parent":null,"owner":null,
 "model":"volc-ark/deepseek-v4-flash-260425","thinkingLevel":"off",
 "thinkingLevels":[],"skills":[],"template":null,"readOnly":false,"busy":null,
 "updatedAt":1794222429706,
 "last":{"id":698,"kind":"pi.assistant","text":""},
 "archived":null}
```

```json
{"id":44,"name":"对话 44","parent":{"id":14,"at":24},"owner":null,
 "model":"volc-ark/deepseek-v4-flash-260425","thinkingLevel":null,
 "thinkingLevels":[],"skills":[],"template":null,"readOnly":false,"busy":null,
 "updatedAt":1793173184104,
 "last":{"id":57,"kind":"pi.assistant","text":"今天是星期三。"},
 "archived":null}
```

```json
{"id":296,"name":"对话 296","parent":{"id":44,"at":50},"owner":null,
 "model":"volc-ark/deepseek-v4-flash-260425","thinkingLevel":null,
 "thinkingLevels":[],"skills":[],"template":null,"readOnly":false,"busy":null,
 "updatedAt":null,"last":null,"archived":null}
```

```json
{"id":735,"name":"scratch 11-13 19:44","parent":null,"owner":null,
 "model":"volc-ark/deepseek-v4-flash-260425","thinkingLevel":null,
 "thinkingLevels":[],"skills":[],"template":null,"readOnly":false,"busy":null,
 "updatedAt":null,"last":null,
 "archived":{"at":1794570325133,"gen":"20261113","purged":false}}
```

```json
{"id":1639,"name":"对话 1639","parent":{"id":1624,"at":1638},"owner":null,
 "model":"Qoder/Fast","thinkingLevel":"off",
 "thinkingLevels":[],"skills":[],"template":null,"readOnly":false,"busy":null,
 "updatedAt":null,"last":null,
 "archived":{"at":1794826245070,"gen":"20261116","purged":false}}
```

补充验证样本（用于证明 `skills` / `template` 不是凭空字段）：会话 490 的 `pi.agent`
最新 base（`documents.id=495`，`document_revisions.seq=1617`）含
`"extensions":{"add":["skill:code-review","prompt:smart-ass","nous"]}`，按主表规则得到 `skills=["code-review"]`、
`template="smart-ass"`；`"nous"` 是核心扩展 id，不映射到任何 wire 字段。

## 7. 无真值字段清单（给 null / 常量 + 原因）

| 字段 | 给什么 | 为什么 |
|---|---|---|
| `owner` | `null` | `conversations.record.$.owner` 189/189 缺失，两个 owner 列全 NULL；上游 `ConversationOwnership` 允许 `ownerless`（`types.ts:426`） |
| `thinkingLevels` | `[]` | 模型能力集合不在 fixture 里；它是模型目录属性（`wire.md` 的 `/v1/models`），不是会话状态 |
| `readOnly` | `false` | 库里没有任何只读标记；fixture 会话都有真实写入 |
| `busy` | `null`（本 fixture 全部空闲） | 128/128 tasks 为 terminal，192/192 `pi.live` 为 `{}`；`wire.md:24` 允许 null |
| `name` 缺键时 | `""` | 正常路径 189/189 有键；本库不生成默认名（§2） |
| `updatedAt` 无时间源时 | `null` | 172/189 是重启前老会话；`wire.md:47-51` 明确 null 合法且禁止伪造 |
| `last` 无 entry 时 | `null` | 172/189 没有 entry；`wire.md:54` 明确允许 |
| `last.text` 只剩 thinking/toolCall 时 | `""` | 真值就是空文本（会话 2 的 entry 698），不要丢掉整个 `last` |

**结论（给下一张票）：** 本调研覆盖了 `wire.md` `GET /v1/sessions` 的全部 14 个字段；
所有能推的字段都给到了表 / 列 / JSON 路径 / 取最大值的规则，其余字段明确给了
null 或常量及原因。下一张票（`nous-fixture` 落地）照着这份文档能直接写代码，
不需要再回上游翻源码。
