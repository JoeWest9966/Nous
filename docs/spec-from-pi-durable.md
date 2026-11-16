# 从 pi-durable 采纳什么、不采纳什么

上游 `packages/durable/docs/spec.md`（7,296 行）+ 1.4.0→1.5.2 的 changelog，是**设计输入**，不是实现蓝图。这份表是"我们采纳哪几条、为什么、什么时候反悔"。

采样基准：`pi@v1.5.2`。上游自称 **Experimental，API 不预告就改**。

## 采纳

| 决定 | 上游怎么说 | 我们怎么落 | 反悔点 |
|---|---|---|---|
| **先落库、后可见** | 任何东西在展示前先提交进存储 | `nous-durable` 的写入顺序；SSE 能看到的必须库里先有（`docs/storage.md` 硬约束 2） | 如果某类中间态落库代价 >1000ms/次，改成延后落库并写清丢失窗口 |
| **崩溃恢复** | 重开存储接上；死在半路的模型请求算一次失败尝试并重试 | 启动时扫未完成提交；重试有上限 | 上限调到 0（不重试）要写进这里 |
| **进度延后落库** | `progress.persistWithinMs` 默认 1,000ms，崩溃丢约 1 秒流式内容，换来写盘次数降一个量级 | 我们取同一个量级；具体值写进 `docs/storage.md` | 有用户抱怨"崩了丢一坨"就降到 100–200ms |
| **abort 要能撤销排队** | `onAbort: "withdraw"`（默认，未答的输入结算为 `aborted`）/ `"keep"`（留着下一轮答） | 我们的 abort 先做 `withdraw` | 出现"我按了停但下一条自己跑了"就考虑 `keep` |
| **会话树 = `parent {id, at}`** | fork 在 entry 上分叉 | 已在 `wire.md` 钉住 | — |
| **按会话的 compaction 设置** | `configure({ compaction })` 存在 `pi.agent`，子会话与 fork 继承 | 我们存在会话记录里，字段进 `wire.md` 再实现 | 只剩全局一份能省事就改 |
| **记录版本 + 任务过旧判定** | `pi.tool` v4 / `pi.generation` v3；新版写的库旧版不打开，旧版看到会 `task_too_old` | 采纳**思路**：记录带版本、任务带最低版本；**版本号自己定** | 我们的版本号和上游对齐时（真要互通）才抄数字 |
| **存储一致性测试套件** | `src/testing/**`（3,444 行）：`storage-conformance` / `env-conformance` / `commit-writes` / 基准 | 改写进 `crates/nous-durable/conformance/`，是 `verify.sh` 的一部分 | 不停 |
| **只依赖序列化字段名** | 上游在 `record` 里存 pi-ai 的 `Message` JSON | `nous-ai` 的 `camelCase` 对齐 `wire.md` | — |

## 不采纳（并写下反悔点）

| 不采纳 | 上游的东西 | 理由 | 反悔点 |
|---|---|---|---|
| **状态视图取代事件流** | 1.5.0 删了 `watchEvents`，改 `Conversation.watch()` + `ConversationView`（窗口 50/100） | 我们的客户端（桌面 / 手机）都是事件驱动 + SSE 合帧，`wire.md` 整张表已经按事件钉住。改成状态视图等于三端重写 | 除非要做"纯渲染客户端"（无本地状态），才值得再评 |
| **watch 那套语义** | `WatchHandle` / `WatchEnd` / 变更折叠 / 超一万条根替换 | 事件流 + `chord::watch` 已经够 | 长会话（>10k 条）渲染出问题时再回来看 |
| **可挂起的工具（park）** | `execute()` 可返回 `{ sleep }` / `{ wait }`，配合 signals、gates、hook 任务 | 第一版单层任务够用；这套是上游最重的一块（1.5.0 的 75 条变更主干） | Nous 真要做"等外部事件"的工具（webhook / 人工审批）时，照这一节的语义补 |
| **signals** | `defineSignal` / `harness.signal()` / `readSignal` | 同上，没有 park 就不需要 | 同上 |
| **`codemode`（QuickJS 沙箱）** | `packages/codemode` + `durable/src/codemode` | qrd3wh 一行都没 import | 要做"模型写脚本调工具"时才评 |
| **`images` / `tool-search`** | `durable/src/{images,tool-search}` | 同上，未使用 | 支持图片输入时才评 |
| **`plans` / 多 provider SDK** | `packages/ai` 的 6 家 SDK | `nous-ai` 只留 4 方言 | 出现新方言时按 `AGENTS.md` 4.2 加 |

## 读上游的正确姿势

1. 先读 `CHANGELOG.md` 里 **`### Breaking Changes`** 段 —— 那是上游自己承认的语义变更，最值钱。
2. 再读 **`### Fixed`** 段 —— 每一条都问"我们有没有同一个毛病"，结论写进 `docs/pitfalls-from-pi.md`。
3. `spec.md` 当**问题清单**读：它回答的是"这件事有哪几个坑"，不是"代码该长什么样"。
