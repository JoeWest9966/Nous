# not-ported · 决定不提货的上游东西

`SOURCES.md` 只记**会有本库文件**的提货；这里记「读了、懂了、决定不要」。分开的理由：台账那五列
问的是「本库哪个文件来自上游哪里」，一个永远不会有本库文件的条目放进去，只会让台账永远差几行。

四条 `nous-env` 的 JSONL 专用方法（`append` / `writer_lease` / `fs_sync` / `shell_override`）本该在这里，
但它们的上游路径是 `crates/rpi-tools/src/env.rs` —— 判据的「不许出现 `crates/` 开头」会把它当成
本地路径（假红，见票 #12 的评论）。判据改成「本地存在才算本地路径」之后挪过来；在那之前它们留在
`SOURCES.md`，改动列写明「不提货（票 #6）」。

**判据由 `scripts/verify.sh` 第 4 段机器执行**：每行的上游路径都要能在 pin 的那个 tag 上
`git cat-file -e` 对上（不做的也编不了），且不许出现 `crates/` / `vendor/` 开头的本地路径
（不做的就不该有本地文件），理由列必须带票号（决定发生在哪张票上）。

四列：上游路径 / 来源 / 理由 + 决定票号 / 日期。

| 上游路径 | 来源 | 理由（决定票号） | 日期 |
|---|---|---|---|
| `packages/chord/src/sqlite/cloudflare.ts` | pi@v1.5.2 | Cloudflare Durable Objects 的 SQLite 适配器（192 行）；本库单进程，走 `local.rs`（决定：#9） | 2026-11-16 |
| `packages/durable/src/storage/scan.ts` | pi@v1.5.2 | 40 行的崩溃恢复扫描；它的形状还在地图 #1 的 Not yet specified 里，先不做，别照抄一半（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/memory.ts` | pi@v1.5.2 | 893 行的内存后端。测试用临时文件库，不为测试造第二个后端（票面第 3 条）（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/indexeddb.ts` | pi@v1.5.2 | 1,273 行的 IndexedDB 后端，本库没有浏览器宿主（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/sqlite/cloudflare.ts` | pi@v1.5.2 | 15 行的 Durable Objects SQLite 绑定，本库走 `nous-chord::sqlite::LocalDatabase`（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/jsonl/**` | pi@v1.5.2 | 986 行的 JSONL 后端，本库只要 SQLite 一份；JSONL 专用的四样 env 方法也已按 #6 不做（决定：#12） | 2026-11-16 |
