---
name: nous-skills-panel
description: "本块（68kazk）是 Nous 自持技能的清单面板：一张技能卡网格 + 左栏类别 facet + 正文抽屉。只发静态文件，读写全在 qrd3wh。要改它、部署它、或排查「面板一片空 / 点不动」时读这一份。"
---

# skills 面板（68kazk）

**`Nous-skills` 仓库里现在有哪些技能**，摆成一页卡片。左栏类别 facet，主区技能卡网格，
点卡进详情（类别 / 正文 / 仓库），`正文 ›` 打开右边抽屉读写 `SKILL.md`。

上一版是「技能编目」的店面：反代到上游编目服务、Bearer token、源卡 + 对账报告 + 版本漂移 +
重新登记。上游编目整个退场了（技能变成我们自己 git 仓库里的文件），那套代码在 git 历史里。

## 为什么只剩发静态文件

数据不走这个进程。页面自己去问 qrd3wh 的 `GET /v1/skills`，地址从宿主的 `__nous/ctx` 里
找 `qrd3wh` 那个块拿 —— **这一块里不拼任何绝对路径**。所以 `server.js` 只剩一件事：
把 `public/` 发出去（剥 `NOUS_SELF`、目录穿越 403、目录 → `index.html`、`cache-control: no-store`）。

这一块**没有** `data/`、没有配置、没有 token、没有后端状态。它不在 `/v1/skills` 那个仓库上跑
任何 git：写盘、重扫、commit、push 都是 qrd3wh 的事（spec I4）。

## 页面上每一栏在说什么

- **顶栏**：仓库 HEAD + `有未提交改动` / `N 个提交没推` / `远端多 N 个` / `已同步`。
  `repo.error` 有值时那行变红 —— git 命令失败也要能显示「仓库读不了」，而不是白屏（spec §6.2）。
- **左栏类别**：缩进的类别树（`/` 分段），每个类别带计数 + 底部「+ 新建类别」。
  点一下筛选，**父级筛出整枝**（点「工具」连「工具/搜索」一起出来；结果区显示「工具 · 含 N 个子级」）。
  每行右侧常驻 `⋯`：新建子类别 / 编辑类别名称（填**全路径**，改前缀就等于搬家）/ 删除类别
  （带影响范围「N 条技能」，删除是删整枝）。右键 / 长按只是快捷方式。
- **顶部筛选**：全部 / 未分类 / 未发布 / 装载失败 / 已归档 N。后两个非零才出现。
  状态图例就是筛选，不另写说明。
- **技能卡**：字母砖（颜色由名字 hash，永远同一个色）+ 名字（`不进提示词` 的会标出来）+
  说明 + 类别 chip + 右上角常驻 `⋯`。状态只有三种：
  **正常** = 实边；**未发布**（`tracked: false`）= 琥珀虚线边 + 琥珀角标；
  **装载失败** = 实线红边 + 实心红角标 + 卡上一行 `error`。
  判据在 `public/model.js` 的 `cardState()`，**优先级 failed > unpublished > ok**（spec §7.1）。
- **「已归档 N」**：沉到列表最后，点开就是回收站：一行一条，`恢复` / `彻底删除`（手输名字才亮）。
  归档项**不在网格里**。
- **详情**（点卡 → **半扇浮层**，右边滑出，和 `正文 ›` 那条抽屉一副骨头）：类别（唯一可编辑的地方）/
  正文（`description` + `SKILL.md` 正文按需读，`改正文 ›` 再叠一层抽屉）/ 仓库（发没发布、目录、文件）。
  宽 `min(720px, 100vw)`，`z` 30/31，在正文抽屉（40/41）**下面** —— 所以详情里开抽屉是盖上，
  关抽屉回详情，关详情才回网格；关掉列表滚动位置、筛选、搜索词一个不动。窄屏整屏从右出。
  上游那一屏**不**走这套：它是主区换页。但**一个订阅只有一张卡** ——
  上游按「一个仓 = 一条技能」算（`matt`，整个仓）。点卡还是那扇浮层，浮层里 `看正文 ›`
  是那只只读抽屉，左栏是**整个仓的文件树**。取用是全节唯一的例外：一个仓里 39 条技能
  能分别取，所以它挂在树里那条 `SKILL.md` 的行尾。左栏 = 订阅列表。
- **`＋ 新建技能`**：只问三样 —— 名字、一句 `description`、类别，外加可空的**分组**
  （`deprecated` 那种只装技能的抽屉，一层；spec §6.5）。建完**不发布**：新技能的第一个正确状态是「未发布」。

## 写路径全在 qrd3wh（这一页只发请求）

```
GET  /v1/skills                              清单 + repo + archived + categories
GET  /v1/skills/files?name=                  <name>/ 下的文件（递归，含子目录）
GET  /v1/skills/file?name=&path=             一份文件（text + etag）
PUT  /v1/skills/file {name,path,text,base}   落盘，不提交；base 对不上 → 409
                                             **目标不存在 = 新建**（中途目录一起造）
POST /v1/skills/mkdir {name,path}            建空文件夹（里面铺 .gitkeep）
DELETE /v1/skills/file?name=&path=[&recursive=1]  删文件 / 删目录，不提交
GET  /v1/skills/pending/diff?path=           对 HEAD 的基线（未跟踪 → before: null）
POST /v1/skills/category {name,category[]}   改一条技能的类别
POST /v1/skills/categories {op,name,to?}     建 / 改名（整枝）/ 删（整枝）
POST /v1/skills/create {name,description,category?}
POST /v1/skills/publish {message?}           内容闸 + 重扫 + commit + push
POST /v1/skills/archive | restore | purge {name}
```

**保存 ≠ 发布，两个按钮不许合成一个。** 保存只落盘（敲到一半可以停）；发布才 commit + push，
而且要过服务端的内容闸（frontmatter / `name` == 目录名 / `description` 非空 / `nous.json` 能解析）。
这一页不复刻那道闸（spec I8）—— 输赢由服务端说了算，UI 只摆结果。

**抽屉能建文件、也能删。** 左栏是一棵树（文件夹由「有文件住在这儿」推出来，`model.js` 的
`fileTree()`）：`＋ 文件` 写一个还不存在的路径，`＋ 文件夹` 建目录 + 一份 `.gitkeep`
（空目录 git 不跟踪，没有它发布那一刻文件夹就没了；写进第一个真文件时服务端会收掉它）。
行尾的 `×` 删文件 / 删整棵，要手输路径确认。**都只落盘、不提交** —— 删掉的东西在 git 历史里，
发布之前都捡得回来。文件列表是现扫的，不存索引。

## 文件

```
package.json       type: module + scripts.start（宿主凭这一条起进程；测试也靠 type: module）
server.js          纯静态（57 行那种，没有反代、没有配置）
public/index.html  骨架：顶栏 / 左栏 / 网格 / 上游 / 详情浮层 / 抽屉 / 确认框 / 新建
public/model.js    纯计算：卡片状态、facet 计数、排序、删除类别影响范围、未分类桶、**文件树**
public/api.js      数据层：`createApi`（可注入 fetch）、ctx 解析、错误信封 / 409、类别 debounce、
                   发布后重读。不碰 DOM —— 测试拿假 fetch 打的就是这一层
public/app.js      DOM 那一半：渲染、菜单、手势、抽屉（编辑 / 上游只读两档）、diff 视图、Esc 层级
public/diff.js     行级对齐 + 词级高亮 + 折叠（不碰 DOM）
public/vendor/diff  jsdiff 8（MIT），逐文件拷的 ESM —— 这页没有打包器
test/model.test.js 测 model.js；test/api.test.js 测 api.js（都不测 DOM）
```

## 踩过的坑（改之前先读这几条）

1. **尾斜杠。** 宿主把这一块挂在某个前缀下面（可能没有尾斜杠）。`index.html` 顶部那段一跳是必须的，
   不然 `./style.css` 会解析到上一层去，页面没样式、`app.js` 直接 404。
2. **`SELF` 只从 `location.pathname` 来，邻居地址只从 `__nous/ctx` 来。** 老版本写死过路径：
   请求越出前缀打到宿主的 SPA，回了 200 的 `index.html`，面板报「上游回了非 JSON」。
   现在地址的唯一来源是 `api.js` 的 `hostBase(ctx)`：在 `ctx.blocks` 里按 **id `qrd3wh`** 找
   （不是名字，名字会改），找到之前一个 `v1/skills…` 请求都不发。找不到就报「ctx 里没有 qrd3wh」。
3. **Esc 的层级**：菜单 → 新建弹层 → 正文抽屉 → 详情浮层 → 列表。菜单排最前，因为它是最后一次交互；
   不这么排的话，Esc 关菜单会顺手把背后的详情也关了。抽屉里差异态先回编辑态再关。
   左栏输入框 / 类别输入框 / 正文类别输入框的 Esc 自己 `stopPropagation()`，否则一次 Esc 关两层。
4. **长按 520ms、挪 10px 当滚动、松手吞掉那个 `click`。** 不吞就是「松手即误触」——
   菜单就在手指底下弹出。菜单件（`mountMenu`）负责贴边夹紧 + 点外面 / `Esc` / 滚动 / 改窗口大小都关；
   卡片和类别行共用同一份，两套各自算位置的代码必然一套对一套错。
5. **输入框要等整串挂进文档之后再 `focus()`。** 行还在内存里时聚焦会落空（左栏踩过）。
6. **差异的基线走 `pending/diff` 的 `before`**，不是文件接口的字段：`§6.1` 的
   `GET /v1/skills/file` 只回 `{name,path,text,etag}`。未跟踪的文件 `before: null` → 整份算新增，
   抽屉里会写一句「发布之后才有基线」。发布成功后必须重读文件 + 重取基线，否则 diff 还在跟旧 HEAD 比。
7. **没有 mock、没有离线缓存、没有本地副本**（spec I2）。D1 的 `public/mock/` 和 `MOCK` 开关
   已经拆了；测试的 fixture 只活在 `test/` 里。想加「先拿缓存顶一下」= 造第二个真相源，别加。
   类别写回是 8 秒 debounce：离开详情页 / 发布前 / 切后台都要 `flush()`，否则用户以为存了。
   保存的 `base` 过期时服务端回 409 —— 那是「重读再改」，不是通用错误（`ConflictError.conflict`）。
8. **测试命令。** `node --test test/`（`test/index.js` 是 barrel，把两个测试文件都 import 进来）、
   `node --test "test/*.test.js"`、裸 `node --test` 都能跑。node 22 把目录参数解析成它的
   `index.js`，所以别删那个 barrel。
   也别忘了 `node --check server.js && node --check public/model.js && node --check public/api.js && node --check public/app.js`。
9. **别 import `server.js`。** 它有顶层 `listen`。能纯算的东西放 `model.js`，测试只测它；
   DOM 归合并后的 `code-review` 看（spec §9：不测 DOM）。
10. **窄屏 375px 不横滚。** 760px 以下左栏折到上面、网格 `minmax(150px,1fr)`、抽屉整屏。
    别往卡里塞长路径 —— 需要的都放 `title` 和详情里。
11. **文件夹是算出来的，不是存出来的。** `state.editor.files` 永远是服务端那份扁平列表，
    树由 `fileTree()` 现算（`ed.dirs` 只存「哪几个展开了」）。在状态里另存一棵树，第一次
    删除之后两边就对不上 —— 那种 bug 的现场是「文件没了，文件夹还在」。
12. **新建 = PUT 一个还不存在的路径。** 已存在就不能拿空串盖掉它：前端在 `submitNewEntry`
    里先把同名的挡下来（那是「新建」不是「清空」），服务端靠 `base` 兜底。
    空文件夹靠 `.gitkeep` 撑着，但它在文件列表里看不见（点开头的文件本来就跳过）——
    所以「建完什么都没出现」是正常的，别去修它。
13. **删除要 `recursive=1` 才动非空目录。** 不带时服务端 409 并说「还有 N 个东西」；
    界面上的 × 一律带 `recursive=1`，靠手输路径那一步挡人（跟归档、彻底删除同一套骨头）。
    删的是盘上的文件，**不是**归档：git 历史就是回收站，没发布之前都捡得回来。
14. **上游那条的「看正文」用同一扇抽屉，只读那一档靠 `#editor[data-readonly]`。**
    不是另做一块：文件树、行号、Esc 层级全复用。差异的是**收掉**写的入口
    （保存 / 发布 / 新建 / 删除 / 差异页签），不是灰着 —— 灰着是「你还没资格点」，
    收掉才是「这里本来就没有这件事」。`closeEditor` 记得把 `data-readonly` 和
    `readOnly` 一起摘掉，不然下一回打开本地那条就写不了了。
15. **文件树里那个名字要给 `min-width: 0`。** 少了它 flex 不让缩，上游那些
    `DESIGN-IT-TWICE.md` 会把右边的大小挤成两行（`2.6` / `KB`）。
16. **上游的正文是从镜像 `cat-file` 出来的，不摊进我们的仓。** 看正文不等于取用 ——
    两者之间没有隐藏的状态迁移，取用永远是自己那一步（`POST upstream/adopt`）。
17. **别把上游拆成一条一条的。** 一个订阅 = **一条技能**（整个仓）。我在这上面错过两次：
    先是把上游做成「39 张卡 + 目录卡 + 面包屑」（文件管理器），再是做成「39 张卡 + 左栏筛目录」。
    对的形状是**一张卡**，`看正文` 打开整仓的树。`upstreamTree` / `upstreamAt` /
    `upstreamCrumb` / `sortUpstream` 那一套零件已经删了，别接回去。

## 搬出去跑

```
cp -r content/new1/map/68kazk ~/68kazk && cd ~/68kazk && npm start   # → http://127.0.0.1:4784/
```

数据来自 qrd3wh，地址来自宿主的 `__nous/ctx` —— 这份 ctx 只有宿主给得出来，
所以这一页只在宿主里跑得起来，这是设计。裸 `npm start` 会停在「ctx 里没有 qrd3wh」那张错误卡上。
`data/` 不预建、不需要：这一块没有后端状态。

## 不做的事

- **不装别人的技能、不搜市场。** 仓库里有就是装着；从别处「导入」等于 `cp -r` 再 commit。
- **不读仓库文件来回答「有哪些技能」。** 真相在 qrd3wh 的 `list()` 里。
- **不给技能改名按钮。** `name` == 目录名 == 会话里的 API 键（spec I3），改名会断掉所有老会话。
- **不复刻服务端校验。** UI 只挡「名字空着」这种连提交都说不清的情况；撞已有目录、撞 `.archive/`、
  撞保留名、名字字符集全在服务端一处判，400 的原文直接显示在新建弹窗里（I8）。
- **不做版本号 / 变体 / 上游漂移。** 技能没有这些（spec §11）。
