# nous-wire

`wire.md` 的 Rust 投影：`/v1/sessions/:id/events` 的 SSE 事件类型。

- **权威**：仓库根 `wire.md`。它和这个 crate 不一致时改这里。
- **来源**：上游 remote 协议的 `protocol.rs` —— 只取事件枚举与形状，帧 / CBOR / 客户端那套
  一律不要。逐行出处见仓库根 `SOURCES.md`（那张表写的是完整路径）。
- **依赖**：只有 `serde` + `serde_json`。无内部依赖（AGENTS.md §二）。
- **不抄的**：上游有、`wire.md` 没有的事件（`ui_dialog_*` / `agent_start` / `agent_end` /
  `retry_scheduled` / `turn_*` / `tool_execution_end`、`ready` / `response` / `command` 帧）
  统统落 `RemoteEvent::Unknown`。宪法没写的，库里不该先长出来。
- **局限**：`Unknown` 不保留原来的 `type` 字面量，所以这层不能做保真重放。
