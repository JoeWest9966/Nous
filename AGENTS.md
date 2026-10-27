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
├── .nous/      ← 宿主的：locks.json（锁）、audit.log（谁想动锁着的块）
└── map/
    ├── README.md         ← 索引（id ↔ tree ↔ 名字），每次保存整份重写，纯派生
    ├── .gone/<nodeId>/   ← 树里已经没有的节点，连文件夹挪到这儿，**不删**
    └── <nodeId>/
        ├── AGENTS.md     ← 入口，agent 进来时自动读的那份。**宿主不铺**：你写第一个字那天它才出现
        ├── *.md          ← 这一块别的文档。文件夹就是清单，能建能删
        ├── .trash/       ← 删掉的文档躺这儿，**不真删**
        ├── package.json  ← **你自己建的**：有 `scripts.start` = 它是个 app
        ├── server.js     ← 你自己建的：它的后端
        ├── public/       ← 你自己建的：它的静态页面
        └── data/         ← **你的代码自己建的**：数据放这儿，`cp -r` 走就带走了（宿主不预建）
```

**这个文件叫 `AGENTS.md`，不叫 `agent.md`。** 一字之差决定 agent 会不会读它：
所有 coding agent 进来先找 `AGENTS.md` / `CLAUDE.md`，小写的 `agent.md` 谁都不会自动打开。
在这个文件夹里跑 `setup-matt-pocock-skills` 时，它编辑的也是这一份（它只就地更新 `## Agent skills`
那一段，不覆盖周围的内容）。万一写坏了没有 git 兜底 —— `content/` 是运行时数据，不在版本控制里；
要靠版本就自己 `git init` 到别处，或者用 `tar`/`rsync` 做备份。

**新建的节点（块和组都一样）只有一个空文件夹：没有 `AGENTS.md`，没有代码，什么都没有。**
所以它的「打开」是一张白板 —— 那不是坏了，是「还没写」。
`AGENTS.md` 不会预先出现：你点开笔记、写下第一个字，它才落成磁盘上的文件；那份从此归你。

这里曾经铺过一份脚手架（`package.json` + `server.js` + `public/index.html`，
一个带 `/api/notes` 的示例 app），**已经拆掉了**：谁都没写过的块，一出生就端出一个像模像样的
功能页，等于骗人说它有内容了。想让它动起来，写 `package.json` 的 `scripts.start` —— 就这一条。
宿主**不建任何东西**：`data/` 也不建（它只把路径塞进 `NOUS_DATA`，要不要这个目录、什么时候建，
块自己决定）。宿主写的文件只有一个：`map/README.md` —— 每盘一份的索引。其余全是你的。

`map/` 的文件夹名是 **id**，不是名字：改名、换爹、撤销都不碰文件系统。对账在 `server/map.ts`，
保存后跑一次，幂等。

### 块自己的页面

```
/p/<board>/<nodeId>/                       ← 真实路径（不是 hash）。冷启动、粘贴、后退都认它
/p/<board>/<nodeId>/__nous/doc             ← 文档页：map/<nodeId>/ 里那一堆 md

GET    /api/boards/<b>/nodes/<n>/docs            → { files[], hasApp }   文件夹里有什么就是什么
GET    /api/boards/<b>/nodes/<n>/docs?q=         → 同上，按名字 + 正文筛，命中带 snippet
POST   /api/boards/<b>/nodes/<n>/docs { name }   → { name }             建一份
GET    /api/boards/<b>/nodes/<n>/doc/<name>      → { text, etag, stub }
PUT    /api/boards/<b>/nodes/<n>/doc/<name>      → { text, etag } → { etag } | 409
DELETE /api/boards/<b>/nodes/<n>/doc/<name>      → { name }             挪进 .trash/
POST   /api/boards/<b>/nodes/<n>/doc/<name>/restore → { name }          从 .trash/ 搬回来
```

五条不变的：

- **正文不进 `tree.json`。** 每份文档有自己的文件、自己的 `etag`（内容哈希），**不参与 board 的 `version`** ——
  写一篇文章不该把树顶成 409，落一手棋也不该让文档冲突。
- **文件夹就是清单。** 不存 index、不往树里写「他有几份文档」。`touch ideas.md` 进去它就出现。
- **`AGENTS.md` 只能改，不能删。** 它是 coding agent 进目录时自动找的文件名，删了整块就没人读得懂。
  UI 上删除入口根本不生成，服务端 `DELETE` 是 403，大小写变体一起拦（macOS 上 `agents.md`
  和 `AGENTS.md` 是同一个文件，Linux 上是两个 —— 别让一个名字在两台机器上是两件事）。
  删别的文档是**挪进 `map/<id>/.trash/`**，不真删，和节点的 `.gone/` 同一套逻辑。
- **树里没有的节点不给看。** 文件夹可以躺在 `.gone/` 里，旧书签不能顺着 id 把它读活。
- **文件不在就给 stub，但不落盘。** 看一眼不该长出一个文件。只有入口那份给 stub，别的不在就是不在。
- **页面盖在盘面上，不卸载盘面。** 卸载会丢掉树的展开状态、滚动位置和撤销栈；
  被盖住的区域用 `inert`，键盘别穿透下去。

文档名（`src/docname.ts`，服务端和浏览器共用同一份）：小写、空格转 `-`、只留字母数字中文和 `._-`、
补 `.md`。**只有这一份实现** —— 两份会漂，上一版就漂过：客户端补 `.md`，服务端不补。

### 一个块 = 一个独立进程（没有条件）

`/p/<board>/<nodeId>/` 往下**永远是那个块自己的进程**，剥掉前缀反代过去，剩下的路径归它解释。
没有「有代码才有进程」这回事 —— 那会让宿主有权替块当页面，铁律立刻变成特权。

| 文件夹里有什么 | 谁在这个进程里 |
|---|---|
| `package.json` 有 `scripts.start` | 它自己写的代码（宿主 `spawn …`，只给四个环境变量） |
| 只有 `index.html` / 静态文件 | 宿主内置的空跑 server（`server/blank.mjs`），静态端出去 |
| 什么都没有 | 同一个空跑 server，回一张**真正的空白页**（无标题、无文字、无链接） |

壳只渲染两样东西：`/`（盘面）和 `/p/<board>/<id>/__nous/doc`（文档页，盖在盘面上）。
**块的页面壳一概不管** —— 所以左列「打开」是整页跳走，不是盖一层。

进程的契约，都是硬的：

- **听 `PORT`。** 宿主给的空闲端口，不写进任何文件。
- **数据写 `NOUS_DATA`**（= 它自己文件夹下的 `data/`）。不是宿主替你存 —— 是「`cp -r` 走了数据也走了」。
- **`__nous/` 不许碰。** 前缀底下的别处全归你，只有这一段永远归宿主（文档页、`__nous/ctx`）。
- **链接写相对路径。** 这是唯一能让 `cp -r map/<id> ~/<id> && npm start` 和宿主里打开
  是同一页的写法。宿主不重写 HTML、不改你的绝对路径，`/style.css` 就是逃出前缀。

宿主这边：崩了给墓碑（stderr 最后几行）+ 502，**只坏这一页**；有代码的块，文件夹里的代码一改
就收掉进程（下次请求带着新代码起来，不自动重启 —— 崩得勤的块会把机器烧穿）；`scripts.start`
改了同理；闲 10 分钟退出。`npm install` 不归宿主管：块要依赖就把 `node_modules` 放进去。

给的四个环境变量，就这四个：`PORT`、`NOUS_DATA`、`NOUS_ORIGIN`（宿主地址）、
`NOUS_SELF`（我在宿主里叫什么，`/p/<board>/<id>`）。

### agent 运行时：`@earendil-works/*`

`pi-durable`（agent loop + tools）、`chord`（持久化/sqlite）、`pi-ai`（模型供应商）钉在 **1.4.0**，
三个一起升、一起锁死 —— `pi-durable` 自己就是精确钉 `1.4.0`，顶层写 `^` 迟早分叉出两份 `pi-ai`，
症状是「同一个类却不相等」。

- **只在 Node 里跑。** 只有 `server/*.ts` 能 import。`src/` 里一行都不许 —— 它们拖着
  `openai` / `@anthropic-ai/sdk` / `@aws-sdk/client-bedrock-runtime` / photon wasm，
  进客户端 bundle 就是几十兆跑不了的死重。`vite.config.ts` 把 `@earendil-works/*` 标成 external，
  手滑了在 build 期就吭声，不是悄悄膨胀。
- **持久化状态写 `NOUS_DATA`。** chord 的 sqlite 也好、agent 的会话记录也好，落块的 `data/` 底下，
  **不许落仓库根**。理由跟块一样：`cp -r map/<id> ~/<id>` 走了，状态也跟着走。
- `node >= 22.19.0`（`engines` 已声明）。`node:sqlite` 在 20.x 上不存在，别指望它会以看得懂的方式报错。

### 锁：做成了 app 的块不许被删

```
content/<board>/.nous/locks.json    {"v":1,"ids":["n41"]}     ← 锁住谁
content/<board>/.nous/audit.log     什么时候谁想让它消失
GET  /api/boards/:id/locks                       → { ids }
PUT  /api/boards/:id/locks   { id, locked }      → { ids }
```

- **锁是宿主的状态，不是块的内容。** 所以它不住在 `map/<id>/`（那是用户和 AI 的地盘，一个 `rm` 就没了），
  也不进 `tree.json`（那会变成第二个真相）。
- **只有块能锁，组不能。** 组是容器，锁容器等于间接锁子节点 —— 同一件事的第二种写法。
- **强制在服务端。** 客户端发来的树里少了锁定的块，`save()` 沿它**原来**的位置往上找最近的存活祖先，
  把它整枝接回来，并把修正后的树返回给客户端（客户端必须认这份，否则本地和盘上分叉）。
  所以「删掉它所在的组 = 它上提一层」，不是「它跟着没了」。
- **前端隐藏「删除」只是防手滑。** 真正拦住机器的是上面那条；别在 UI 里加第二套判断。
- **绕过会留痕**：每次「锁定块从树里消失」的尝试都写进 `audit.log`，带来源地址。

做不到的也说清：能改宿主状态文件的人（root、或者将来的 AI）照样能解锁。
这里给的是「没有顺手删掉这回事」，不是物理防线。

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
