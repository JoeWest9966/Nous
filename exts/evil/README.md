# `exts/evil` —— 判据载体，不是测试夹具

它进仓、有清单、有入口，**永远不许装载成功**。每一种坏能力由 `<目录>/mode` 里的一个词选出来
（默认 `oversize-frame`）。每条都对应附录 A 的 EX 编号；从目录里拿掉一种坏能力，对应的 EX
就失去载体、当场变 SKIP（SKIP 不是绿）。

| mode | 坏能力 | 期望（拒绝的形状） | EX |
|---|---|---|---|
| `oversize-frame`（默认） | 握手后发一个 2MB 单帧 | 宿主在读入前拒、杀掉、归因 `extension_crash`，RSS 涨幅 0 | EX-58 |
| `rss` | 吃 300MB RSS | 撞 256MB 硬顶被杀，归因 `extension_crash` | EX-36 |
| `protocol4` | 自报协议 4 | 拒载、报双方版本，旧进程不动 | EX-06 |
| `exit-after-hello` | 握手后立刻退出 | 重载失败留旧；开机失败落提交 + 推 SSE | EX-41 / EX-66 |
| `crash` | 收到 section 就退出 | 第一次崩 = `degraded`；第二次 = 进程熔断 | EX-28 / EX-32 |
| `slow` | section 前睡 1.5s | 撞 1s 总预算，归因 `timeout`（非崩失败） | EX-29 |
| `conflict` | 自报 `skill:tdd`（与 `nameplate` 冲突） | 后装拒载并指名双方，先装的照常工作 | EX-59 |
| `no-nameplates` | 一名牌都不报 | 名牌全消失、`missing()` 逐个点名 | EX-23 / EX-56 |
| 工具 `evil_write` | 越界写（按 `args.path` 原样打开） | 非宿主 uid 时拿到 `EACCES`，目标不存在 | EX-45 / EX-73 |
| 工具 `evil_mutate` | 申请第四种写形状（非 `entry`/`document`） | 变成一条 error result 回给模型，不落库 | EX-22 |
carriers: EX-45,EX-58,EX-60,EX-67,EX-71

| 工具 `peek` | 读任意路径（用来演示 `/proc/<pid>/fd` 泄漏的红法） | 读不到句柄值；`/proc/<pid>/fd` 里没有值 | EX-26 / EX-46 / EX-47 |

符号链接（EX-67）与路径越界（EX-60）的载体在**内核代读侧**：往句柄值目录里放
`leak.md -> /etc/shadow` 或 `../../etc/passwd`，宿主必须在代读时就拒/跳过。
