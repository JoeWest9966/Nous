# 存储契约

表结构、版本号、迁移链**由本库自己定**，不从上游继承。这份文件是唯一的权威描述；`nous-durable/src/storage/` 里的代码要能逐条对上这里的每一条。

**2026-11-16 定稿**（决策票 #11；证据见 `docs/fixture-map.md` 对 `fixtures/agent.sqlite` 的实测）。七问全部有答案，正文即答案，不再有「待定」。

## 0. 一句话

**形状照抄上游，版本号和身份自己定。** 表九张不动（fixture 里九张全有数据），`durable_schema.version` 从 1 自己数起，另加一列 `writer` 认门。语义（提交序号、落库节奏、失败行为）由本库自己写死。

> **与 `AGENTS.md` §五 守卫一致。** 守卫那一行的原文是：
>
> > 表结构和 `record` 形状**照抄**上游（DDL 取 `storage/sqlite/migrations.ts` 的 v2 + 一列 `writer`；`record` 存 camelCase JSON）；**版本号从 1 数起、迁移链单向**、分拆列规则本库自己定。全部定在 `docs/storage.md`，改这三样就是改契约，先改文档
>
> 也就是：表结构与 `record` 形状照抄（DDL 取上游 migrations 的 v2 + 一列 `writer`），版本号从 1 数起、迁移链单向、拆列规则本库自己定 —— 三样全定在这份文档里。

## 1. 表：9 张，照抄，不减不加

```
conversations / entries / record_ids / documents / document_revisions
tasks / submissions / durable_metadata / durable_schema
```

理由：fixture 里九张全有数据，且下面两张是**在用**的，不是历史残留 ——

- `record_ids`（1609 行，id → 类型台账）是**分配器**：没有它就没法保证 id 单调且不复用（§8 硬约束 5）。
- `submissions`（56 行）里的 `request_id` 就是 `wire.md` 的幂等键：`POST /v1/sessions/:id/messages` 的 `requestId` 靠它去重。

## 2. 记录体：混合，照上游

**被 `WHERE` / `ORDER BY` / 联结用到的字段才拆列**，其余全进 `record TEXT`：

拆列的：`conversation_id` / `status` / `kind` / `abort_requested` / `background` / `commit_seq` / `record_type`。

规矩：新字段**默认进 `record`**，除非它需要被查询。这样子加字段不用迁移；需要查询时才升列（升列 = 一次迁移，见 §4）。

## 3. 两个版本，语义分开

| 东西 | 管什么 | 取值 |
|---|---|---|
| `durable_schema.version` | **表结构**版本 | 本库**从 1 数起**，不复刻上游的 2 |
| 记录版本（`pi.tool` v4 / `pi.generation` v3 / `document_revisions.version`） | **内容格式**版本 | 跟上游对齐 |
| `durable_schema.writer` | 这个库是谁写的 | `'nous'`（新列，表数仍是 9） |

**认门规则**：

```
写模式：durable_schema.writer = 'nous'，否则拒绝打开。
只读模式：不校验 writer，上游 v2 库照读（导入器唯一入口）。
导入器：永远只读，永不写回源库。
```

理由：fixture 是上游 v2 的真库、压根没有 `writer` 列（只读打开、`mode=ro`）；写模式才需要认门，否则「和上游库版本号撞车」会把库写坏。

## 4. 迁移单向

- `migrate_v(n) → v(n+1)`，**没有 down**。不可逆是明确的选择，不是遗漏。
- 每个迁移带一份 `v(n)` 库副本测试：拿一份 v(n) 的库 → 打开 → 迁移 → 断言 v(n+1) 的结构与内容。副本进 `fixtures/`（gitignore 挡着）。
- **读上游 v2 的库走导入器，不走迁移。** 两条路不许混：
  - **导入器** = 只读打开任意上游库 → 吐 `/v1` 形状（`nous-fixture` 走这条）。
  - **迁移** = 只推进**我们自己**写的库的版本。

## 5. 提交序号：单库全局递增

- `durable_metadata.next_seq` 一个计数器；`entries.commit_seq` **全局单调**（fixture 实证 3…5073，跨会话不分局）。
- `?since=` 是**全局游标**，重连语义 = `WHERE commit_seq > since`。
- **这是内部量，这一版不进 `wire.md`。** SSE 重连对外仍靠第一帧 `snapshot`（`wire.md` 的 SSE 段本来就是这么写的）。将来要把它变成对外契约，先改 `wire.md`（§4.4 的顺序），不是先改代码。

## 6. 进度落库：500 ms

- `pi.live` 每 **500 ms** 一个 revision。落库粒度 = **一次 append 一个 revision**。
- 为什么不是上游默认的 1000 ms：桌面端盯着屏幕，崩溃丢约一秒可见文字太刺眼。
- 为什么不是 100 ms：写放大换不到可感知的收益。
- 反悔点：有用户抱怨「崩了丢一坨」→ 降到 100–200 ms；写盘成为瓶颈 → 回到 1000 ms 并写清丢失窗口。

## 7. 失败 / 半写 / 多进程

- **单写者**：`BEGIN IMMEDIATE` + WAL + `busy_timeout = 5000`。
- **一次调用失败的三种样子，别压成一句**（见 §7.1）：
  1. **校验拒绝**（durable 自己的检查）—— 不抛，装进 `CommitResult.refused`；
     前 N−1 批照常提交，seq 逐批盖章，回执在 `COMMIT` 之后发。
  2. **数据库失败**（约束、IO、连接）—— 整次回滚，前 N−1 批不可见，seq 不推进。
  3. **认门失败**（`writer != 'nous'`）—— 写模式直接拒绝打开，根本不进入事务。
- IO / 磁盘错：该次生成落 `status = terminal`，`record` 里标 `storage_failure`，**不静默续跑**。
- 只读打开一律 `mode=ro`（fixture 就是这样，永不写回）。

### 7.1 拒绝 ≠ 回滚（上游原话）

出处 `packages/durable/src/storage/sqlite/storage.ts:160-195`：

```js
for (const writes of batches) {
  const checked = await this.#checkBatch(transaction, schema, writes);
  if ("refused" in checked) { refused = { error: checked.refused }; break; }  // ← break，不是 throw
  ...
}
await pending.flush(transaction, ...);   // ← 已接受的批次照常写
return refused === undefined ? { stored } : { stored, refused };   // ← refused 是返回值字段
```

上游注释原文：`A failure of the database rejects the call and rolls back the batches before it
with the transaction.`

也就是说 **`refused` 是 `CommitResult` 的一个字段，不是异常**；只有数据库自己的失败才让整次
调用失败。本库的 `CommitResult { stored, refused: Option<Refusal> }` 与它逐字对齐，
`tests/commit_semantics.rs` 两组测试各钉住一半：
`a_refused_batch_stops_the_call_but_the_batches_before_it_stay`（拒绝，前面留下）与
`a_half_written_call_rolls_back_whole_and_is_invisible`（数据库失败，整次不可见）。

**为什么不做成 savepoint**：上游不做，而且 `refused` 需要「前面已接受的批确实落盘」这个前提
才对调用方有意义（否则调用方无从知道哪一批进了库）。改成 savepoint 会让 `stored` 的含义变。

## 8. 硬约束

1. **`conversations.id` 与 `entries.id` 数值单调**：客户端拿 id 当游标（`?since=`），回退的 id 会打断续流。
2. **落库的顺序 = 对外可见的顺序**：SSE 能看到的，库里必须先有。反过来可以。
3. **只读打开不写入**：fixture 打开时 `read_only`，永远不写回。
4. **`agent.sqlite` 是一份真实库**（1,179,648 B / `conversations` 189 行 / `entries` 209 行 / `documents` 961 行），当 fixture 用；它的内容属于用户，**不进 git**。
5. **id 分配即占位，删除不回收**：`record_ids` 里有 244 个 entry 类型 id，`entries` 只有 209 行 —— 35 个 id 分配过、记录已不在。分配器**只增不复用**，否则与 fixture 的 id 空间语义分叉。
6. **文档会成孤儿**：`pi.*` 文档各 192 份、`conversations` 只有 189 行 —— 3 份是归档/撕过的会话留下的。导入器与 `/v1/sessions` 拼装都必须扛住「文档在、会话不在」：**跳过，不报错**。

## 9. 已知空白

**`entries.head` 列保留，本版不赋予语义**（fixture 209 行全 NULL）。要定它的意思得连着 fork / 归档一起定，那时再回来看。

**陈旧草稿的拒绝在 `Draft` 层，不在 `apply` 层**（`nous-chord`）。`Draft::prepare(current_revision)` 比对草稿出生时的修订号，不等就 `RevisionError::Stale` —— 两份草稿从同一个 base 出发，A 提交推进修订号后，B 再提交会**明确失败**，不会静默覆盖（上游 `tracker.ts:289` 同语义，有单测钉住）。

这条保证的边界要写明白：

- **单进程内有效**：凡是走 `Draft` 的写入都受保护。
- **跨进程无效**：`documents` 的修订号只在持有它的进程里推进；两个进程各写一份，靠的是 §7 的**单写者**（`BEGIN IMMEDIATE` + `busy_timeout`）+ §5 的 seq 单向，不是修订号比对。要跨进程陈旧检测得把修订号落库并在事务里 `WHERE revision = ?`，本版没做。
- **绕过 `Draft` 直接重放 op 不受保护**：`apply` 收的是相对 base 的一批 op，手里没有修订号（`tests/delta_semantics.rs::a_stale_batch_is_rejected_by_the_draft_not_by_apply` 把这个边界钉住了）。所以 **storage 自己必须做修订号校验**，不能指望 apply。

## 10. 落点：这一层的零件已经在 `nous-chord`

`#12` 不新造 SQLite 门面 —— 底下的东西已经在 `nous-chord::sqlite` 里，判据都跑过了：

| 需要的能力 | 在哪 | 判据 |
|---|---|---|
| 只读/读写打开、`BEGIN IMMEDIATE` 事务、`run_counted` | `nous_chord::sqlite::LocalDatabase` | `local.rs::the_fixture_opens_read_only_and_lists_nine_tables`（fixture 只读打开 + 9 张表 + 写失败） |
| 迁移（版本连续、整批原子、库比代码新就拒绝） | `nous_chord::sqlite::apply_sqlite_migrations` | `migrations.rs` 6 条（回滚、版本过新、元数据表前缀校验） |
| schema 前缀校验 | `nous_chord::sqlite::validate_schema_prefix` | `schema.rs` 3 条（含 `sqlite_` 保留名） |
| 严格 JSON / 深度上限 | `nous_chord::json` | `json.rs` 5 条 |
| 文档修订（base/delta）的编解码与重放 | `nous_chord::delta` | `delta_semantics.rs` 9 条 + 1,000 例随机往返 |
| 取消与进度 | `nous_chord::context`（`nous-env::Context` 就是它） | `context.rs` 10 条 |

所以 `#12` 要写的是**表结构与提交语义**，不是数据库门面。

### 10.1 12a 落地的部分（`crates/nous-durable/src/storage/`）

| 文件 | 内容 | 判据 |
|---|---|---|
| `sqlite/schema.rs` | 九张表 + 十六个索引的名字（可带前缀） | 键写错在开发期 panic；`every_key_is_present_exactly_once` |
| `sqlite/migrations.rs` | v1 = 最终形状（含 `sleeping`）+ `writer` 列；27 条语句 | 9 表 / 16 索引 / `writer='nous'` / `version=1` 全断言 |
| `sqlite/rows.rs` | `record` 原样、索引字符串 JSON 编码、哨兵 `0` / `""` | 孤立代理对与 ` ` 往返无损 |
| `sqlite/statements.rs` | 一条 JSON 数组 + `json_each` 批量写；大行走多行 `VALUES` | 500 行 = **1 条语句**；20 个大行 ≤ 2 条 |
| `sqlite/tables.rs` | 四张记录表 + `record_ids` / `documents` / `revisions` 的行构造 | 每张表一条单测 |
| `import.rs` | 只读导入 + **重算索引列再逐列对齐** | fixture 七表 19,689 单元读→写→读 diff = 0 |
| `sessions.rs` | `/v1/sessions` 形状（名字/模型来自旧引擎文档） | 189 条、id 降序；最新 revision 是 `delta` 时明确失败 |

**认门的两条边界**（都有测试）：上游 v2 库**写模式打不开**（版本过新）、缺 `writer` 列的库
**写模式也打不开**（`found: None`），两者**只读都能读**。

### 10.2 12b 落地的部分（提交语义）

| 文件 | 内容 | 判据 |
|---|---|---|
| `commit.rs` | `CommitState`（三类状态各一条语句 + 一次 join 读全）、`PendingWrites`（按表攒净效果）、拒绝语义（不用 savepoint） | 一次提交 500 行仍是几条语句；只读态文档只留最新 base、可回放的全留 |
| `writes.rs` | `StorageWrite` 七种、`claim`、`prepareDocumentActions`、`checkIds` / `checkDocuments`、`Refusal` 十二种 | id 重复/串类型、两条内容命令、delta 版本接不上、同地址两个在世化身，各一条单测 |
| `storage.rs` | `BEGIN IMMEDIATE` 事务壳、seq 盖章、回执、`mint_id`、会话/entry 扫描 | 见下三条负例 + 分配器 + 游标 |
| `cursor.rs` | 不透明游标（`a`/`d` + base36 id）、`scan_start`、`cut_page` | 编造的游标 / 方向不一致 = 拒绝 |

**三条负例**（`tests/commit_semantics.rs`，票面点名）：

1. **半写回滚整批不可见**：第 1 批合法、第 2 批撞 `tasks.status` 的 CHECK → 整次调用失败，
   会话 / 台账 / `next_seq` 一样都没动。（对照：只是「被拒绝」时，前面的批照常提交 ——
   没有 savepoint 就是这样。）
2. **seq 回退绝不复用**：把 `next_seq` 拨回 1 再提交 → `SeqRegression { next_seq: 1, floor: 1 }`。
   这条守卫是**上游没有的**：上游信任自己的计数器，我们不信 —— 拷库、半重放、手工改库都见过。
3. **`writer != 'nous'` 写拒、只读通**：`Storage::open` 拒绝，`Storage::open_read_only` 照读。

**id 分配器**：`mint_id` 只在内存里推进（上游一样），随下一次提交落盘；一旦 id 进了
`record_ids`，哪怕记录被删也永不再分配（`next_id` 只向前）。判据：分 5 个 → 删 3 条 →
重开 → 下一个是 7，台账里 entry 仍是 5 条。

### 10.3 12c 落地的部分（文档修订与复制）

| 文件 | 内容 | 判据 |
|---|---|---|
| `documents.rs` | `DocumentPoint`（`Current` / `At(seq)`）、`StoredDocument`、`materialize`：找最新 base → tail 逐条校验版本 → 逐条 apply | 见下五条 |
| `writes.rs` | `StorageWrite::DocumentCopy` / `CopySource` / `DocumentAction.copy`；`Refusal::CopySourceChanged` / `CopyRejected` | 同批改源 = 拒绝；源形状不符 = 拒绝 |
| `commit.rs` | `resolve_copies`：有复制时**先 flush 前面已接受的批**再读源（源才看得见）；`Sqlite` 错往上抛、其余算拒绝 | 复制出来的是**一条 base**，`deltas_since_base == 0` |

**读文档的四条判定**（`tests/documents.rs`）：

1. **跨版本没有 base = 明确失败**（`VersionCrossing`）：revision 序列里出现 `version != base.version`
   的 delta，说明库被写坏或我们的写入漏了 base —— 不猜、不按错的版本硬 apply。
2. **只留最新态的文档不能读过去**（`HistoryNotRetained`）：session / task 作用域与 `history: "latest"`
   的文档没有历史，给「最新的值当作过去的」就是伪造。
3. **在世是左闭右开**：退役那个 seq 上已经读不到，之前读得到。
4. **delta 存的是 op 数组**，读时用 `nous-chord` 的 `Decoder` + `TrustedOps::from_decoded` 再 apply ——
   信任边界只有一处（存进去的 ops 就是当初调用方给的 wire 形式）。

**带 emoji 的往返**：`an_emoji_rewrite_round_trips_through_a_base_and_a_delta` 的改写里含
ZWJ 家族 emoji（多个代理对），断言的是**逐字符**相等，不是长度相等 —— `t` 的计数是 UTF-16 码元，
截在代理对中间要整字符丢弃。

**真数据上的第一跑**：`every_fixture_document_replays_and_survives_a_delta_round_trip` 把 fixture 的
**961 份文档**全部重放一遍，并把每一份的值折成 delta 再重放，逐字段比；`truncation_counts_utf16_units_on_real_fixture_text`
在真中文串上做 `t 1`，断言正好少一个字符（fixture 不在时跳过并打印，不假绿）。

**「500ms 一个窗口 = 一条 op」**：窗口化在调用方（harness 的 `persistWithinMs`），存储侧看到的是
「一个窗口一条 delta revision」。判据在 `a_window_of_streamed_appends_is_one_op_and_one_revision`：
纯增长的两段文字折成**一条** `a`，一个窗口一条 revision，两个窗口两条。

## 11. 记录字段的对齐

对外记录的字段名由 `wire.md` 决定；内部存的东西可以和它不同，但**同名字段必须同义**。`pi.*` 记录里 assistant 消息的字段名（`stopReason` / `responseId` / `responseModel` / `rawStopReason` / `usage`）来自上游，`nous-ai` 的 `#[serde(rename_all = "camelCase")]` 已经对齐，改这里之前先看 `wire.md` 对应段。
