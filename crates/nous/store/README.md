# store

> [中文](#zh) · [English](#en)

<a id="zh"></a>

## 1 它是什么 / What it is

中文：Nous **自己的**库（`nous.db`）—— 两张表（`triggers` / `fires`）+ 一条自己的迁移链。
触发器是 Nous 的实体（插件造出来的「商品」），不是 pi 的语义；`nous-durable` 的
session / entry / document / submission / task 里没有这个概念，所以它是**第二个文件**，
不是 durable 的第五张表。它不负责：提交消息（那是 durable）、调度节拍（那是 `dispatcher`）、
端点（那是 `server`）、审计（等权限系统定稿）。

EN: Nous' **own** database (`nous.db`): two tables (`triggers` / `fires`) plus its own migration
chain. Triggers are Nous' entities, not pi's semantics — `nous-durable` has no such concept, so
this is a **second file**, not a fifth table. It does not own message submission (durable), the
scheduling tick (`dispatcher`), endpoints (`server`), or auditing (waiting on the permission model).

## 2 什么时候用 / When to use

中文：要「谁、什么时候、该触发什么」以及「这一次到底提交了没有」时用它。它的判据是一个
**可穷举的纯函数**（`reclaim`）和一张**账本**（`fires`）。不该用它存会话、entry 或任何
pi 语义 —— 那些进 `durable.db`。

EN: Reach for it when you need "who, when, and did this fire actually submit", i.e. the
exhaustively testable `reclaim` plus the `fires` ledger. Do not use it for sessions, entries or
any pi semantics — those belong in `durable.db`.

## 3 清单 / Manifest

由脚本从相邻 `plugin.toml` 生成，禁止手写。

Generated from the adjacent `plugin.toml` by the checker; do not hand-write.

## 4 API

> 待生成：见 #48 的 /v1/_routes（禁止手写占位表格）

## 5 权限 / Permissions

中文：无凭据、无 scope —— 单用户、本机、块与块互信（与仓里同一套前提）。**唯一的硬边界**是
「库的所有者只有一个」：别人想读 `nous.db` 走本 crate 的 API，不许自己开连接（判据 5 的
`sqlite-entry` 哨兵）。`Store::conn()` 是给判据用的裸连接，生产路径不许走它。

EN: No credentials, no scopes — single user, local machine, mutually trusted components. The one
hard boundary is single ownership of the file: everyone else goes through this crate's API instead
of opening a second connection (the `sqlite-entry` sentinel in criterion 5). `Store::conn()` exists
for tests; production paths must not use it.

## 6 失败模式 / Failure modes

中文：库比代码新 → `SchemaTooNew`，拒绝打开（不让写入落在看不懂的表结构上）。迁移跳号 →
`MigrationChain`。备份目标已存在 → 拒绝（不覆盖上一份）。`code` 复用、改 `code`、真 `DELETE`
→ 库层的触发器拒绝，不是「我们的 API 没提供」。提交崩在中间 → `fires` 留在 `pending`，
超期由 `reclaim` 扫成 `orphaned` 并重放（重放幂等，安全）。

EN: A newer database refuses to open (`SchemaTooNew`); a gap in the migration chain is
`MigrationChain`; an existing backup target is refused rather than overwritten. Reusing a `code`,
rewriting it, or hard-`DELETE`-ing a row is rejected by triggers **in the database**, not merely
missing from the API. A crash between "record" and "submit" leaves `fires` at `pending`; past the
grace period `reclaim` marks it `orphaned` and replays it (idempotent, safe).

## 7 判据 / Judgement

中文：`cargo test -p nous-store`（判据 2/3/4/5）+ `./scripts/check-separation.sh`（判据 1/6/7）。
判据 4 的 `reclaim` 是穷举的：三状态 × 宽限期边界前后；判据 3 的负向哨兵是「去掉 PRIMARY KEY
→ 行数 2」；判据 5 的负向哨兵是「改成真 `DELETE` → 行没了、`code` 可复用」。

EN: `cargo test -p nous-store` (criteria 2/3/4/5) and `./scripts/check-separation.sh`
(criteria 1/6/7). `reclaim` is tested exhaustively over the three states crossed with offsets
around the grace boundary; criterion 3's negative sentinel is "drop the PRIMARY KEY → two rows";
criterion 5's is "turn delete into a real DELETE → row gone, code reusable".

## 8 反悔点 / Reversal

中文：版本号与库文件名可反悔（加迁移不会改历史；改名 = 搬文件 + 改部署脚本）。**不可反悔**的
是 `code` 的不可变与墓碑：一旦有人复用了 `code`，历史 `requestId` / `actor.id` / 审计就指向
另一个触发器，那是事故。**一个已做的选择**：`reclaim` 也重放已经是 `orphaned` 的行（不只
`pending` 超期的）—— 只认 `pending` 会在「标了 orphaned、还没重放」那个窗口把触发永远丢掉。
如果哪天调度循环决定自己记「已重放过」，可以把它收回去，但要在 `docs/decisions.md` 留痕。

EN: The version number and the file name are reversible. What is **not** reversible is `code`
immutability and the tombstone: reusing a code makes historical `requestId` / `actor.id` / audit
records point at a different trigger. One recorded choice: `reclaim` also replays rows that are
already `orphaned`, because only handling overdue `pending` rows would lose a trigger in the
"marked but not yet replayed" crash window.

<a id="en"></a>
