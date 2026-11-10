# mcfpqm —— 归档库

一页只干一件事：把 qrd3wh 的归档库摊开 —— 哪天归的、里面有哪些会话、哪些原件还在、
哪些已经撕了 —— 并且只在这一页给「撕原件」和「打捞」两个按钮。

**为什么撕原件只放这儿**：主列表（qrd3wh 左列 / pxbgng）永远不出现「彻底删除」，
得先翻进归档库、看见复印件确实在那儿，才有撕的入口。反过来也一样 —— 要捞回一条
早就不在列表里的对话，只能来这儿。

## 数据从哪来

不碰 qrd3wh 的 `data/archives/*.sqlite`，也不读它的 `agent.sqlite`。全部走 HTTP：

| 用途 | 请求 | 回什么 |
|---|---|---|
| 地址 | 宿主的 `__nous/ctx` → `blocks[].url`（页面上一个 `/p/...` 都没拼） | — |
| 归档库 | `GET  <qrd3wh>/v1/archive` | `{gens:[{gen, items:[{id, name, parent, entries, last, archived, live}]}]}` |
| 撕原件 | `POST <qrd3wh>/v1/sessions/:id/purge` | `{purged:[id], gen, rows}` |
| 打捞 | `POST <qrd3wh>/v1/archive/:id/restore {gen}` | `{restored:[id], gen, rows}` |

`gen` 是 `YYYYMMDD`（一天一个归档文件）。`archived` 是墓碑 `{at, gen, purged}`，
`live` = 活库里还有没有原件。四态由这两个字段推：

| 状态 | 判据 | 行上显示 | 操作 |
|---|---|---|---|
| 原件在 | `live && archived && !purged` | 原件在 | 撕原件 |
| 已销毁 | `archived.purged` | 已销毁 | 打捞 |
| 已捞回 | `live && !archived` | 已捞回 | — |
| 无墓碑 | `!live && !archived` | 无墓碑 | 打捞 |

硬条件全在 qrd3wh 那侧拦（忙、还有活分支、复印件不完整、重复打捞 → 409），
这一页只负责把错误原文显示出来 —— **不要在这页再判一遍**，两处规则迟早会分叉。

## 改它的注意

- 全是静态文件，没有 `server.js` —— 宿主的内置空跑 server 直接端出去。
- 归档、改名、删会话都在 qrd3wh 做；这一页只负责看和捞/撕。
- 轮询 5 秒，且页面藏起来（`visibilityState`）就不问 —— `GET /v1/archive` 每次要
  打开所有 gen 文件，别更勤。
- 归档条数一多，`/v1/archive` 会变慢（每个 gen 文件都要开一次 + 扫一遍活库会话）。
  真到那一步再给 `?gen=` 过滤，现在不加。

## 外观从哪来

跟 pxbgng（对话列表）同一套皮，因为它们是同一个树里的兄弟：16px Inter、28px 行距、
20px 缩进、1px 引导线、`#1c1b1a` 底、`#cecdc3` 字。差异只有三处：

- 一天一个 `.gen` 段头（日期 + 条数 + 「今天」徽章）
- 已销毁的行整体压暗（`.gone`：图标和名字褪色，标签转 `--warn`）
- 行操作按钮平时 `opacity:0`，指上去才现形 —— 这一页平时是「看」的
