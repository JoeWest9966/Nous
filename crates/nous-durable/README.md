# nous-durable

`durable` 的语义自己写 —— `AGENTS.md` §一 的原话是**只借语义和坑，不借结构**。
上游 27,259 行里我们只取 storage / session / harness 的核心，`watch` 那套不要（事件走 `nous-wire`）。

**已落地**：`storage/` 的行层与只读导入（12a）、提交语义（12b）、文档修订与复制（12c）
**待落地**：conformance（12d）、session、harness。

## 目录

```
storage/
  sqlite/    schema / migrations / rows / statements / tables（上游 storage/sqlite/** + store/tables.ts）
  import.rs  只读导入：任意上游库 → 九张表 → 与源库逐列对齐
  sessions.rs 会话树 → wire.md 的 GET /v1/sessions
  mod.rs     SqliteStore：打开、认门、计数器
  writes.rs  一次提交里的写 + 「这一批合法吗」（Refusal 十二种）
  commit.rs  CommitState / PendingWrites：一次调用几条语句，拒绝不回滚已验证的批
  storage.rs 事务壳：seq 盖章、回执、id 分配器、扫描
  cursor.rs  不透明游标（a/d + base36 id），只有 id
  documents.rs 一个点上的值：最新 base + 其后的 delta 重放；复制 = materialize 源当 base
```

## 三条自己定的规矩（改之前先看 docs/storage.md）

1. **表结构照抄、版本号自己定**：九张表一字不改，`durable_schema.version` 从 1 数起
   （上游是 2，我们不复制它 v1→v2 重建 `tasks` 的历史）。
2. **认门只管写模式**：`durable_schema.writer = 'nous'` 才允许写；只读打开上游 v2 库照读
   （导入器唯一入口），永不写回。
3. **导入器重算索引列再对齐**：从记录重算 `conversations.owner_*` / `tasks.status` 等列，
   与源库逐列比对，对不上就 `Shape` 失败 —— 不静默导入一份错的。

## 判据（都跑在真 fixture 上）

- `storage_roundtrip.rs::the_fixture_round_trips_through_a_fresh_database_with_zero_field_diffs`
  —— 189 会话 / 209 entries / 961 文档 / 969 revisions / 1609 台账 → 写进新库 → 读回，
  **19,689 个单元逐字段 diff = 0**。
- `…::the_sessions_render_wire_shaped_items_newest_first` —— 189 条、id 降序、字段名 camelCase、
  `null` 照样在场。
- `…::orphan_names_are_skipped_not_an_error` —— 名字表 207 条 vs 会话 189 条：孤儿跳过不报错。
- `…::an_upstream_fixture_is_refused_in_write_mode_but_open_read_only` —— 认门两个方向都有测试。
- `commit_semantics.rs` 三条负例：**半写回滚整批不可见**、**seq 回退不复用**
  （`SeqRegression { next_seq, floor }`，上游没有这条守卫）、**`writer != 'nous'` 写拒只读通**；
  外加 seq 逐批盖章 + 回执晚于 `COMMIT`、分配器删记录后也不复用、游标分页（升/降序 + 方向不符即拒）。
- `documents.rs` 十条：带 ZWJ emoji 的 base+delta 往返、一个窗口一条 op / 一条 revision、
  退役左闭右开、只留最新态的文档拒读过去、跨版本无 base 即错、复制的三种（当前 / 过去点 / 形状不符）、
  同批改源即拒；外加 **fixture 961 份文档全部重放 + 折成 delta 再重放**（真中文串上做 `t 1`）。

fixture 不在时这些测试**跳过并打印**，不假绿。

## 依赖红线

`nous-durable → chord, ai, wire, telemetry, env`。**`cargo tree -p nous-durable` 里永远不该有
`nous-tools`** —— 工具靠 `nous-ai::Tool` trait 注入。
