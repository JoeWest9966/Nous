# durable

> [中文](#zh) · [English](#en)

<a id="zh"></a>

## 1 它是什么 / What it is

中文：`durable` 是内核层的存储与事务边界 —— 一次提交里的写、会话与 entry 查询、文档修订，
全在这里收口。语义自己写（`AGENTS.md` §一：只借语义和坑，不借结构）：
上游 27,259 行里只取 storage / session / harness 的核心，`watch` 那套不要，事件走 `nous-wire`。
它认识 sqlite 与 seq，不认识 HTTP、进程、产品路由。

EN: `durable` is the kernel-layer storage and transaction boundary: writes in a commit,
conversation and entry queries, and document revisions all converge here. Its semantics
are written in-house (`AGENTS.md` §1: borrow the semantics and the pitfalls, not the structure).
Of the upstream 27,259 lines only the storage / session / harness core is kept; `watch` is not —
events go through `nous-wire`. It knows sqlite and seq, not HTTP, processes, or product routes.

## 2 什么时候用 / When to use

中文：要写会话状态、读投影、做文档修订或导入上游库时用 `durable`；只读装配可以用
`ImportedSource`。写必须走 `nous_durable::session::Session`，别开第二条裸 `Storage`
（#42 的坑：那会绕过 `ObserverRegistry`，别的客户端看不到）。要发 HTTP / SSE 就去 `server`；
要调工具就靠 `nous-ai::Tool` trait 注入，不把 `nous-tools` 拉进来。

EN: Use `durable` to write conversation state, read projections, revise documents, or import
an upstream database; read-only assembly may use `ImportedSource`. Writes must go through
`nous_durable::session::Session` — never open a second bare `Storage` (#42: it bypasses
`ObserverRegistry`, so other clients cannot see the write). HTTP / SSE belongs to `server`;
tools arrive via the `nous-ai::Tool` trait, and `nous-tools` is never a dependency here.

## 3 清单 / Manifest

由脚本从相邻 `plugin.toml` 生成，禁止手写。

Generated from the adjacent `plugin.toml` by the checker; do not hand-write.

## 4 API

> 待生成：见 #48 的 /v1/_routes（禁止手写占位表格）

## 5 权限 / Permissions

中文：`nous-durable` 是 kernel 层，公开面只给 pub API；它对上不认识进程与路由，
对下只有它自己管 sqlite 连接。写模式要认门：`durable_schema.writer = 'nous'` 才允许写，
认门只管写模式；只读打开上游 v2 库照读（导入器唯一入口），永不写回。
内核层不许依赖 platform / app 的 crate —— `cargo tree -p nous-durable` 里不该有 `nous-tools`。

EN: `nous-durable` is kernel layer: its public face is pub APIs only. It knows neither
processes nor routes above it, and owns the sqlite connection below it. Write mode checks
the gate: only `durable_schema.writer = 'nous'` may write; the gate applies to write mode only —
an upstream v2 database may be opened read-only (the importer's sole entry) and is never written back.
A kernel crate must not depend on platform / app crates: `cargo tree -p nous-durable` must not
contain `nous-tools`.

## 6 失败模式 / Failure modes

中文：导入器从记录重算 `conversations.owner_*` / `tasks.status` 等索引列，与源库逐列比对，
对不上就 `Shape` 失败 —— 不静默导入一份错的。`commit_semantics` 钉死三条负例：
半写回滚整批不可见、`SeqRegression { next_seq, floor }`（seq 回退不复用，上游没有这条守卫）、
`writer != 'nous'` 写拒只读通。测试数据来自真 fixture；fixture 不在时跳过并打印，不假绿。

EN: The importer recomputes index columns such as `conversations.owner_*` / `tasks.status`
from records and compares them column by column with the source; a mismatch is a `Shape`
failure — no silently wrong import. `commit_semantics` pins three negative cases: a half-write
rolls back the whole batch invisibly, `SeqRegression { next_seq, floor }` refuses seq reuse
(a guard upstream lacks), and `writer != 'nous'` refuses writes while reads pass. Tests use the
real fixture; if it is absent they skip with a printed note instead of faking green.

## 7 判据 / Judgement

中文：判据都跑在真 fixture 上。`storage_roundtrip.rs` 的
`the_fixture_round_trips_through_a_fresh_database_with_zero_field_diffs` 把 189 会话 /
209 entries / 961 文档 / 969 revisions / 1609 台账写进新库再读回，19,689 个单元逐字段
diff = 0；`the_sessions_render_wire_shaped_items_newest_first` 验 189 条按 id 降序、
字段名 camelCase、`null` 照样在场；`orphan_names_are_skipped_not_an_error` 验名字表 207 条
vs 会话 189 条；`an_upstream_fixture_is_refused_in_write_mode_but_open_read_only` 验认门两个方向。
`documents.rs` 十条覆盖 ZWJ emoji 的 base+delta 往返、一个窗口一条 op / 一条 revision、
退役左闭右开、复制三种、同批改源即拒，外加 fixture 961 份文档全部重放再折成 delta 重放。
一致性套件：`cargo test -p nous-durable --test conformance`（29 案），
`conformance/FLOOR.tsv` 记下限，防瘦身。

EN: All judgement runs on the real fixture. `storage_roundtrip.rs`::
`the_fixture_round_trips_through_a_fresh_database_with_zero_field_diffs` writes 189 conversations /
209 entries / 961 documents / 969 revisions / 1609 ledger rows into a fresh database and reads
them back: 19,689 cells, zero field diffs. `the_sessions_render_wire_shaped_items_newest_first`
checks 189 items in descending id order, camelCase field names, and `null` still present.
`orphan_names_are_skipped_not_an_error` checks 207 name rows vs 189 conversations, and
`an_upstream_fixture_is_refused_in_write_mode_but_open_read_only` covers both gate directions.
Ten `documents.rs` tests cover ZWJ-emoji base+delta round trips, one op / one revision per window,
half-open retirement, three copy modes, same-batch source edits rejected, plus replaying all 961
fixture documents and folding them back into deltas. The conformance suite is
`cargo test -p nous-durable --test conformance` (29 cases) with a floor in
`conformance/FLOOR.tsv` to prevent thinning.

## 8 反悔点 / Reversal

中文：存储格式照抄上游九张表，版本号自己从 1 数起 —— 这是可逆的：真要回到上游 v2 迁移路径，
改的是 `durable_schema.version` 的演进策略，不是重写表结构。认门与 `SeqRegression` 守卫
是不可逆的语义决定：一旦放开写模式或复用 seq，历史提交的身份就丢了，回退要去修数据。
`watch` 不搬、事件走 `nous-wire` 也是单向门：把事件塞回 durable 只会造出第二套真相。

EN: The storage format copies upstream's nine tables verbatim while its own schema version
starts at 1 — reversible: returning to the upstream v2 migration path changes the
`durable_schema.version` evolution policy, not the tables. The writer gate and
`SeqRegression` guard are irreversible semantics: allowing writes or reusing seq destroys the
identity of past commits, and reversing them means repairing data. Not porting `watch` and
routing events through `nous-wire` is a one-way door too: putting events back into durable
would create a second source of truth.

## 目录 / Layout

```
storage/
  sqlite/    schema / migrations / rows / statements / tables
  import.rs  只读导入：任意上游库 → 九张表 → 与源库逐列对齐
  sessions.rs 会话树 → wire.md 的 GET /v1/sessions
  mod.rs     SqliteStore：打开、认门、计数器
  writes.rs  一次提交里的写 + 「这一批合法吗」（Refusal 十二种）
  commit.rs  CommitState / PendingWrites：一次调用几条语句，拒绝不回滚已验证的批
  storage.rs 事务壳：seq 盖章、回执、id 分配器、扫描
  cursor.rs  不透明游标（a/d + base36 id），只有 id
  documents.rs 一个点上的值：最新 base + 其后的 delta 重放；复制 = materialize 源当 base
```

一致性套件 32 = 29 已落 + 3 不做（对象别名两条 + 不种根会话，理由在 `docs/not-ported.md`），
案名逐字照抄上游 `testing/storage-conformance.ts` 的 `createCase(`；变更要在 `conformance/`
里的变异实测中留痕。

The conformance inventory is 32 = 29 implemented + 3 deliberately not ported (two object-alias
cases + no root conversation seeding; see `docs/not-ported.md`). Case names are copied verbatim
from upstream `testing/storage-conformance.ts` `createCase(`; changes must be recorded in the
mutation notes under `conformance/`.

<a id="en"></a>
