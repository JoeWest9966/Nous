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

## 6. 进度落库：闸是 100 ms，**没有**「每 N 毫秒一个 revision」这种东西

上一版这里写的是「`pi.live` 每 **500 ms** 一个 revision」。**那个数是错的，已作废**：它假装
`pi.live` 有一支自己的节拍，而上游没有。上游的三个原文数字（`harness/agent.ts:36-40`）：

```ts
export const DEFAULT_PROGRESS_POLICY: ProgressPolicy = {
    partialIntervalMs: 100,   // 流式 partial 的闸
    outputIntervalMs: 100,    // 工具输出流的闸（外加按字节配速）
    persistWithinMs: 1_000,   // deferred 提交的最长窗口 —— 不是节拍
};
```

`pi.live` 的 revision 来自下面三处，**只有前两处有节拍，且都是 100 ms**：

| 来源 | 上游 | 我们 |
|---|---|---|
| 流式 partial（`generation.message`） | 闸 `partialIntervalMs = 100` | **已落**（`harness/generation.rs`）：每帧一次真提交 = 一条 revision |
| 工具输出（`tools[].output` / `droppedBytes` / `diagnostics`） | 闸 `outputIntervalMs = 100`，另按「每写 1 字节歇 1/100 KiB 秒」配速（`chord/src/output.ts:306` `PROGRESS_BYTES_PER_SECOND = 100 * 1024`，`:395` 用它算下一次） | **未落**（属工具执行票）。落的时候照抄这两个数，别自创 |
| 书签类变更（`run` / `compactions` / `compactionFailure` / `runFailure` / `retry` / abort） | **无自家节拍**：都走 `PROGRESS_COMMIT`（deferred），攒到下一次读或写 `pi.live` 的提交顺手带走，最迟 `persistWithinMs = 1000` | **无窗口，但必须自己提交**：deferred 被砍（`docs/decisions.md`）之后没有「攒着」这回事 —— **每次状态转换自带一次提交**（工具启动一次、结束一次、abort 一次），一次提交一条 revision。不许攒到工具结束，也不许「跟着下一个事件走」：一个跑 60 秒的 `bash` 期间没有下一个事件，攒就等于客户端 60 秒看不见它（`docs/harness.md` §1 ③ 附 (5)） |

`live.rs` 的九个函数（`end_run` / `add_compaction_status` / `compaction_status` /
`remove_compaction_status` / `fail_compaction_status` / `tool_slot` / `finish_slot` /
`remove_nested_slots` / `clear_progress`）**不提交**，只改 `Draft<LiveState>`（与上游签名一致）。
所以「书签类变更的提交点」不在 `#14` 里，在工具执行层 / 调度层；`#22` 的 SSE 出帧频率要从这里
推出来（revision 在哪儿生成 → 帧在哪儿发），不能反过来假设「每条 commit 都有一帧」。

为什么能这么定：

- **100 ms 的写放大已经付过一次了。** partial 就是 100 ms 一写；实测（`docs/bench-commit.md`）
  16 秒流 = 160 次提交、ext4 `NORMAL` 下 **110 ms 总墙钟**。所以「100 ms 太贵」「500 ms 换更多
  收益」这个理由当初没有证据，现在也没有。
- **崩溃丢失窗口比上游更小。** 上游最坏丢 `persistWithinMs = 1 s` 的可见文字；我们最坏丢**一个
  闸周期（100 ms）**。这是砍 deferred 白拿的，不是付代价换的。
- **`500` 从没落地过任何代码。** 它是「以为 `pi.live` 有自家节拍」时的猜测；`#23`/`#14` 落完
  之后，`pi.live` 只有 partial 一路在写。

反悔点：

1. 若真实桌面进程里测出 partial 100 ms 让盘饱和 → **先给 partial 加按字节配速**（上游
   `PROGRESS_BYTES_PER_SECOND` 那套），**不要**回到「500 ms 定时器」：定时器分不清「小增量」与
   「大增量」，配速分得清。改的时候把本条反悔点一起改写。
2. 若工具输出实测打满盘（例如 1 000 行/秒的日志）→ 加 `100 ms + 100 KiB/s` 配速（上游就这两个
   数）。仍不够才重开 deferred，那要同时改 `#13` 的「明确拒绝」决定。
3. 若以后真需要「一个窗口内合并所有 `pi.live` 变更」→ 那是 **deferred**，不是给 `pi.live` 加节拍。
   **不许在 `live.rs` 里加定时器**：`live.rs` 一旦自己起定时器，就有第二个写者与 partial 的闸打架，
   而 `pi.live` 的单写者前提（`docs/harness.md` §1 ③ 附 (1)）会当场破掉。

## 7. 失败 / 半写 / 多进程

- **单写者**：`BEGIN IMMEDIATE` + WAL + `busy_timeout = 5000` + `synchronous = NORMAL`。
  `NORMAL` 是数字逼出来的：ext4 上 1,000 次提交默认 FULL 要 11,200 ms（每次 ~11 ms 全在等 fsync），
  NORMAL 1,890 ms。代价写在这里别写在心里：**掉电会丢最近若干个提交，库不会坏**（WAL 下只有
  checkpoint 保证落盘）。依据与全部数字见 `docs/bench-commit.md`。
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
| `sqlite/rows.rs` | `record` 原样、索引字符串 JSON 编码、哨兵 `0` / `""` | 孤立代理对与 `\u0000` 往返无损 |
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

### 10.6 `#25` 落地的部分（`materialize(Current)` 的缓存）

| 文件 | 内容 | 判据 |
|---|---|---|
| `documents.rs` | `DocumentCache`（按文档 id，**上限 `CACHED_DOCUMENTS = 256` 条，LRU**）、`CachedDocument`（`seq` + `version` + `value` + `deltas_since_base`）、`materialize_cached`、`refresh`（一批写落库**之前**对着库校准）、`values_after_batch`（落库**之后**算这批的 `(version, value)`） | `tests/materialize_cache.rs` 六条 |
| `storage.rs` | `Storage.cache: Mutex<DocumentCache>`；`commit` 的每批：`refresh` → `flush` → `values_after_batch` → `stored_batch(…, values)`；公开的 `Storage::materialize` 走 `materialize_cached`；`Storage::cached_documents()` / `caches_document(id)` 只给判据用 | 同上 |
| `commit.rs` | `stored_batch` 不再自己读库：delta 的 `value` 由调用方算好传进来（`values`） | 同上 |

**缓存的有效性判据只有一条**：库里这份文档 `MAX(seq)` 等于缓存记下的 `seq`。理由：revision 只插不改
（主键 `(document_id, seq)`，没有 upsert），所以「最新那条还是我算值的那一条」⇒ 中间没有任何人写过
⇒ 那个值仍然成立。

**失效点**（顺序：都在事务里、写锁在手上）：

1. 该文档写了 base ⇒ `values_after_batch` 直接把缓存换成 base 的值（`deltas_since_base = 0`）；
2. 该文档被 `Retire` ⇒ 删掉缓存。**退役不写 revision**，所以「比 seq」这条判据单独看会漏它
   （`history: "latest"` 的文档退役时 revision 会被删掉，`MAX(seq)` 变 NULL 恰好挡住；`rewindable`
   的退役后 revision 全在，就真的会漏）：`materialize_cached` 与 `refresh` 都另外读 `retired_at`；
3. 该文档被 `Copy` ⇒ 新 id 本来就没有缓存；源文档不动；
4. **库被另一个连接 / 进程写过** ⇒ `refresh` 比 `MAX(seq)`，不符就删掉，退回重放一次。

**只缓存 `Current`。** `At(seq)` 是历史读取（还拿它判一条 delta 的 `version`），没有「每次都读同一个
点」的性质，缓存它只有伪造历史的收益。

### 上限：256 条，LRU —— 算术（不是注释里的一句「应该够了」）

缓存是**每个 `Storage` 一份**（一条连接的生命）。没有上限时它只在退役时删，于是「不释放」是个沉默的
漏，不是任何一个决定。

| 量 | 数 |
|---|---|
| 上限 | **256 条**（`CACHED_DOCUMENTS`），按**条目数**，不按字节数 |
| 一条的大小 | 文档值 + `record` + 几十字节簿记（`seq` / `version` / `deltas_since_base` / `touched`） |
| 典型（fixture 那 961 份文档） | 全部加起来 53 KB，平均 ≈ 56 B/份 ⇒ 256 份 ≈ **15 KB** |
| 最坏（`pi.live` 一份长答案 100 KB） | 256 × 100 KB ≈ **25 MB** 封顶（每连接） |
| 没有上限时的最坏 | 5,000 个会话 × 一份 100 KB 的 `pi.live` = **≈ 500 MB** 常驻，且没有任何一步会报错 |

按条目数而不是字节数的理由：一份值有多大由调用方决定（`pi.live` 在流式期间就是整段正文），
字节数上限会变成一个「按负载调的数字」，改它的理由永远说不清；条目数上限是个能写的常数，而
「最坏 25 MB」是可以算出来的。

**为什么是 256，不是 8 或 8,192。** 淘汰的代价是一次**完整重放**（链长 `n` ⇒ `n` 条 apply，线性），
而上限太小会让「反复重放」变成新的抖动源。256 比「一次 RPC 里同时活跃的文档数」（一页会话 + 当前
正在流的那几份）大一到两个数量级，同时又把最坏情况压在 25 MB —— 这两头都留着余量，中间不需要调。

**淘汰只影响成本，不影响正确性**，而且这不是靠小心：未命中走的是完整重放，那条路一直在（缓存不是
唯一的读法）。所以「被淘汰的文档读回来仍然等于直接读库」是一条**结构性**的性质，判据只需守住它
（`the_cache_stops_at_its_cap_and_an_evicted_document_still_reads_back_correctly`）。

**淘汰是 LRU 不是 FIFO。** 差别只有一句「命中是否推时间戳」，后果差很远：FIFO 下「每一轮都在读的
那一份」因为写得早而被淘汰，每次读都要重放；真正冷的那份一直占着格子。判据
`the_cache_keeps_what_was_read_recently_not_what_was_written_first` —— 这条必须读得到「某份还在不在
缓存里」才能判，所以 `Storage::caches_document(id)` 存在（只给判据用）。

**两条不许松的判据**：① 同一 store 里两份文档交替读，值各归各的；
② 加缓存之后 `materialize` 的结果与「另开一条只读连接直接从库里读」**逐字段相等** ——
`#13` 那条「publication `value` 等于直接读回」不因为加了缓存而放松。后者松了就是缓存与库不一致，
而这种不一致在流式 UI 上只表现为「最后一段文本没出现」。

**没有「缓存命中率」日志。** 这是语义层，不是指标层：`RUST_LOG` 里出现「命中/未命中」只会变成
被人盯着的噪声，而它既不解释正确性也不解释那 0.2 ms。

**「500ms 一个窗口 = 一条 op」**：窗口化在调用方（harness 的 `persistWithinMs`），存储侧看到的是
「一个窗口一条 delta revision」。判据在 `a_window_of_streamed_appends_is_one_op_and_one_revision`：
纯增长的两段文字折成**一条** `a`，一个窗口一条 revision，两个窗口两条。

### 10.7 `#21` 落地的部分（四个 `scan_*` + `close()`）

读的那一半从 `storage.rs` 挪进 `storage/scans.rs`：扫描是纯读，只依赖表名 + 索引列 + 不透明游标，
和提交 / id 分配器 / 缓存没有关系。`storage.rs` 里剩下的是几行委托。

| 入口 | 语义 | 默认方向 |
|---|---|---|
| `scan_conversations` | owner 两列可过滤 | 升序 |
| `scan_entries` | **fork 链**：自己那一段 + 每一代祖先到 `parent.at` 为止 | **降序**（最新的先） |
| `scan_tasks` | `conversationId` / `kind` / `status` / `abortRequested` / `background` | 升序 |
| `scan_submissions` | `conversationId` / `status` | 升序 |
| `scan_documents` | **作用域**（`kind` 可选、`key` 不参与）+ 在世判定 | 升序（没有降序这条路） |
| `find_document` | **地址**：`kind` + `scope` + `key` 五个索引列全相等 | — |
| `find_latest_head_marker` | fork 链上最新的带 `head` 的 entry = 一段历史的真实下界 | — |
| `entry_in_conversation` | 一条 entry 在这个会话的 fork 链上**看得见**吗 | — |

**「多取一条」的 `+ 1` 在调用方**（`fetch_records` 的 `rows` 是原样 `LIMIT`）。分页判据就靠多出来
的那一条，藏在 helper 里读代码时看不出来 —— 所以它不藏在 helper 里。

**过滤列是编码体**：`tasks.kind` / `submissions.request_id` 存的是 `encode_indexed_string` 之后的
索引字符串（§12.6 的「严格编码」），拿原文去比会**一条都匹配不上而且不报错**。

**两条默认方向不同不是口味**：任务/提交是最老的先（队列），entry 是最新的先（历史）。写反了翻页
照样能翻，只是顺序错，所以两条各写一遍、各测一遍。

**fork 链两条方向的做法不一样**：降序从自己往下读，读满就停，没读满就把上界收到 `parent.at` 换去
父会话（`upper` 一路取 `min`）；升序必须先把链走完再**反过来**从最老的读 —— 升序的第一页来自根，
而根是走到链尾才知道的。两者的下界都对 `after + 1` 取 `max`。

**`close()` 只加一个门闩，不做生命周期管理**：`closed: bool` + 每个入口 `ensure_open()`，关掉之后
读、写、取 id 全失败（`StorageError::Closed`，Display 里有 `closed`）。上游还做「等 flight 走完、
放掉声明」，本库单进程单写者，flight 的等待由 `&mut self` 借用本身保证。`Drop` 仍是收尾正路；
`close()` 存在的理由只有一个：**关掉之后还拿着这个值的代码必须报错**。所以它不消费 `self`
（消费掉就写不出那条判据了 —— 上游 `rejects every operation after close` 就是关完再调三个入口）。

## 11. 记录字段的对齐

对外记录的字段名由 `wire.md` 决定；内部存的东西可以和它不同，但**同名字段必须同义**。`pi.*` 记录里 assistant 消息的字段名（`stopReason` / `responseId` / `responseModel` / `rawStopReason` / `usage`）来自上游，`nous-ai` 的 `#[serde(rename_all = "camelCase")]` 已经对齐，改这里之前先看 `wire.md` 对应段。

### 10.4 12d 落地的部分（一致性套件）

`crates/nous-durable/conformance/storage_conformance.rs`，`[[test]] name = "conformance"`。

- **来源**：`packages/durable/src/testing/storage-conformance.ts` 的 32 个 `createCase(` 抄了 **29** 个。
  案名逐字照抄（改名就是换判据）；`createCase` → 一条 `#[test]`，`storage` 夹具 → 临时文件真后端。
- **不造假后端**：套件跑在真 SQLite 上。`nous-env` 只有 trait、没有真后端，所以
  `env_conformance.rs` 现在不做 —— 测一个假后端什么也证明不了。它的判据随本地后端一起进 #13。
- **`FLOOR.tsv` 记 29**，文件头写了复算命令
  （`grep -cE '^[[:space:]]*#\[(tokio::)?test' …`），所以「套件被悄悄写瘦」这件事是机器判的。
- **32 = 29 已落 + 3 不做**（`#21` 清零）。3 条不做的是对象别名两条（Rust 里记录以 `Value` 交出、
  没有共享引用，也没有原型链）与「不种根会话一条」（本库迁移只建表，证据：真 fixture 里
  `conversations where id=1` 是 0 行）。理由都在 `docs/not-ported.md`，票 #21 留着那张 32 行的表。
- **十条变异实测**在文件头（改被测实现、跑完还原）：六条各自红该红的那几条；两条**不红**的也写在
  那里并给了原因 —— 「这条案测不到这个面」比「假装它测到了」有用。

`nous-fixture`（`verify.sh` 第 2/3 段的对象）只干一件事：读 `agent.sqlite`（只读）→ 打
`{ items: [...] }` → exit 0。无子命令、无 flag、不写库。

### 10.5 13 落地的部分（记录类型 + 提交发布面）

三个 commit，拆开是为了让每一个都能单独 review 完：

| commit | 落点 | 判据 |
|---|---|---|
| ① | `src/types/{records,draft}.rs`（14 个类型 + 取值域枚举） | 五种记录从**真 fixture** 读进来再写回去，逐字段相等（189 / 209 / 128 / 56 / 961）；取值域逐格验 |
| ② | `src/session/publish.rs` + `src/session/entries.rs` | publication 的 JSON 形状与上游 `types.ts` 逐字段相同；一次提交里三种记录各一次、只有一个 `seq`；文档 `ops` 原样透传 |
| ③ | `src/session/mod.rs`（`SessionEnd` + `Session`）+ `src/storage/commit.rs` 的 `StoredBatch` 回执 | 12 条（见下） |

**publication 不是 SSE**（写在这里是因为这条最容易被顺手破坏）：

- `CommitPublication = {seq, changes[]}` 是「**这次事务存了什么**」的事实；SSE 帧是「**客户端看到
  什么**」的投影，投影住 `nous-server`（#22）。
- 投影里有一半东西 publication 里本来就没有：`snapshot` 帧、`message_start`、
  `message_update.changes[]` —— 那些是生成态（harness）的产物。按投影倒推 publication，等于让
  存储层长出客户端概念，`#22` 也就不薄了。
- `document` 变更带的 `value` + `ops` 是 live projection 的原料（#14 的 `partial-changes`），裁掉
  它，#14 得回头重算 diff。
- `conversationId` 在 **session 作用域**的文档上**键缺席**（上游 `ConversationId | undefined`），
  不是 `null`；`document.copy` 的那个是必填。
- 反序列化**逐键对账**：多一个键就是错（§12.6 的同一条规矩）。

**`Session` 公开面 = 5 个方法**（判据是扫源码数的，`tests/session.rs`，多一个就红）：

| 方法 | 语义 |
|---|---|
| `async fn commit(&self, Vec<StorageWrite>, CommitOptions) -> Result<CommitOutcome, SessionError>` | 落库 → 发布给观察者 → 返回。**只收一批写**：`Tx` 回调不进 Rust，事务只有 `Storage::commit` 一处 |
| `async fn close(&self) -> Result<(), SessionError>` | 幂等；密封入场、等**在飞的提交走完**（同一把锁）、关存储、观察者收 `end(ended)` |
| `async fn closed(&self) -> SessionEnd` | **一个 settle 点**（`SessionEnd::Closed` / `Failed{error}`），不是 watch 通道；`#19` 要 watch 时再改签名 |
| `async fn snapshot(&self, id) -> Result<Option<StoredDocument>>` | 一份文档的当前值（`materialize(Current)`），按 `id` 定位 |
| `async fn get_entry(&self, id) -> Result<Option<(EntryRecord, Seq)>>` | 一条 entry 与它落库的 `seq` |

`observe_doc` / `watch_doc` / `observe_commits` / `subscribe_close` **属 #19**。发布侧的**回调契约**
（`CommitObserver`）本票交，挂在构造器入参上（`Session::with_observers`）——所以公开面是 5 个方法而不是 7 个。

**关闭的三条语义**（票面钉死，`tests/session.rs` 逐条测）：
1. `close()` 之后 `commit()` 得到 `SessionError::Closed` —— 明确错误，不 panic、不挂住；
2. `close()` 幂等 —— 第二次返回 `Ok`（不是「已经关过」的错误）；
3. 在飞的提交走完再关 —— 提交与关共用一把异步锁；测试用「发布时刻晚于 `close()` 调用时刻」见证
   （批次太小会让这条测试白跑，断言里直接写着别删）。

**存储失败 = 会话自己关掉**（上游同一语义）：失败后 `closed()` 是 `Failed{error}`，之后的调用一律
`Closed`，但 `close()` 仍幂等地返回 `Ok`。

**`CommitPersistence.deferred` 明确拒绝**（`SessionError::DeferredUnsupported`），不静默按
`immediate` 存：上游的延后提交只对「丢了也无所谓」的流式进度成立，静默降级正好是崩溃丢数据那类差别。

**观察者**：`CommitObserver::advance` 是同步回调（签名钉住，不许 await），先快照名单再逐个调，
所以观察者可以在回调里 `detach()` 自己。上游把观察者抛的错接住并 `end(listener_error)`；Rust 没有
「抛错」这个通道，`panic` 会冒到 `commit()` 的调用方 —— 我们**不** `catch_unwind` 兜住：把库里的
bug 变成「事件静默丢失」比崩掉更难查。

### 10.5.1 `#22` 撑大的公开面（Rust 里 `pub` 就是契约）

`#22`（端点层）需要三件 durable 里**原本没有公开**的东西。加在这里不是「顺手 export」，是
`#13` 那张 5 方法表之外**第二张**要有人负责的表。记住这一条：`Session` 的五个方法没变，
下面这些都是 **sibling 模块 / 自由函数**，但它们在 Rust 里同样是契约。

| 新增 | 形状 | 为什么住 durable 而不是 `nous-server` | `#22` 之后谁来用 |
|---|---|---|---|
| `session::commands::{submit_message, abort, fork}` | `async fn(&Session, …) -> Result<…, CommandError>`；配套 `SubmitRequest` / `SubmitOutcome` / `AbortOutcome` / `ForkRequest` / `ForkOutcome` | 三条命令都要「铸 id + 一次提交 + 把 publication 交给观察者」。id 分配器与提交锁在 `Session` 里（`Storage::mint_id` 不落盘，必须和下一次提交在同一个事务里推进）；把这三步拆到端点层等于让 `nous-server` 重做一遍提交语义 | `nous-server` 的 `POST /messages` / `abort` / `fork` 直接调；将来 harness 起生成时也走 `submit_message` 的同一份受理语义 |
| `harness::project::{tool_changes, ToolChange}` | `fn(&CommitChange) -> Result<Vec<ToolChange>, HarnessError>`；`ToolChange::{Start, Update}` | 工具启动必须解 `ops` 才知道「谁刚开始跑」；`nous-server` 的契约面**不含 `nous-chord`**（§二），所以解码只能在 durable 这边做。从批后 `value.tools` 反推会把「每次提交都把整轮工具重发一遍」当成正确行为 | `nous-server` 的 `message_update` 旁边的 `tool_execution_start/update`（issue #22 的 ≤1 拍判据钉着） |
| `storage::sessions::{render_entries, render_entries_page, render_live_message, ENTRIES_MAX_LIMIT, LIVE_DOCUMENT_KIND}` | 只读：entries 的 `/v1` 形状（`model` 原样透传）、一页 + 不透明游标、`pi.live.generation.message` | 这是「旧引擎文档 → `wire.md` 形状」的知识（`qrd3wh.sessions` / `pi.agent` / `pi.live` 的文档布局）。端点层不知道这些 kind，也不该知道 | `nous-server` 的 `GET /entries` 与 SSE `snapshot`；将来 12c 落自己的文档契约时替换掉 |
| `session::publish::{Context, WatchEnd}` 重新导出 | `pub use nous_chord::{context::Context, watch::WatchEnd}` | 它们是 `CommitObserver` 签名的一部分。不 re-export，`nous-server` 为了写一个 `impl CommitObserver` 就得直接依赖 `nous-chord`，而 §二 没有那条边（`verify.sh` 4e 会红） | `nous-server` 的观察者适配层；`#19` 的订阅实现同理 |
| `Storage::{submissions, tasks, latest_entry_id}` | 只读查询：一个会话的 submission / task 记录（按 id 升序）、最后一条 entry id | `requestId` 幂等要按 `conversationId + requestId` 查库；abort/fork 的前置条件要读活跃任务。这些是存储语义，不是端点的 | `session::commands`；将来 `#25` 的缓存失效判定也会读同两条 |

**不进这张表的东西**：`Session` 的 5 个方法一个没加（`tests/session.rs` 扫源码钉着）；
`commands` 是子模块，所以扫描器看不到它 —— 这不是绕过判据，判据的意图是「不许悄悄给 `Session`
加方法」，而这三条命令本来就不该是 `Session` 的方法：它们要**先读后写**（幂等查找、活跃任务
检查），而 `Session::commit` 只收一批已经定好的写。

**代价**：公开面多一行就多一条兼容义务。`#22` 之后要改 `CommandError` 的四个分类、或去掉
`ToolChange` 的某个变体，都得先看 `nous-server` 的路由映射与 `tests/{commands,harness_projection}.rs`。

## 12. 与上游的故意差别

这四条是**我们和上游不一样的地方**，不是还没做完的地方。写在这里的唯一理由：以后最容易撞车的就是它们，
散在票面和 issue 里等于没有。改这一节之前先想清楚 —— 每一条都在某处被测试钉着。

### 12.1 不种根会话

上游 `migrations` 里播一条 `conversation id = 1` 的不可变根会话；**本库迁移只建表，不写记录**。

这不是「漏了」，有证据（2016-11 复核，真引擎写的库就是没有）：

```
fixtures/agent.sqlite
  conversations  id 范围 2 … 1694，id=1 不存在（0 行）
  record_ids     id=1 无任何记录（0 行）
```

顺带的结果：`record_ids` 从 2 开始分配也不撞人 —— 上游「id 1 属于根会话」这条约定在本库没有对应语义。

### 12.2 `CommitResult.refused` 是返回值，不是异常

校验拒绝（`Refusal`）**不回滚**：前 N−1 批照常提交，`stored` 是真正落库的批数，`refused` 说明第 N 批为什么
被拒，之后的批一个都不碰。只有**数据库自己失败**（`StorageError::Sqlite`）才整次回滚。

这不是我们的设计发明，是上游 `storage.ts:160-195` 的循环语义（`break` 不是 `throw`）。详见 §7 / §7.1：
为什么不做 savepoint 也写在那儿。

### 12.3 版本号从 1 数起

上游 `durable_schema.version` 从 2 数（v1 是历史包袱）；**本库从 1 数起**，迁移链单向。

所以两边的数字**不许互相比较**：看到上游 v2 不等于本库 v2。表结构照抄，版本号自定 —— 这条在
`SOURCES.md` 的迁移行里有说明。

### 12.4 `record` 拆列规则本库自己定

`entries.record` 这类 JSON 列，上游用一张「哪些键提到独立列」的隐式约定；本库把这条规则写死在自己的
`storage/sqlite/tables.rs` 里（重算索引列 → 与源库逐列对齐，对不上就报 `Shape` 错误），并让导入器把它
当成**判据**而不是搬运。

改这条规则 = 改 `tables.rs` + 改导入器的重算，两边必须同时动，否则老库读出来会和自己的索引列打架。

### 12.5 这三处是「我们比上游弱」的地方：原样存、原样取的不透明 JSON

不是「以后再说」，是**当前就弱一档**，故意留在这一节里 —— 不写下来，下一个人会以为我们实现了 `TaskState`。

| 字段 | 上游是什么 | 本库是什么 | 为什么暂时不追上 |
|---|---|---|---|
| `TaskRecord.state` | 泛型判别联合 `TaskState<S, R>`：`pending` / `running` / `waiting` / `sleeping` / `completing` / `terminal`，每种状态带自己的字段（`mem$`、`checkpoint`、`waitingFor`…） | **不透明 JSON**：写进来什么原样存，读出去原样给，不校验、不解释 | 本库没有 task runner（`tasks` 表只存记录）；搬这套判别联合等于把 task 引擎的状态机也搬进来，而引擎不在本库的四个 crate 里 |
| `TaskRecord.memos` | `Readonly<Record<string, JsonValue>>`，first-writer-wins 的语义由上游 runner 维护 | **不透明 JSON** | 同上：语义的维护者不在本库 |
| 记录上的 `trace?: TelemetryCarrier` | 类型来自 `pi-telemetry` 包（`SubmissionRecord`、`TaskRecord`、`TaskOptions` 都有） | **不透明 JSON**：adapter 塞进来什么就存什么 | 本库 `nous-telemetry` 是 `rpi-telemetry@v0.24.2` 的冻结版，没有同名类型；造一个形状不同的 `TelemetryCarrier` 只会让两份名字打架 |

**判据**：这三处的测试只断言「写进去的 JSON 与读出来的一字不差」，**不**断言字段内部结构。
哪天要追上（比如 #14 之后开始有 task runner），是**删掉这节**并同时改 `docs/storage.md` §11 与
`SOURCES.md`，不是悄悄加类型。

### 12.6 「我们比上游严」的地方，以及为它付的维护成本

上游把一批约束写在**类型**里（TS 编译期），运行时收到一份宽松的对象不会有任何抱怨。我们把这些约束
落在**反序列化**里：认不出、配不上就报错。代价是有的，所以逐条写下来，免得下一个人以为是手滑。

| 约束 | 上游怎么表达 | 本库怎么落 | 维护成本 |
|---|---|---|---|
| `ContextEdit` 的两个动作各带自己的键 | `{action:"omit", messages?: never} \| {action:"replace", messages: Message[]}` | `records.rs` 里**手写 `Deserialize`**：`omit` 带 `messages` → 错，`replace` 不带 `messages` → 错 | 加字段要动三处（结构体、`Raw` 中转体、`match` 分支） |
| `TaskRecord.abandonOnRestart` 是字面量 `true` | `readonly abandonOnRestart?: true` | `deserialize_true`：`false` 直接报错 | 同上，两行 |
| 提交的 `type` × `status` 矩阵 | 判别联合，10 个合法格子 | `SubmissionFields::validate`：`continue`+`queued`、`write`+`placed`、`reason` 只在 `unanswered`…… 逐格拒 | 矩阵变了要同时改 `validate` 与 `tests/records_domains.rs` |
| 文档 `scope` × `history` × `fork` | `DocumentCreate` 的判别联合 | `DocumentRecord::validate`：`asOf` 只在 `rewindable` 下、`session`/`task` 作用域不许带 `history`/`fork` | 同上 |

**为什么 `ContextEdit` 必须手写，而不是加个 `deny_unknown_fields`**：serde 的**内部标签枚举**
（`#[serde(tag = "action")]`）**不支持** `deny_unknown_fields` —— 加了也是静默无效。派生版本会把
`{"action":"omit","target":9,"messages":[]}` 解析成功并把 `messages` 扔掉，也就是把「客户端塞了不该有的
载荷」变成「悄悄忽略」。这个坑是**先写成测试、红了才发现的**，不是设计时想到的。

**规矩**：只有「多出一个键就等于另一种语义」的枚举才手写反序列化；其余继续 derive。手写的那几处必须
在同一次提交里带一条「非法形状明确报错」的测试，否则下次改结构不会有人发现约束已经松了。
判据：`crates/nous-durable/tests/records_domains.rs`。

### 12.7 `waitMs`：传了就 400，不等

`POST /v1/sessions/:id/messages` 的 body 里出现 `waitMs` → **400 `{error}`**，不会挂住等结果。

- **为什么**：`wire.md` 明写桌面端的用法是「发消息 → 202 → 靠 SSE 收内容」，并说别用 `waitMs`
  等同步。实现一条没有人走的同步路径，等于在端点层再造一套超时/取消语义，而它不受任何客户端
  判据约束。
- **反悔点**：出现**真要同步等**的调用方时，二选一 —— 实现它（`waitMs` 上限、超时返回当前
  submission 状态、取消语义都要先写进 `wire.md`），或者把状态码改成 501 明说「不实现」。
  不许改成「静默忽略」：那会让调用方以为等到了。
- 判据：`crates/nous-server/tests/commands_endpoints.rs::fork_while_busy_is_409_and_wait_ms_is_400`。

### 12.8 `whenBusy`：当前**静默忽略**，行为 = 照样排队

body 里出现 `whenBusy` → 收下、不解释、**不改变行为**：这次输入照样写一条 `queued` submission。

- **为什么**：`wire.md` 只列了这个键，没有定义取值域与语义；自己发明一套「reject / queue /
  interrupt」会让客户端按猜的写。当前的确定行为是：**忙不忙都排队**（`busy` 只影响列表显示）。
- **反悔点**：先在 `wire.md` 里把取值域写死（例如 `reject` 时返回 409），再在
  `session::commands::submit_message` 里按值分支，并在同一提交里补「忙 + reject → 409、忙 +
  queue → 202」两条判据。**在那之前，任何「whenBusy 生效了」的说法都是假的。**
- 判据：目前没有行为判据，只有「字段存在且不影响 202」这条隐含事实；加语义时先加判据。

### 12.9 上游 v2 副本的「领养」（`NOUS_ADOPT_LEGACY=1`）

`nous-server` 的 bin 可以对一份**上游 v2 库的副本**做一次性领养：补 `durable_schema.writer`
列（值 `nous`）、把 `version` 记成 1。**表结构不动、行数据不动** —— 本库 v1 的 DDL 就是上游
v2 的最终形状（`tasks.status` 的 `sleeping` 已含在内）。

- **默认拒绝**：不设这个开关时，`NOUS_DB` 指向上游 v2 库会明确报错并打印操作提示（§12.3：
  版本号不可比，写模式拒 v2）。
- **为什么需要它**：端到端验证要在**可写副本**上跑（不能写 `fixtures/agent.sqlite` 语料）。
- **守卫**：只在「没有 `writer` 列 + 版本号正好 2」时动作；已有认门值的库拒绝领养
  （`WriterMismatch`）。判据：`crates/nous-durable/tests/adoption.rs` 两条。
- **反悔点**：真要做「上游库 → 本库」的常规迁移时，单独开票写导入器（只读源 → 新库），
  **不要**把领养扩成通用路径。领养是破坏性的：跑完那份文件在本库眼里就是 v1，真上游再打开
  会看到旧版本号。

### 12.10 `scan_entries` 走 fork 链；找不到会话是**错误**，不是空页

两条都与上游一致，写在这里是因为它们容易被当成「我们的怪癖」：

- **fork 链**：`scan_entries({conversationId})` 读的是「这个会话 + 祖先到各自 fork 点为止」的全部
  entry，不是只有自己那一段。fork 出来的会话只存自己那一段，之前的历史住在父会话里 —— 只读自己
  那一段会得到一个「历史凭空从中间开始」的会话。
- **未知会话报错**（`UnknownConversation`）：空页要留给「会话在、但历史到头了」。两者混成一个结果，
  客户端就分不出「翻完了」与「id 写错了」。同理 `entry_in_conversation` / `find_latest_head_marker`。

**反悔点**：某天要给「不存在的会话」返回 404 而不是 500 时，改的是端点层的映射，不是这里的语义
（`StorageError::UnknownConversation` 是给端点层分类用的）。
