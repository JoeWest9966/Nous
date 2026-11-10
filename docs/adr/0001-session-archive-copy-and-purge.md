# 会话归档：复印 + 撕原件，agent.sqlite 永不关

qrd3wh 需要"删除会话"，但 pi-durable 在会话粒度没有任何删除 API（ConversationRecord 无墓碑字段、Tx 无 delete、record 无 retire 路径），而 agent.sqlite 的活 Session 绝不能关（用户红线，长驻服务）。

决定把"删除"拆成两个在线动作：

1. **归档 = 复印**：第二个**只读**连接扫出目标死子树的行（九张表、原样、id 不变），写进 `data/archives/gen-YYYYMMDD.sqlite`（同 durable schema 的独立文件，按天一个）；同时在 `Sessions` 文档写墓碑（`archived`），列表即不可见。
2. **真删 = 撕原件**：**纯手动**（用户决定，不自动兜底），条件苛刻——必须已复印、会话必须 idle、活库中不能有活着的 fork 后代；第二个连接事务 DELETE 九张表里它的全部行 + 墓碑记 `purged`。

打捞 = 原样 INSERT 回活库。**SPIKE 已验证（2026-11-10，临时库）**：撕掉和捞回的全程 Session 活着不炸、`tx.conversation()` 和 `scanEntries()` 正常、id 原样保留、捞回后 `appendEntry` 照常落新 entry。

## Considered Options

- **转写到第二个 durable Session**（appendEntry 重放）：ID 不可复用、fork 链断、task/submission/doc 全丢 —— 是副本不是原件。否。
- **整库轮转**（旧库改名、开新库）：粒度太粗，等于关 Session。否。
- **关窗手术**（close → 搬行 → reopen）：用户红线 —— agent.sqlite 绝不能关。否。
- **纯软删除**（墓碑 + 视图过滤）：小资源服务器上必须真瘦身。否。
- **维护窗打捞**（重启间隙执行）：SPIKE 证明在线 INSERT 可行，不需要。否。

## Consequences

- 我们在 durable 的表布局层（九张表）动刀，低于官方 API；pi-durable 钉死 1.4.0 + 每次操作前校验 `durable_schema` 版本兜底。
- DELETE 不缩文件，真缩要 VACUUM（静默时刻或重启间隙）—— 行删了和文件瘦了是两件事。
- 墓碑记归去了哪个 gen、原件是否已撕；浏览端点据此如实显示"能捞 / 捞不了 / 已销毁"。
- 归档单元 = 死子树闭包：带活后代的会话拒绝归档（"先处理它的活分支"），与锁"只锁自己一个"同一脾气。
- 端点词汇沿用扩展那套三动词：`archive / restore / purge`（server.js extensions 段同款）。
- 归档库文件住 qrd3wh 自己的 `data/archives/`；mcfpqm（storage-Archive）纯遥控器，走 HTTP 调引擎，不碰文件。
