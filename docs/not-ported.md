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

下面几行有**两种粒度**，理由列自己写清是哪种：`案 …` 是某个上游文件里的一条用例（文件本身仍在提货，
只是那一条不抄），其余是整个文件不提货。行首路径重复出现是故意的 —— 判据按「一行的上游路径能不能
在 pin 的 tag 上 `git cat-file -e` 对上」验，重复的路径照样要真存在。

| 上游路径 | 来源 | 理由（决定票号） | 日期 |
|---|---|---|---|
| `packages/chord/src/sqlite/cloudflare.ts` | pi@v1.5.2 | Cloudflare Durable Objects 的 SQLite 适配器（192 行）；本库单进程，走 `local.rs`（决定：#9） | 2026-11-16 |
| `packages/durable/src/storage/memory.ts` | pi@v1.5.2 | 893 行的内存后端。测试用临时文件库，不为测试造第二个后端（票面第 3 条）（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/indexeddb.ts` | pi@v1.5.2 | 1,273 行的 IndexedDB 后端，本库没有浏览器宿主（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/sqlite/cloudflare.ts` | pi@v1.5.2 | 15 行的 Durable Objects SQLite 绑定，本库走 `nous-chord::sqlite::LocalDatabase`（决定：#12） | 2026-11-16 |
| `packages/durable/src/storage/jsonl/**` | pi@v1.5.2 | 986 行的 JSONL 后端，本库只要 SQLite 一份（决定：#12） | 2026-11-16 |
| `crates/rpi-tools/src/env.rs` | rpi@v0.24.2 | 四样 JSONL 专用方法：`open_append_file` / `append_with_receipt` + `AppendFile` / `AppendReceipt`（可复用追加句柄 + 回执，为 rpi-durable 的 JSONL 日志服务，本库写 SQLite）；`WriterLease` / `try_acquire_writer_lease` / `verify_fence`（跨进程写者租约，单写者由 SQLite 的 `BEGIN IMMEDIATE` 保证）；`sync_file` / `sync_directory`（手写 fsync，契约面只留 TS 的 `flush_file`）；`ShellOverride`（自定义解释器，TS 用 `string[]` 表达「不经 shell 的 argv」，两套并存就是两套契约）（决定：#6） | 2026-11-16 |
| `packages/durable/src/testing/storage-conformance.ts` | pi@v1.5.2 | **案** `detaches retained writes and every returned record`（32 案里的一条，不是整个文件 —— 其余 20 条已在 `conformance/storage_conformance.rs`）：JS 的对象别名问题，同一个对象既能被调用方改、又能被后来的提交改。Rust 里记录以 `Value` 交出、行以 `Vec<SqliteRow>` 克隆，不存在共享引用，照抄只能写成恒真断言（决定：#21） | 2026-11-17 |
| `packages/durable/src/testing/storage-conformance.ts` | pi@v1.5.2 | **案** `detaches prototype-like JSON keys without changing object prototypes`：同上的别名问题的另一半 —— `serde_json::Value` 是树，没有原型链，`__proto__` 只是一个普通键（决定：#21） | 2026-11-17 |
| `packages/durable/src/testing/storage-conformance.ts` | pi@v1.5.2 | **案** `reserves ID 1 for the immutable root conversation`：**本库不种根会话**。上游迁移写死一条 id = 1 的根会话，本库迁移只建表、不写记录 —— 这是故意差别，不是漏了。证据：`fixtures/agent.sqlite` 里 `select count(*) from conversations where id=1` = **0**，`min(id)` = 2（决定：#21） | 2026-11-17 |
| `packages/durable/src/testing/env-conformance.ts` | pi@v1.5.2 | 633 行 / 20 个 `createCase(` —— **是规范，但没有可测的真后端**：本库 `nous-env` 只有 trait（`FileSystem` / `Shell` / `ExecutionEnv`），一个实现都没有，而 conformance 测的正是真后端。造个假后端让它绿，什么也证明不了，那就是「绿得浅」。等本地 `FileSystem` + `Shell` 实现落地再抄（决定：#15） | 2026-11-17 |
| `packages/durable/src/testing/storage-benchmark.ts` | pi@v1.5.2 | 500 行 benchmark，`createCase(` 调用点 **0** —— 不是规范，与 `FLOOR.tsv` 的用例数下限无关。本库的性能基线是 `docs/bench-commit.md` 里那几条，按本库的写路径重写（1,000 次即时提交、每 100 次打一次 base、链长 200/400/800 的单次提交中位数）；照抄上游 benchmark 量的是上游实现（决定：#15） | 2026-11-17 |
| `packages/durable/src/testing/{assertions,runner,types,index,commit-writes}.ts` | pi@v1.5.2 | 五个**工具**文件（共 169 行）：断言门面 / 用例注册器 / 用例类型 / 出口 / 提交写入辅助，`createCase(` 调用点 0。本库只有一张规范文件，夹具与断言就内联在 `conformance/storage_conformance.rs` 顶部（`backend` / `conversation` / `count` / `revision_count` / `delta` …），照抄注册器等于给不存在的东西造一层间接（决定：#15） | 2026-11-17 |

## 扩展的段进真会话（2026-11-23 发现，当天接上）

**发现**：`nous-ext` 的 `Runtime::render_turn` 只有 `ext-probe` 在调，`server` 的轮次驱动器
没调 —— 扩展装得上、跑得稳、`items` 里是 `running`，但模型看不到那几段。

**接法**（两处都在内核，一处都不在扩展）：

1. `routes.rs::messages` 组装轮次时调 `inject_extension_section`：它把本会话的选中项拼成
   名字（`session_selection`：技能 → `skill:<名>`、模板 → `prompt:<名>`），在
   `spawn_blocking` 里调 `Runtime::render_turn_with`（1s 段预算，不能占异步执行器），
   把 `Turn.text` 落一条 `pi.system`（`sections.extensions`，entry id = `entry_id + 4`，
   模板/技能顺延到 `+5`）并塞进本轮 context。**没装运行时 / 渲染出空串**才跳过，
   「没选任何东西」不跳过 —— `nous` 那条扩展产出的就是基础 preamble。
2. 帧里的 `input.extensions` 从「装载名」改成**本会话选中的名字**（`skill:` / `prompt:`，
   顺序即选中顺序）。装载名对扩展没有信息量 —— 它自己就是其中之一。

**判据**：`crates/nous/server/tests/extension_injection.rs`（真进程，段文本逐字进
`sections.extensions`）+ `nous-ext-probe frame-selection`（帧里就是那两份名字）。

**还没做的**：`triggers.rs` 的 faux 轮次（`submission_to_turn` 拿不到 `AppState`）没有扩展段；
触发器只跑 faux 回声，先不接。
