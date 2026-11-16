# SOURCES · 提货台账

一行一个来源单元。四列 + 状态。**判据：每一行都能用 `git -C <来源> show <tag>:<路径>` 对上；对不上的行是错的 —— 改台账或删代码。**

状态只有三个值：

- `待提货` —— 计划里，还没抄进来
- `已提货` —— 抄进来了，和上游一致
- `已改` —— 抄进来了，本库改过（改动写进第四列，一句说清）

来源代号：`rpi@v0.24.2` = `bigfish1913/pi-rust`；`pi@v1.5.2` = `earendil-works/pi`。

## 一、冻结拷贝（pi-rust）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-ai/**` | rpi@v0.24.2 | `crates/rpi-ai/src/**` | 原样 | 待提货 |
| `crates/nous-ai/src/providers/google_generative_ai.rs` | pi@v1.5.2 | `packages/ai/src/api/google-generative-ai.ts` | TS→Rust 翻译（rpi 缺这一个方言） | 待提货 |
| `crates/nous-tools/**` | rpi@v0.24.2 | `crates/rpi-tools/src/**` | 原样 | 待提货 |
| `crates/nous-runtime/**` | rpi@v0.24.2 | `crates/rpi-runtime/src/**` | 原样 | 待提货 |
| `crates/nous-telemetry/**` | rpi@v0.24.2 | `crates/rpi-telemetry/src/**` | 原样 | 待提货 |
| `crates/nous-wire/src/event.rs` | rpi@v0.24.2 | `crates/rpi-cli/src/remote/protocol.rs` | 只取事件枚举与形状；帧/CBOR/客户端那套不要 | 待提货 |
| `crates/nous-durable/src/tokens.rs` | rpi@v0.24.2 | `crates/rpi-harness/src/compaction/tokens.rs` | 原样 | 待提货 |
| `crates/nous-durable/src/retry.rs` | rpi@v0.24.2 | `crates/rpi-harness/src/agent_harness.rs`（retry 段） | 摘录一小段 | 待提货 |

## 二、自己 port（官方 pi）

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `crates/nous-chord/src/json.rs` | pi@v1.5.2 | `packages/chord/src/json.ts` | 翻译 | 待提货 |
| `crates/nous-chord/src/delta/**` | pi@v1.5.2 | `packages/chord/src/delta/**` | 翻译 | 待提货 |
| `crates/nous-chord/src/sqlite/**` | pi@v1.5.2 | `packages/chord/src/sqlite/**` | 翻译 | 待提货 |
| `crates/nous-chord/src/watch/**` | pi@v1.5.2 | `packages/chord/src/watch/**` | 翻译 | 待提货 |
| `crates/nous-chord/src/output/**` | pi@v1.5.2 | `packages/chord/src/output/**` | 翻译 | 待提货 |
| `crates/nous-chord/src/context/**` | pi@v1.5.2 | `packages/chord/src/context/**` | 翻译 | 待提货 |
| `crates/nous-durable/src/storage/**` | pi@v1.5.2 | `packages/durable/src/storage/**` | 翻译；**表结构与版本号自己定**（见 `docs/storage.md`） | 待提货 |
| `crates/nous-durable/src/session/**` | pi@v1.5.2 | `packages/durable/src/session/**` + `entries.ts` + `documents.ts` | 翻译 | 待提货 |
| `crates/nous-durable/src/harness/{generation,abort,live,partial_changes}.rs` | pi@v1.5.2 | `packages/durable/src/harness/` 对应文件 | 翻译；**watch 那套不要**，事件走 `nous-wire` | 待提货 |
| `crates/nous-durable/src/record/**` | pi@v1.5.2 | `packages/durable/src/types.ts` | 只摘记录版本语义（`pi.tool` / `pi.generation` / `task_too_old`） | 待提货 |
| `crates/nous-durable/conformance/**` | pi@v1.5.2 | `packages/durable/src/testing/**` | 改写成 Rust 验收套件 | 待提货 |

## 三、只用不改

| 本库路径 | 来源 | 上下游路径 | 改动 | 状态 |
|---|---|---|---|---|
| `vendor/pi-env-daemon/**` | pi@v1.5.2 | `packages/env/daemon/**` | 原样，**只 build 不改** | 待提货 |
| `docs/spec-from-pi-durable.md` | pi@v1.5.2 | `packages/durable/docs/spec.md` | 摘录 + 逐条"采纳 / 不采纳" | 待提货 |
| `docs/pitfalls-from-pi.md` | pi@v1.5.2 | `packages/durable/CHANGELOG.md` | 只追加判定 | 进行中 |
| `wire.md` | NousDesk@`wire.md` | 桌面端仓根 `wire.md` | 迁入并改为权威版本 | 已提货 |

## 四、不在本库（写下来是为了以后不用再讨论）

```
chord:   host / node / store / remote / surfaces / tui / web / react / sync /
         jobs / link / contracts / registries / data / state / telemetry-ingest   ≈ 5.5 万行
durable: codemode / images / tool-search
pi:      packages/{ai, agent, coding-agent, tui, mcp, schema, telemetry, codemode, evals}
pi-rust: crates/{rpi-harness, rpi-cli, rpi-tui, rpi-extensions, rpi-plugin-sdk}
         （只有 tokens.rs / retry 段 / protocol.rs 三处例外，见上表）
```
