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
- 事务失败 = 回滚 = 内存态保留 = **对外不发布**（先落库后可见，硬约束 2）。
- IO / 磁盘错：该次生成落 `status = terminal`，`record` 里标 `storage_failure`，**不静默续跑**。
- 只读打开一律 `mode=ro`（fixture 就是这样，永不写回）。

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

## 11. 记录字段的对齐

对外记录的字段名由 `wire.md` 决定；内部存的东西可以和它不同，但**同名字段必须同义**。`pi.*` 记录里 assistant 消息的字段名（`stopReason` / `responseId` / `responseModel` / `rawStopReason` / `usage`）来自上游，`nous-ai` 的 `#[serde(rename_all = "camelCase")]` 已经对齐，改这里之前先看 `wire.md` 对应段。
