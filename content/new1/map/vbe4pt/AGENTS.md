# extend（vbe4pt）· Nous 自研扩展的清单前端

一页只读视图：**`Nous-extend` 仓库里有什么** + **正在跑的 qrd3wh 进程里装着什么**。
以前是 pi CLI 扩展的管理器（对账 / 收编 / 装卸 / 快照 / 出厂检查），那套跟 pi-durable 无关，
2026-11 整条退场。代码在 git 历史里，运行时存档在 `data/legacy-pi/`（没有代码再读它）。

## 为什么只剩发静态文件

数据不走这个进程。页面自己去问 qrd3wh 的 `GET /v1/extensions`，地址从宿主的
`__nous/ctx` 里找 `qrd3wh` 那个块拿。所以 `server.js` 现在只做一件事：把 `public/` 发出去。

块之间只走 HTTP，地址只从宿主的 ctx 来 —— 这一块里一个 `/p/…` 都不拼。

## 两套扩展，别混（这一页只管下面那套）

| | pi CLI 扩展 | Nous 扩展 |
|---|---|---|
| 形态 | 磁盘上的 `.ts` 文件 | 进程内对象 `{name, tools?, sections?, hooks?, wraps?}` |
| 真相在哪 | 安装位（`~/.pi/agent/extensions/` 等） | `registry.install()` 之后的内存 |
| 启停 | 改名 `x.ts` ↔ `x.disabled.ts` + `/reload` | 一次 `install()`，同名就地替换，不重启 |
| 版本 | 文件时间戳 | git commit（`extensions.lock.json` 钉着） |

给下面那套套「改名 / reload」的开关会得到能点但不起作用的东西 —— 所以这一页**不给按钮**。

## 页面上每一栏在说什么

- **lock / HEAD**：`extensions.lock.json` 钉的提交 vs 仓库 HEAD。不一致就是红的 ——
  这是「改了仓库但没重启，跑的还是旧代码」唯一能看见的信号。
- **我们开发的**：仓库 `index.json` 里的扩展，逐条列提示词段、要宿主喂什么、哪几条会话在用。
- **`仓库里有，进程里没装`**（虚线红边）：菜谱上有、厨房里没有。扫目录永远看不出这种漂移，
  这一页存在的理由大半是这个。
- **不归这一页管**：pi-durable 自带的（`coding-tools`）和技能名牌（`skill:<名>`）。
  列出来只为让人知道它们存在，不是漏了。谁写的代码谁进账，自带的就不该有装卸按钮。

## 接口

```
GET  /                静态（public/index.html）
GET  /app.js  /style.css
```

别的一律 404 —— 这一块没有后端状态，`data/` 里除了 `legacy-pi/` 那份存档什么都没有。

## 不做的事

- **不装卸。** 那要先回答「一会话一容器时 registry 归谁」：每容器一个 registry（这页是纯制品库），
  还是常驻引擎持有 registry（容器只是视图）。没定论之前不做假的。
- **不读仓库文件来回答「装着什么」。** 仓库里写着什么 ≠ 这一刻装着什么。
- **不碰 pi CLI 的扩展。** 那是另一个世界的东西，见上面的表。
