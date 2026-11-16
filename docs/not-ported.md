# not-ported · 决定不提货的上游东西

`SOURCES.md` 只记**会有本库文件**的提货；这里记「读了、懂了、决定不要」。分开的理由：台账那五列
问的是「本库哪个文件来自上游哪里」，一个永远不会有本库文件的条目放进去，只会让台账永远差几行。

（`packages/durable/src/storage/scan.ts` 曾在这份清单里，理由是「崩溃恢复扫描」—— 读错了：它
40 行是**游标**助手，12b 已提货成 `crates/nous-durable/src/storage/cursor.rs`。）

四条 `nous-env` 的 JSONL 专用方法（`append` / `writer_lease` / `fs_sync` / `shell_override`）曾因判据
假红在 `SOURCES.md` 里待了一阵：它们的上游路径是 `crates/rpi-tools/src/env.rs`，而判据按前缀把
`crates/` 开头一律当本地路径。`a60645f` 把判据改成「磁盘上真存在才算本地路径」之后，它们搬到
这里 —— 判据修好之前没去绕，是故意的。

**判据由 `scripts/verify.sh` 第 4 段机器执行**：每行的上游路径都要能在 pin 的那个 tag 上
`git cat-file -e` 对上（不做的也编不了），且不许出现 `crates/` / `vendor/` 开头的本地路径
（不做的就不该有本地文件），理由列必须带票号（决定发生在哪张票上）。

四列：上游路径 / 来源 / 理由 + 决定票号 / 日期。

| 上游路径 | 来源 | 理由（决定票号） | 日期 |
|---|---|---|---|
| `packages/chord/src/sqlite/cloudflare.ts` | pi@v1.5.2 | Cloudflare Durable Objects 的 SQLite 适配器（192 行）；本库单进程，走 `local.rs`（决定：#9） | 2026-11-16 |
| `packages/durable/src/storage/memory.ts` | pi@v1.5.2 | 893 行的内存后端。测试用临时文件库，不为测试造第二个后端（票面第 3 条）（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/indexeddb.ts` | pi@v1.5.2 | 1,273 行的 IndexedDB 后端，本库没有浏览器宿主（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/sqlite/cloudflare.ts` | pi@v1.5.2 | 15 行的 Durable Objects SQLite 绑定，本库走 `nous-chord::sqlite::LocalDatabase`（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/jsonl/**` | pi@v1.5.2 | 986 行的 JSONL 后端，本库只要 SQLite 一份（决定：#12） | 2026-11-16 |
| `crates/rpi-tools/src/env.rs` | rpi@v0.24.2 | 四样 JSONL 专用方法：`open_append_file` / `append_with_receipt` + `AppendFile` / `AppendReceipt`（可复用追加句柄 + 回执，为 rpi-durable 的 JSONL 日志服务，本库写 SQLite）；`WriterLease` / `try_acquire_writer_lease` / `verify_fence`（跨进程写者租约，单写者由 SQLite 的 `BEGIN IMMEDIATE` 保证）；`sync_file` / `sync_directory`（手写 fsync，契约面只留 TS 的 `flush_file`）；`ShellOverride`（自定义解释器，TS 用 `string[]` 表达「不经 shell 的 argv」，两套并存就是两套契约）（决定：#6） | 2026-11-16 |
