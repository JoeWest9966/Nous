# nous-durable

`durable` 的语义自己写 —— `AGENTS.md` §一 的原话是**只借语义和坑，不借结构**。
上游 27,259 行里我们只取 storage / session / harness 的核心，`watch` 那套不要（事件走 `nous-wire`）。

**已落地（12a）**：`storage/`
**待落地**：提交语义（12b）、文档修订（12c）、conformance（12d）、session、harness。

## 目录

```
storage/
  sqlite/    schema / migrations / rows / statements / tables（上游 storage/sqlite/** + store/tables.ts）
  import.rs  只读导入：任意上游库 → 九张表 → 与源库逐列对齐
  sessions.rs 会话树 → wire.md 的 GET /v1/sessions
  mod.rs     SqliteStore：打开、认门、计数器
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

fixture 不在时这些测试**跳过并打印**，不假绿。

## 依赖红线

`nous-durable → chord, ai, wire, telemetry, env`。**`cargo tree -p nous-durable` 里永远不该有
`nous-tools`** —— 工具靠 `nous-ai::Tool` trait 注入。
