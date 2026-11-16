# SOURCES · 提货台账

一行一个来源单元。五列：本库路径 / 来源 / 上下游路径 / 改动 / 状态。**判据：每一行都能用 `git -C <来源> cat-file -e <tag>:<上下游路径>` 对上；对不上的行是错的 —— 改台账或删代码。** 这条判据由 `scripts/verify.sh` 第 4 段机器执行，不是口头约定。

状态只有三个值：

- `待提货` —— 计划里，还没抄进来
- `已提货` —— 抄进来了，和上游一致
- `已改` —— 抄进来了，本库改过（改动写进第四列，一句说清）

来源代号：`rpi@v0.24.2` = `bigfish1913/pi-rust`（本地只读检出 `/tmp/pi-rust`）；`pi@v1.5.2` = `earendil-works/pi`（本地只读检出 `/tmp/pi-check`）。

上下游路径列的规矩（机器要读）：**一个路径，不带注记**。`/**` 表示整棵子树，`{a,b}` 表示并列的多个文件；注记写进「改动」列。台账**不写行数**（行数随上游漂，作不了判据）。

## 一、冻结拷贝（pi-rust）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-telemetry/**` | rpi@v0.24.2 | `crates/rpi-telemetry/src/**` | 原样；包名 `rpi-telemetry` → `nous-telemetry`；README 换成带出处的抬头 | 已提货 |
| `crates/nous-ai/**` | rpi@v0.24.2 | `crates/rpi-ai/src/**` | 原样（`src/**` + `tests/**`）；包名 `rpi-ai` → `nous-ai`、依赖 `rpi-telemetry` → `nous-telemetry`（含注释与测试里的 `rpi_ai::` 路径）；`providers` / `smoke` feature 保留 | 已提货 |
| `crates/nous-ai/src/tool.rs` | rpi@v0.24.2 | `crates/rpi-agent/src/agent_tool.rs` | 只取 `AgentTool` trait，零逻辑改动（连带 `ToolExecutionMode`，它是 `execution_mode()` 的返回类型，跑不掉）；`rpi_ai` → `nous_ai` 只改名 | 已改 |
| `crates/nous-ai/src/tool.rs` | rpi@v0.24.2 | `crates/rpi-agent/src/error.rs` | 只取 `AgentError` 枚举 + `impl`，零逻辑改动（`Box<crate::types::AgentToolResult>` → 同模块 `Box<AgentToolResult>`） | 已改 |
| `crates/nous-ai/src/tool.rs` | rpi@v0.24.2 | `crates/rpi-agent/src/types.rs` | 只取 `AgentToolResult` / `TextContentOrImage` / `ToolResultPartial`（含同源的三个便捷方法与 `From` 实现），零逻辑改动；`rpi_ai::types::X` → `crate::types::X` | 已改 |
| `crates/nous-ai/src/providers/google_generative_ai.rs` | pi@v1.5.2 | `packages/ai/src/api/google-generative-ai.ts` | TS→Rust 翻译；rpi 缺这一个方言 | 待提货 |
| `crates/nous-tools/**` | rpi@v0.24.2 | `crates/rpi-tools/src/**` | 改 use：`rpi_agent::*` → `nous_ai::{tool,types}`（11 文件）；`rpi_plugin_sdk` 两处常量内联成 `&str`；`env.rs` 移出到 `nous-env`；`os_env.rs` 的 `process::resume_child` **未提货**（需子进程控制，超出契约面） | 待提货 |
| `crates/nous-runtime/**` | rpi@v0.24.2 | `crates/rpi-runtime/src/**` | 原样；包名改名，依赖 `rpi-ai`+`providers` → `nous-ai`+`providers` | 待提货 |
| `crates/nous-wire/src/event.rs` | rpi@v0.24.2 | `crates/rpi-cli/src/remote/protocol.rs` | 只取事件枚举与形状；帧/CBOR/客户端那套不要 | 待提货 |
| `crates/nous-durable/src/tokens.rs` | rpi@v0.24.2 | `crates/rpi-harness/src/compaction/tokens.rs` | 原样 | 待提货 |
| `crates/nous-durable/src/retry.rs` | rpi@v0.24.2 | `crates/rpi-harness/src/agent_harness.rs` | 摘录 retry 段，其余不要 | 待提货 |

## 二、env 契约层（从 rpi-tools 里抽出来，不是拷贝）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-env/src/**.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/env.rs` | 只取 `ExecutionEnv = FileSystem + Shell` 与 `FileChange`；**不带走** InMemory / 变更队列 / 工具实现 | 待提货 |
| `crates/nous-env/src/error.rs` | rpi@v0.24.2 | `crates/rpi-tools/src/error.rs` | 只取 `FileError` / `FileErrorCode` / `ExecutionError` | 待提货 |

## 三、自己 port（官方 pi）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-chord/src/json.rs` | pi@v1.5.2 | `packages/chord/src/json.ts` | 翻译；`JsonRepresentation` 落到 serde 类型约束，不留 `any` 旁路 | 待提货 |
| `crates/nous-chord/src/context/**` | pi@v1.5.2 | `packages/chord/src/context/index.ts` | 翻译 | 待提货 |
| `crates/nous-chord/src/output.rs` | pi@v1.5.2 | `packages/chord/src/output.ts` | 翻译；`pi-telemetry` → `nous-telemetry` | 待提货 |
| `crates/nous-chord/src/watch/**` | pi@v1.5.2 | `packages/chord/src/watch.ts` | 翻译；**`watchEvents` 视角不要**，事件出口接 `nous-wire` | 待提货 |
| `crates/nous-chord/src/sqlite/**` | pi@v1.5.2 | `packages/chord/src/sqlite/**` | 翻译；`node:sqlite` → `rusqlite`，`node:fs`/`node:path` → `nous-env` | 待提货 |
| `crates/nous-chord/src/delta/**` | pi@v1.5.2 | `packages/chord/src/delta/**` | 翻译；去掉 Promise；`Proxy`/descriptor 换显式类型 | 待提货 |
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
| `crates/nous-durable/conformance/storage_conformance.rs` | pi@v1.5.2 | `packages/durable/src/testing/storage-conformance.ts` | 改写成 Rust：`#[test]` ≥ 17（上游 33 个用例） | 待提货 |
| `crates/nous-durable/conformance/env_conformance.rs` | pi@v1.5.2 | `packages/durable/src/testing/env-conformance.ts` | 改写成 Rust：`#[test]` ≥ 11（上游 21 个用例） | 待提货 |
| `crates/nous-durable/conformance/storage_benchmark.rs` | pi@v1.5.2 | `packages/durable/src/testing/storage-benchmark.ts` | 改写成 Rust：`#[test]` ≥ 6（上游 12 个用例） | 待提货 |
| `crates/nous-durable/conformance/commit_writes.rs` | pi@v1.5.2 | `packages/durable/src/testing/commit-writes.ts` | 改写成 Rust：辅助 + ≥ 1 条 `#[test]` | 待提货 |
| `crates/nous-durable/conformance/assertions.rs` | pi@v1.5.2 | `packages/durable/src/testing/assertions.ts` | 改写成 Rust 断言门面 | 待提货 |
| `crates/nous-durable/conformance/types.rs` | pi@v1.5.2 | `packages/durable/src/testing/types.ts` | 改写成 Rust 用例/断言类型 | 待提货 |
| `crates/nous-durable/conformance/runner.rs` | pi@v1.5.2 | `packages/durable/src/testing/runner.ts` | 改写成 Rust 注册器 | 待提货 |
| `crates/nous-durable/conformance/mod.rs` | pi@v1.5.2 | `packages/durable/src/testing/index.ts` | 改写成 Rust 出口 | 待提货 |

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
