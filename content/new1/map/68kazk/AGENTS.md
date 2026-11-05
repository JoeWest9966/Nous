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
  上游清单**不**走这套：它列的是「一整张表」，还是主区换页。
- **`＋ 新建技能`**：只问三样 —— 名字、一句 `description`、类别（spec §6.5）。
  建完**不发布**：新技能的第一个正确状态是「未发布」。

## 写路径全在 qrd3wh（这一页只发请求）

```
GET  /v1/skills                              清单 + repo + archived + categories
GET  /v1/skills/files?name=                  <name>/ 下的文件
GET  /v1/skills/file?name=&path=             一份文件（text + etag）
PUT  /v1/skills/file {name,path,text,base}   落盘，不提交；base 对不上 → 409
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

**抽屉不建文件、不删文件。** 它只读写已经存在的文件。想加文件就 `touch`，列表下一次读就有了。

## 文件

```
package.json       type: module + scripts.start（宿主凭这一条起进程；测试也靠 type: module）
server.js          纯静态（57 行那种，没有反代、没有配置）
public/index.html  骨架：顶栏 / 左栏 / 网格 / 上游 / 详情浮层 / 抽屉 / 确认框 / 新建
public/model.js    纯计算：卡片状态、facet 计数、排序、删除类别影响范围、未分类桶
public/api.js      数据层：`createApi`（可注入 fetch）、ctx 解析、错误信封 / 409、类别 debounce、
                   发布后重读。不碰 DOM —— 测试拿假 fetch 打的就是这一层
public/app.js      DOM 那一半：渲染、菜单、手势、抽屉、diff 视图、Esc 层级
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
