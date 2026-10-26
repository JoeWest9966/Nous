# Nous

## 块和组的字段

**块（leaf）和组（section）是同一个类型 `Node`，没有类型字段。** 判据是 `children` 在不在：
`isSection(n) = Array.isArray(n.children)`（`src/game.ts:20`）。

### 存进 `tree.json` 的（就这四个）

| 字段 | 类型 | 块 | 组 | 说明 |
|---|---|---|---|---|
| `id` | `string` | 必有 | 必有 | 唯一引用键。**6 位随机，如 `7kq2md`**：字母表 32 个（去掉 `i l o u`），`freshId()` 发，`newId()` 在整棵树里查重、撞了就重掷。**新建那一刻定型，改名、换爹、撤销都不动它**。这个字段搬过两次家（`id` → `tree` → `id`，值一次没变），老盘两个名字都认：`adoptIds()`（`store.ts`）载入时接住，下一次落子按新名写回 |
| `name` | `string` | 必有 | 必有 | 人看的那层皮。改名不动 `id`（`renameNode` 只改 `name`） |
| `value` | `number?` | 必有 | **不许有** | 面积就是它。组的值由子节点相加得出，写进去就是两份事实源 |
| `children` | `Node[]?` | **不许有** | 必有（可为空数组） | 唯一的结构字段，同时兼任「是不是组」的判据 |

就是这么四个。没有 `type`、没有 `parent`、没有 `order`、没有 `x`/`y`/`w`/`h`、没有 `collapsed`、没有时间戳 —— 也没有 `tree`：**路径是算出来的,不存盘**(见下表)。

### 派生、不存盘的三样

| 东西 | 怎么算 | 在哪儿 |
|---|---|---|
| `valueOf(n)` | 块返回 `value`，组返回子节点之和 | `game.ts:17` |
| `tree` | 名字路径 `N/src/App.tsx`，同级重名按遍历顺序加 `-2`。改名、搬爹它就变,每次从树现算，**不进 json** | `treeMap()` `game.ts:120` |
| `pathTo` / `childIds` | 钻到某节点的组路径（一串组 id）/ 某组底下的顺序 | `game.ts` |

布局（谁在哪、多大）也是派生的：`layout()` + `squarify()` 从值算出来，丢了重算一次就有。
存它等于把衍生物当事实源。

### 盘上长什么样

```
content/<board-id>/
├── tree.json   { v, name, version, updatedAt, root }   ← 语义唯一真相，丢了就是真丢
├── log.json    { v: 1, moves[] }                        ← 最近 200 手，丢了只是没历史
│                                    move 里指节点的字段也叫 `tree`（老记录是 `id`，只当历史看）
└── map/<nodeId>/
    ├── agent.md          ← 这一块的正文。用户的，程序只在建文件夹时写一次，之后永不覆盖
    ├── README.md         ← `map/` 的索引（id ↔ tree ↔ 名字），每次保存整份重写，纯派生
    └── .gone/<nodeId>/   ← 树里已经没有的节点，连文件夹挪到这儿，**不删**
```

`map/` 的文件夹名是 **id**，不是名字：改名、换爹、撤销都不碰文件系统。对账在 `server/map.ts`，
保存后跑一次，幂等。

### 块自己的页面

```
/p/<board>/<nodeId>/          ← 真实路径（不是 hash）。冷启动、粘贴、后退都认它
GET /api/boards/<board>/nodes/<node>/doc   → { text, etag, stub }
```

四条不变的：

- **正文不进 `tree.json`。** 它有自己的文件、自己的 `etag`（内容哈希），**不参与 board 的 `version`** ——
  写一篇文章不该把树顶成 409，落一手棋也不该让笔记冲突。
- **树里没有的节点不给看。** 文件夹可以躺在 `.gone/` 里，旧书签不能顺着 id 把它读活。
- **文件不在就给 stub，但不落盘。** 看一眼不该长出一个文件。
- **页面盖在盘面上，不卸载盘面。** 卸载会丢掉树的展开状态、滚动位置和撤销栈；
  被盖住的区域用 `inert`，键盘别穿透下去。

### 有代码的块 = 一个进程

同一个 URL 空间里两套东西，判定**只看文件夹里有什么**（幂等，不往 `tree.json` 写任何东西）：

| 文件夹里 | `/p/<board>/<id>/` 归谁 |
|---|---|
| 只有 `agent.md` | 壳：文档页 |
| + `package.json` 里有 `scripts.start` | 它自己：剥掉前缀反代到它的进程 |

进程的契约就四条，都是硬的：

- **听 `PORT`。** 宿主给的空闲端口，不写进任何文件。
- **数据写 `NOUS_DATA`**（= 它自己文件夹下的 `data/`）。不是宿主替你存 —— 是「`cp -r` 走了数据也走了」。
- **`__nous/` 不许碰。** 前缀底下的别处全归你，只有这一段永远归宿主（笔记页、`__nous/ctx`）。
- **链接写相对路径。** 这是唯一能让 `cp -r map/<id> ~/<id> && npm start` 和宿主里打开
  是同一页的写法。宿主不重写 HTML、不改你的绝对路径，`/style.css` 就是逃出前缀。

宿主这边：崩了给墓碑（stderr 最后几行）+ 502，**只坏这一页**；文件夹里的代码一改就收掉进程
（下次请求带着新代码起来，不自动重启 —— 崩得勤的块会把机器烧穿）；`scripts.start` 改了同理；
闲 10 分钟退出。`npm install` 不归宿主管：块要依赖就把 `node_modules` 放进去。

给的四个环境变量，就这四个：`PORT`、`NOUS_DATA`、`NOUS_ORIGIN`（宿主地址）、
`NOUS_SELF`（我在宿主里叫什么，`/p/<board>/<id>`）。

### 块之间怎么说话

```
GET <NOUS_ORIGIN><NOUS_SELF>/__nous/ctx          → { board, host, base, node, blocks[], resolve }
GET …/__nous/resolve/<id>[?path=/底下]           → { url }   ← 绝对地址
```

`ctx.blocks[id].url` 就是邻居的地址，拿到就能电话。**两条铁律：**

- **只走 HTTP。** 不许 import 别人的代码，不许读别人的文件夹，不信任别人的 `data/`。
  按路径自己拼 `/p/<board>/<id>` 也不许 —— 地址只有一个来源，就是 `resolve`。
  理由不是洁癖：某块明天搬去自己的域名或容器，改的是宿主那张表，不是所有块。
- **没有凭据。** 单用户、内网、块与块互信。这里没有 token、签名、scope。
  要多人、要跑不可信代码那天，先上进程/容器边界，再谈身份 —— 顺序反了全是白工。

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.
