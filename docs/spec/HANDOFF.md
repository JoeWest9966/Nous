# HANDOFF · 内核 spec

- **spec**：`docs/spec/kernel-rust.md`（九章 + 附录 A 的 31 条已拍板清单）。术语在 `GLOSSARY.md`。
- **票**：`#14` 搬迁（最后做）、`#15` 置顶进内核（**阻塞** `#16` 的 C 段）、`#16` 删代码（A 段可并行，C 段等 `#15`）。
- **判据怎么跑**：`scripts/verify.sh` 八段 + spec 里那些 grep（`spawn` 恰好 1 处、`flock` 为空、`wire.md` 唯一的端点索引、`Nous-Package` 计数为 0、`resolve` 不含秘密）。
- **票由别人做，你不改 spec**，除非附录 A 里某条的"反悔点"命中，且把命中证据写进票里。
- **你验，不许合并 `main`**。
