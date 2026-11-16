# nous-chord

`packages/chord` 的六个目录，逐个 port。**已落地：`delta`。**

- **权威**：`docs/chord-delta-notes.md`（不变式清单）+ `wire.md`（对外事件形状）。上游只读参考。
- **依赖**：`delta` 只需要 `serde_json`。无内部依赖（AGENTS.md §二）。
- **未落地**：`tracker.ts` 的 overlay / piece tree / 竞争草稿 / 生命周期状态机。台账单独一行 `待提货`，
  理由写在那里 —— `Draft` 已经有语义（含 `append_text` 的 `a` op），但性能结构与并发还没做。
- **`json` / `context` / `output` / `watch` / `sqlite`**：随后按票落地。

两条移植规矩（`docs/pitfalls-from-pi.md` 判 `中`）：

1. `t` op 的 `count` 是 **UTF-16 码元**。用 `chars().count()` 会在 emoji 上悄悄漂，用字节会切烂 UTF-8。
2. `append_text` 必须存在，`prepare` 直接发 `a`；前缀检测只在没有 append 记录时走 —— 否则流式 SSE 从线性变二次。
