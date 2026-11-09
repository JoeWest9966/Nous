# d6s4v1 —— storage 的 X 光机

把一块的 `data/agent.sqlite`（pi-durable 的库）摊开给你看：字节长在哪儿、会话怎么分叉、
每次调用模型看到了什么、花了多少 token、有没有活卡住。

**只读。** 不写、不建索引、不 VACUUM。这页要是能手滑写库，它就是 pi-durable 的第二个写者，
而 WAL 只有一个写者。想修库？命令在「⑤ 维修」里，复制走自己跑。

一句话：`sqlite3` 给你一坨 JSON，这一页给你「按时间往回倒」。

## 它看谁的库

| 来源 | 说明 |
|---|---|
| 自己的 `data/agent.sqlite` | 默认，最正当的那个病人 |
| 邻居块的 `map/<id>/data/agent.sqlite` | **旁路读盘**：能读，页面上明确标着，`XRAY_SIBLINGS=0` 关掉 |

邻居那条是妥协，不是设计。铁律是「块之间只走 HTTP」，但对面今天还没有 `/__nous/ledger`
这个端点 —— 等谁做了，这个口子就该收窄成只认端点。**永远只读、永远标出来**，
这两条不妥协。

发现逻辑在 `discover()`：候选必须真的长着 `durable_metadata` 这张表才算数。
光看文件名会把「不是 pi-durable 的 sqlite」也当病人，然后在第一屏炸掉。

## 端点

```
GET /healthz
GET /api/targets               能看哪几个库（自己的排第一）
GET /api/overview?db=          文件 / pragma / 每张表的字节与行数 / entries 的 kind 分布
GET /api/conversations?db=     会话树（owner_conversation_id 就是那棵树）
GET /api/conversation?db=&id=  一条会话的 entries + 「此刻模型看到了什么」+ context 曲线
GET /api/prompt?db=&id=&upTo=  只取 prompt 重建
GET /api/cost?db=              tokens / 钱：按天（不足两天退到按小时）、按模型、按会话
GET /api/health?db=            卡住的任务、未回答的输入、base/delta、WAL、维修工单
GET /…                         public/ 里的静态文件
```

`db=` 只接受 `/api/targets` 里出现过的 id —— **不开任意路径读取的口子**。

## 页面的五屏

| 屏 | 回答什么 | 图 |
|---|---|---|
| ① 解剖 | 这 400KB 花在哪儿 | treemap（dbstat 真页数）、er |
| ② 会话树 | 谁从谁身上分叉、什么时候说过话 | gitGraph、timeline |
| ③ 回放 | 这条会话说了什么、那一刻模型看到什么 | xychart（上下文增长）、sequence |
| ④ 账单 | token 从哪来、到哪去 | xychart、pie、sankey |
| ⑤ 维修 | 有没有卡住的活、能不能瘦 | kanban、quadrant、pie |

「此刻模型看到了什么」不是模拟：条目就是库里存的那一条条 record，只是把
`input + cacheRead` 这类真值补上去。点 entries 表里任意一行就是一次回放。

## 数据从哪来

只有一处：`entries.record` / `tasks.record` / `documents.record` 里的 JSON，加上
`dbstat`、`pragma`、`fs.stat`。**没有第二份事实源**，不落快照、不缓存成文件
（`discover()` 的 5 秒缓存只是为了别让一个页面打六次盘）。

几个容易看错的地方，代码里已经处理，改的时候别改回去：

- `tasks.kind` / `documents.kind` 在盘上是 JSON 字面量（`"pi.generation"`），
  显示前要 `unquote()`，否则表格里带着引号。
- `tasks.record.state.status` 才是真状态，表上的 `status` 列是它的副本；
  两者不一致时以 record 为准（`health()` 读的是列，但 sample 从 record 取起止时间）。
- `entries.head` 是压缩点，压缩过的会话里 entries 会分成几段，**不代表丢数据**。
- `usage.cost` 全是 0 很常见：provider 不回 price，pi-durable 就记 0。
  `tokens` 才是真数，别把 0 当成免费。

## 图：pele（Mermaid 12.1 → SVG）

`public/vendor/pele.global.js` 是 vendored 的 pele 1.12.0（MIT，见同目录 `pele.LICENSE`）。
全局 `pele.mount(el, source, { zoom: "auto" })`。图都是**服务端拼源码**（`diagrams.js`），
浏览器只管渲染 —— 这样同一张图能在 node 里先验一遍。

`npm run check`（`dev/check.mjs`）会把每张图真的渲染一遍。**Mermaid 语法错在浏览器里
只表现成「一张图不见了」**，所以改完图务必跑一次。

踩过的三个坑，都写在 `diagrams.js` 里了：

1. `quadrantChart` 的坐标一旦等于 `1.0` 就词法错误 —— 一律压到 `0.999` 以下。
2. `gantt` 会把每根条拉满（`start` 被吃掉），所有条一样长 = 没画，所以它被换成了 quadrant。
3. `timeline` 的时间里不能有冒号（`06:23` → 解析崩），写成 `06.23`；
   而且它天生横着长，12 句就是 2000px，所以「会话内时间线」改成「会话的时间线」。

## 跑起来

```bash
npm start                 # PORT 由宿主给，本机自己跑默认 4782
XRAY_SIBLINGS=0 npm start # 只看自己的库
npm run check             # 需要服务已在跑
```

Node ≥ 22.19：`node:sqlite` 在 20.x 上不存在。`dbstat` 虚表在这个 Node 上是可用的，
页面依赖它 —— 不依赖的话字节会退化成「按 JSON 长度估」，那就不值得看了。

## 还没做的

- 没有 `/__nous/ledger` 的消费端：邻居那个口子现在读的是文件，等端点出现要改成 HTTP。
- 没有趋势：历史只有当下这一份快照。要做「这周涨了多少」，得自己按天落一条记录
  （落 `data/` 底下，别落仓库根）。
- 「把老会话归档到 storage-Archive」只给了命令，没做成一键：那是对数据的写操作，
  该由人按下去。
