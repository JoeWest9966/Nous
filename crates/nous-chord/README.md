# nous-chord

`packages/chord` 的六个目录，逐个 port。**已落地：`context` / `json` / `delta` / `sqlite`。**
`output` / `watch` 在 #19（harness 才用）。

- **权威**：`docs/chord-delta-notes.md`（delta 的不变式）、`wire.md`（对外事件形状）、
  `docs/storage.md`（表结构与提交语义）。上游只读参考。
- **依赖红线**：`cargo tree -p nous-chord` 只该有 `serde_json` / `tokio-util` / `rusqlite`。
- **`context` 是全库唯一一份**：`nous-env::Context` 是它的 `pub use`（AGENTS.md §二）。
  进度挂在 `PROGRESS_KEY` 上，不是结构体字段。

## 四条移植规矩（判过 `中` 的坑）

1. **`t` op 按 UTF-16 码元计数**（`encode_utf16`）。`chars().count()` 在 emoji 上悄悄漂，
   字节会切烂 UTF-8。`apply` / `diff` / `draft` 三处单位必须一致。
2. **`append_text` 必须存在**，`prepare` 直接发 `a` op；前缀检测只在没有 append 记录时走 ——
   否则流式 SSE 从线性变二次。
3. **陈旧草稿是硬失败**：`Draft::prepare(current_revision)` 不等即 `RevisionError::Stale`，
   不静默覆盖（上游 `tracker.ts:289`）。
4. **`SqliteDatabase::transaction` 用 `BEGIN IMMEDIATE`**：立刻拿写锁，不和别的进程空转。

## 已有的测试判据

- `delta_semantics.rs`：三条不变式 + 1,000 例随机 `apply(diff(x)) == x`（含 wire 往返）；
  emoji / 代理对 / ZWJ 的 `t` 截断。
- `context.rs`：`AbortSignal.any` 语义（父取消传染子、子不传染父）、`withoutAbortSignal`
  遮蔽、`await_with_context` 取消只终止等待者。
- `sqlite/local.rs`：`mode=ro` 打开 `fixtures/agent.sqlite` 并列出 9 张表 + 只读写失败
  （fixture 不在时跳过并打印，不假绿）。

## 未落地

`delta/tracker.rs`（overlay / piece tree / 竞争草稿）与 `sqlite/cloudflare.rs`、
`context.rs` 的 `withTelemetry` / `traceIn`。`SOURCES.md` 各有行与理由。
