# SOURCES · 提货台账

一行一个来源单元。五列：本库路径 / 来源 / 上下游路径 / 改动 / 状态。**判据：每一行都能用 `git -C <来源> cat-file -e <tag>:<上下游路径>` 对上；对不上的行是错的 —— 改台账或删代码。** 这条判据由 `scripts/verify.sh` 第 4 段机器执行，不是口头约定。

状态只有四个值：

- `待提货` —— 计划里，还没抄进来
- `已提货` —— 抄进来了，和上游一致
- `已改` —— 抄进来了，本库改过（改动写进第四列，一句说清）
- `自写` —— 本库自己写的，没有上游可对（上游只提供形状参考）

来源代号：`rpi@v0.24.2` = `bigfish1913/pi-rust`（本地只读检出 `/tmp/pi-rust`）；`pi@v1.5.2` = `earendil-works/pi`（本地只读检出 `/tmp/pi-check`）。

上下游路径列的规矩（机器要读）：**一个路径，不带注记**。`/**` 表示整棵子树，`{a,b}` 表示并列的多个文件；注记写进「改动」列。台账**不写行数**（行数随上游漂，作不了判据）。

## 一、冻结拷贝（pi-rust）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-telemetry/**` | rpi@v0.24.2 | `crates/rpi-telemetry/src/**` | 原样；包名 `rpi-telemetry` → `nous-telemetry`；README 换成带出处的抬头 | 已提货 |
| `crates/nous-ai/src/providers/google_generative_ai/build_params.rs` | pi@v1.5.2 | `packages/ai/src/api/google-shared.ts` | TS→Rust 翻译：convertMessages / convertTools / thinking 解析 / mapStopReason | 已改 |
| `crates/nous-ai/**` | rpi@v0.24.2 | `crates/rpi-ai/src/**` | 原样（`src/**` + `tests/**`）；包名 `rpi-ai` → `nous-ai`、依赖 `rpi-telemetry` → `nous-telemetry`（含注释与测试里的 `rpi_ai::` 路径）；`providers` / `smoke` feature 保留 | 已提货 |
| `crates/nous-ai/src/tool.rs` | rpi@v0.24.2 | `crates/rpi-agent/src/agent_tool.rs` | 只取 `AgentTool` trait，零逻辑改动（连带 `ToolExecutionMode`，它是 `execution_mode()` 的返回类型，跑不掉）；`rpi_ai` → `nous_ai` 只改名 | 已改 |
| `crates/nous-ai/src/tool.rs` | rpi@v0.24.2 | `crates/rpi-agent/src/error.rs` | 只取 `AgentError` 枚举 + `impl`，零逻辑改动（`Box<crate::types::AgentToolResult>` → 同模块 `Box<AgentToolResult>`） | 已改 |
| `crates/nous-ai/src/tool.rs` | rpi@v0.24.2 | `crates/rpi-agent/src/types.rs` | 只取 `AgentToolResult` / `TextContentOrImage` / `ToolResultPartial`（含同源的三个便捷方法与 `From` 实现），零逻辑改动；`rpi_ai::types::X` → `crate::types::X` | 已改 |
| `crates/nous-ai/src/providers/google_generative_ai/**` | pi@v1.5.2 | `packages/ai/src/api/google-generative-ai.ts` | TS→Rust 翻译（本库第一个**自己写**的 provider，不是提货）：`mod.rs` = stream/streamSimple/createClient，`build_params.rs` = buildParams + `google-shared.ts` 的 convertMessages/convertTools/thinking，`mapper.rs` = 流式映射 + mapStopReason。结构化差异：SDK 摊平的 REST body 手写、`abortSignal` → `CancellationToken`。**已知缺口：`toolChoice`（TS 的 `GoogleOptions` 有，`rpi-ai` 的 `SimpleStreamOptions` 没有）。固定按「未给」处理。要支持 `auto`/`none`/`any` 得给 provider 层加 per-provider 选项通道，那是扩展契约，等真有调用方要「强制/禁止工具」时再开票。** 测试备注：方言之间的工具声明对拍只覆盖 anthropic ↔ google（两者的 builder 是 `pub`）。openai 两个方言的 builder 在冻结文件里是私有的，**不为了多两方对拍去改可见性** —— `diff -r` 归零是本库唯一「原样提货」的机器判据，把它换掉不划算。将来真要覆盖 openai，走公开入口（`streamSimple` 喂 options、抓请求体断言 tools 形状），不改可见性 | 已改 |
| `crates/nous-tools/**` | rpi@v0.24.2 | `crates/rpi-tools/src/**` | 改 use：`rpi_agent::*` → `nous_ai::{tool,types}`（11 文件）；`rpi_plugin_sdk` 两处常量内联成 `&str`；`env.rs` 移出到 `nous-env`；`os_env.rs` 的 `process::resume_child` **未提货**（需子进程控制，超出契约面） | 待提货 |
| `crates/nous-runtime/**` | rpi@v0.24.2 | `crates/rpi-runtime/src/**` | 原样；包名改名，依赖 `rpi-ai`+`providers` → `nous-ai`+`providers` | 待提货 |
| `crates/nous-wire/src/event.rs` | rpi@v0.24.2 | `crates/rpi-cli/src/remote/protocol.rs` | 只取事件枚举的**形状语义**，类型、变体、辅助方法由本库按 `wire.md` 定；上游有而宪法没有的一律不抄（`ui_dialog_*` / `agent_start` / `agent_end` / `retry_scheduled` / `turn_*` / `tool_execution_end`、`ready`/`response`/`command` 帧），全落 `Unknown`；帧 / CBOR / 客户端那套不要。测试是手写 JSON 对宪法，不照着上游类型编 | 自写 |
| `crates/nous-durable/src/tokens.rs` | rpi@v0.24.2 | `crates/rpi-harness/src/compaction/tokens.rs` | 原样 | 待提货 |
| `crates/nous-durable/src/retry.rs` | rpi@v0.24.2 | `crates/rpi-harness/src/agent_harness.rs` | 摘录 retry 段，其余不要 | 待提货 |

## 二、env 契约层（形状取 TS 那份，不是拷贝）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-env/src/**` | pi@v1.5.2 | `packages/durable/src/env/index.ts` | **形状取 TS**（`durable/src/env/index.ts`），名 snake_case 化 + `Context` 合并（`AbortSignal`+telemetry → `CancellationToken`+进度 sink，改一次不散进 116 个调用点）；不提货 Rust 那份的四样（JSONL 专用，见下）；三档能力写进 trait 文档（必需 14 个无默认实现 / 可选 12 个默认 `not_supported`）；`truncate_file` 按 TS 语义（可缩可扩）；`cwd` 只读；**不带** IO 实现（本地 / SSH / InMemory 都在 trait 之下） | 已改 |
| `crates/nous-env/src/types.rs` | pi@v1.5.2 | `packages/durable/src/env/index.ts` | 只取 `FileChange`（`paths`/`overflow`/`failed`）、`WatchTarget`、`FileWatcher`（`native`/`polling`）。**备注**：文件监视在 rpi-rust 那边不在 `env.rs`，在 `crates/rpi-harness/src/watcher.rs` 与 `crates/rpi-cli/src/fs_watch.rs` —— 所以别以为「Rust 没实现，我们也可以不做」：**我们要做，走 TS 形状** | 已改 |
| `crates/nous-env/src/error.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/error.rs` | 原样：`FileError`/`FileErrorCode`、`ExecutionError`/`ExecutionErrorCode`、`io_to_file_code`/`io_to_file_error`（本地/SSH/内存三个后端要给出同一组 code，所以分类留在契约层）；**补 `ExecutionError::spill_path`**（TS 有 `spillPath`，Rust 版没有，而 `spill` 是我们明确要带走的） | 已改 |
| `crates/nous-env/src/append.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/env.rs` | **不提货**：可复用追加句柄 + 回执（`AppendFile` / `AppendReceipt` / `open_append_file` / `append_with_receipt`）。理由：为 rpi-durable 的 JSONL 日志服务，本库写 SQLite | 待提货 |
| `crates/nous-env/src/writer_lease.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/env.rs` | **不提货**：跨进程写者租约（`WriterLease` / `try_acquire_writer_lease` / `verify_fence`）。理由同上 | 待提货 |
| `crates/nous-env/src/fs_sync.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/env.rs` | **不提货**：`sync_file(path, data_only)` / `sync_directory`（目录 fsync 让 rename 真正落地）。理由同上；契约面只留 TS 的 `flush_file` | 待提货 |
| `crates/nous-env/src/shell_override.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/env.rs` | **不提货**：`ShellOverride`（自定义解释器 + 固定参数）。理由：TS 用 `string[]` 命令表达「不经 shell 的 argv」，两套并存就是两套契约 —— 取 TS 那套 | 待提货 |

## 三、自己 port（官方 pi）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-chord/src/json.rs` | pi@v1.5.2 | `packages/chord/src/json.ts` | 翻译；`JsonRepresentation` 落到 serde 类型约束，不留 `any` 旁路；运行期四项（环/稀疏数组/symbol/非有限数）在 Rust 是类型保证，只留深度校验（`MAX_JSON_DEPTH`），理由写进模块文档 | 已改 |
| `crates/nous-chord/src/context.rs` | pi@v1.5.2 | `packages/chord/src/context/index.ts` | 翻成 Rust；`AbortSignal` → `CancellationToken`（`AbortSignal.any` 换成「沿链多 token，任一取消」）；`ContextKey<T>` 用 `TypeId` 做键（TS 是 `Symbol()`，代价是同一类型只能挂一个值）；`awaitWithContext` 的底层 future 会被丢弃（Rust 与 promise 的差别，写进模块文档）。**例外：`withTelemetry` / `traceIn` 两处未提货** —— 跨 `nous-telemetry` 的 span 接线未定，先按 `ContextKey` 挂值，接通时再定要不要变成一等成员；不为「161 行抄全」把没想清楚的东西焊进去 | 已改 |
| `crates/nous-chord/src/output.rs` | pi@v1.5.2 | `packages/chord/src/output.ts` | 翻译；`pi-telemetry` → `nous-telemetry` | 待提货 |
| `crates/nous-chord/src/watch/**` | pi@v1.5.2 | `packages/chord/src/watch.ts` | 翻译；**`watchEvents` 视角不要**，事件出口接 `nous-wire` | 待提货 |
| `crates/nous-chord/src/sqlite/{database,schema,migrations,local}.rs` | pi@v1.5.2 | `packages/chord/src/sqlite/{database,schema,migrations,node}.ts` | 翻成 Rust；`node:sqlite` → `rusqlite`（`bundled`，版本从 workspace 引）；去掉 Promise（同步，调用方要挪线程自己 `spawn_blocking`）；行是「列名 → `SqliteValue`」而非泛型反序列化；事务用 `BEGIN IMMEDIATE`（`docs/storage.md` §7） | 已改 |
| `crates/nous-chord/src/sqlite/cloudflare.rs` | pi@v1.5.2 | `packages/chord/src/sqlite/cloudflare.ts` | **未提货**：Cloudflare Durable Objects 的 SQLite 适配器（192 行）；本库是单进程 SQLite，走 `local.rs` | 待提货 |
| `crates/nous-chord/src/delta/{mod,op,apply,codec,diff,pending,validator,draft}.rs` | pi@v1.5.2 | `packages/chord/src/delta/{index,diff,pending,apply-immutable-trusted,revision-validator,draft}.ts` | 翻成 Rust；去掉 Promise（同步 `Result`）；`Proxy`/descriptor 换成显式命令式 API（`set`/`remove`/`append_text`/`splice`）；`WeakSet` 的 copy-on-write 换成「每批克隆一次 root」（语义同、性能结构不同，见模块文档）；数组 diff 简化成前后缀对齐 + 一段 splice（**精确但不规范**，判据只有重放值）；`t` 按 **UTF-16 码元**（`encode_utf16`）计数与切分；`assertXxx` 换成 `Result<_, OpError>` | 已改 |
| `crates/nous-chord/src/delta/tracker.rs` | pi@v1.5.2 | `packages/chord/src/delta/tracker.ts` | **未提货**：overlay 节点树 / piece tree / 位置缓存 / 竞争草稿与 `open→prepared→consumed→aborted→stale` 生命周期状态机（上游 2,319 行）；`emit` 的保留段折叠与 4096 上限已在 `diff`/`draft` 里以简化形式存在。`Draft` 目前建在克隆值上、`prepare` 用 diff 兜底 —— 语义已通、性能结构与并发生命周期未做 | 待提货 |
| `crates/nous-durable/src/storage/**` | pi@v1.5.2 | `packages/durable/src/storage/**` | 翻译；**表结构与版本号自己定**（`docs/storage.md`）；`commit()` 批写 | 待提货 |
| `crates/nous-durable/src/session/**` | pi@v1.5.2 | `packages/durable/src/session/**` | 翻译 | 待提货 |
| `crates/nous-durable/src/session/entries.rs` | pi@v1.5.2 | `packages/durable/src/entries.ts` | 翻译 | 待提货 |
| `crates/nous-durable/src/session/documents.rs` | pi@v1.5.2 | `packages/durable/src/documents.ts` | 翻译 | 待提货 |
| `crates/nous-durable/src/harness/generation.rs` | pi@v1.5.2 | `packages/durable/src/harness/generation.ts` | 翻译；同文件摘 abort 段 | 待提货 |
| `crates/nous-durable/src/harness/live.rs` | pi@v1.5.2 | `packages/durable/src/harness/live.ts` | 翻译 | 待提货 |
| `crates/nous-durable/src/harness/partial_changes.rs` | pi@v1.5.2 | `packages/durable/src/harness/partial-changes.ts` | 翻译 | 待提货 |
| `crates/nous-durable/src/harness/generation.rs` | pi@v1.5.2 | `packages/durable/src/harness/harness.ts` | 摘 abort 段并入（`onAbort: withdraw`）；watch 那套不要 | 待提货 |
| `crates/nous-durable/src/harness/generation.rs` | pi@v1.5.2 | `packages/durable/src/harness/util.ts` | 摘 abort 相关辅助并入 | 待提货 |
| `crates/nous-durable/src/harness/partial_changes.rs` | pi@v1.5.2 | `packages/durable/src/harness/types.ts` | 摘 abort / 部分变更相关类型并入 | 待提货 |
| `crates/nous-durable/src/record/**` | pi@v1.5.2 | `packages/durable/src/types.ts` | 只摘记录版本语义（`pi.tool` / `pi.generation` / `task_too_old`）；**不整份搬类型** | 待提货 |
| `crates/nous-durable/conformance/storage_conformance.rs` | pi@v1.5.2 | `packages/durable/src/testing/storage-conformance.ts` | 改写成 Rust：**≥ 16 个 `#[test]`**（上游 2,099 行 / **32 个 `createCase(`** 调用点，取一半）。数字来源命令写进文件头，可复算：`grep -c '^\s*createCase(' packages/durable/src/testing/storage-conformance.ts` → 32。**案名逐条照抄英文原文**（如 `reserves ID 1 for the immutable root conversation`），一个不许改名 —— 上游加了案，一条 grep 就能看出缺哪条 | 待提货 |
| `crates/nous-durable/conformance/env_conformance.rs` | pi@v1.5.2 | `packages/durable/src/testing/env-conformance.ts` | 改写成 Rust：**≥ 10 个 `#[test]`**（上游 633 行 / **20 个 `createCase(`**，取一半；同上命令可复算）。案名照抄 | 待提货 |
| `crates/nous-durable/conformance/storage_benchmark.rs` | pi@v1.5.2 | `packages/durable/src/testing/storage-benchmark.ts` | 改写成 Rust：**benchmark 不是规范**，`createCase` 调用点 0，不参与 `FLOOR.tsv` 的用例数下限；只用来定本库的性能基线 | 待提货 |
| `crates/nous-durable/conformance/{assertions,runner,types,mod,commit_writes}.rs` | pi@v1.5.2 | `packages/durable/src/testing/{assertions,runner,types,index,commit-writes}.ts` | **自写**薄门面：用例注册器 + 断言门面 + 用例类型 + 出口 + 提交写入辅助。上游这五个共 169 行，它们是**工具**，`createCase` 调用点 0 —— 不为凑数逐行翻译，只作形状参考 | 自写 |

## 四、只用不改

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `vendor/pi-env-daemon/**` | pi@v1.5.2 | `packages/env/daemon/**` | 原样，**只 build 不改**；自带 `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml`，本仓根 `Cargo.toml` 必须 `exclude = ["vendor/pi-env-daemon"]` | 待提货 |
| `docs/spec-from-pi-durable.md` | pi@v1.5.2 | `packages/durable/docs/spec.md` | 摘录 + 逐条「采纳 / 不采纳」+ 反悔点 | 已改 |
| `docs/pitfalls-from-pi.md` | pi@v1.5.2 | `packages/durable/CHANGELOG.md` | 只追加判定（`中` / `不中` / `未知`），不改旧条目 | 已改 |
| `wire.md` | NousDesk@`wire.md` | `wire.md` | 迁入并改为权威版本（Nous 侧是只读镜像）。**这行是唯一对不上 tag 的**：来源是 NousDesk 仓，不是 pi/pi-rust，没法用 `git cat-file` 核；台账体检会把它列进「跳过」并打出来。留着是为了记出处，不是漏改 | 已提货 |

## 五、不在本库（写下来是为了以后不用再讨论）

```
chord:   host / node / store / remote / surfaces / tui / web / react / sync /
         jobs / link / contracts / registries / data / state / telemetry-ingest   ≈ 5.5 万行
durable: codemode / images / tool-search
pi:      packages/{ai, agent, coding-agent, tui, mcp, schema, telemetry, codemode, evals}
pi-rust: crates/{rpi-harness, rpi-cli, rpi-tui, rpi-extensions, rpi-plugin-sdk}
         （只有 tokens.rs / retry 段 / protocol.rs 三处例外，见上表）
```
