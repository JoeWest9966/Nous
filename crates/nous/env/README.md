# nous-env

`ExecutionEnv` 契约层：`FileSystem` + `Shell` 两张 trait 面 + `Context` + 两份错误类型 +
四条私库路径解析（`content.rs`：#63）。

- **私库路径**：`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` / `NOUS_EXTEND` 一处解析，
  默认值分别是 `/opt/Nous-skills` / `/opt/Nous-prompts` / `/opt/Nous-Template` /
  `/opt/Nous-extend`（与 TS 版 `server.js:77-83` 对齐）。只查环境、只给路径，不碰盘。
- **形状来源**：`packages/durable/src/env/index.ts`（TS）。理由不是「哪份更全」，是调用点
  数量：durable 侧 116 处 vs rpi-tools 侧 27 处。逐行出处见仓库根 `SOURCES.md`。
- **依赖**：只有 `async-trait` + `tokio-util`。无内部依赖（AGENTS.md §二）。
- **三档能力**：必需 14 个（无默认实现）、可选 12 个（默认 `not_supported`，调用方必须显式
  降级）。清单在 `FileSystem` 的 rustdoc 里。
- **不带 IO**：本地实现、SSH / 远端实现、测试用 InMemory 都在这个 trait 之下。
- **不带 JSONL 专用那四样**：追加句柄 / 写者租约 / 目录 fsync / 自定义解释器。台账各记一行
  待提货并写明理由 —— 本库写 SQLite。
- **对上游唯一的结构性改动**：所有方法统一收 `&Context`（`CancellationToken` + 可选进度
  sink），改一次，不散进调用点。
