# 6ysg01 · Session-Template

**`Nous-prompts` 里现在有哪些模板**，摆成一页卡片：左栏类别 facet、主区卡片网格、点卡开详情
（类别 / 摘要 / 正文）、`正文 ›` 开右边抽屉读写 `PROMPT.md` 和 `nous.json`。
详情里那个「用这条开会话 ›」是这座库存在的理由 —— 建一条已带着这条模板的会话。

## 为什么只剩发静态文件

数据不走这个进程。页面自己去问 `qrd3wh` 的 `GET /v1/prompts`，地址从宿主的 `__nous/ctx`
里找 —— **这一块里不拼任何绝对路径，也不跑 git**。所以 `server.js` 只做一件事：把
`public/` 发出去（剥 `NOUS_SELF`、目录穿越 403、目录 → `index.html`、`cache-control: no-store`）。

| 谁 | 干什么 |
|---|---|
| 本块 | 发静态文件 + 在浏览器里画 |
| `qrd3wh` | 唯一写入口：`/v1/prompts*`（落盘 / 重扫 / commit / push / 归档） |
| `Nous-prompts` 仓 | 唯一真相：`<名>/PROMPT.md` + `nous.json` |

**这一块没有 `data/`、没有配置、没有 token、没有后端状态。**

## 页面上每一栏在说什么

- **顶栏**：仓库 HEAD + `已同步` / `有未提交改动` / `N 个提交没推` / `远端多 N 个`；
  `repo.error` 有值时那行变红 —— git 失败也要能看见，而不是白屏。
- **左栏类别**：缩进的类别树（`/` 分段），父级筛出整枝；每行 `⋯`（右键 / 长按 520ms）：
  新建子类别 / 编辑名称（填全路径，改前缀 = 搬家）/ 删除整枝（确认框写清影响几条模板）。
- **筛选栏**：全部 / 未分类 / 未发布 / 装载失败 / 已归档。非零才出现；图例就是筛选。
- **卡片**：字母砖（色相由名字 hash）+ 名字 + 摘要 + 类别 chip。三种状态：
  **正常**实边；**未发布**（`tracked: false`）琥珀虚线；**装载失败**实线红边 + 卡上一行 `error`。
  优先级 `failed > unpublished > ok`（`model.js` 的 `cardState()`）。
- **详情**（点半扇浮层，不是换页）：类别（唯一可改的地方）/ 摘要 + `正文 ›` / 仓库（发没发布、
  绝对目录、文件）。装载失败时最上面那条红字 + 「重试装载」—— 那个按钮只重扫，**不会替你修文件**。
- **正文抽屉**：左边文件列表（`PROMPT.md` / `nous.json`），右边编辑槽 + 与 HEAD 的差异视图。
  **保存和发布是两个按钮，不许合成一个**：保存只落盘（`PUT /v1/prompts/file`，带 `etag`，
  撞了就 409），发布才 `POST /v1/prompts/publish`（内容闸 + 重扫 + commit + push）。
- **「用这条开会话 ›」**：`GET v1/device` 拿默认模型 → `POST v1/sessions { name, model, template }`
  （**同一个 commit 落定**，中间没有「会话建好了、模板还没到」那一瞬）→ `POST v1/selection`
  把盘上那只眼睛切过去 → **新标签页**打开会话引擎。不在这页内嵌聊天。

## 三条不许踩

1. **不复刻服务端的闸。** 名字字符集、撞目录、撞 `.archive/`、保留名、正文非空、`summary` 非空、
   `settings.default` 指向真模板 —— 全部由 qrd3wh 一处判定。这一页只播报服务端的 400 原文。
   本地多一套规则就是第二份真相。
2. **模板改完要重扫。** 扩展的 `render()` 不碰盘（每轮请求都跑），所以命令行里改了 `PROMPT.md`
   之后要么点「重扫」（`POST v1/prompts/reload`），要么从这一页的抽屉保存 —— 后者写端点自己会扫。
3. **新模板的第一个正确状态是「未发布」。** 建完不提交；发布是人的动作，不是新建的副作用。

## 文件

```
package.json       type: module + scripts.start（宿主凭这一条起进程）
server.js          纯静态（剥前缀、403 目录穿越、no-store）
public/index.html  骨架：顶栏 / 左栏 / 网格 / 详情 / 抽屉 / 确认框 / 新建
public/model.js    纯计算：卡片状态、facet 计数、排序、删除类别影响范围、未分类桶
public/api.js      数据层：ctx 解析、错误信封、409、类别 debounce、发布后重读
public/app.js      DOM 那一半：渲染、菜单、手势、抽屉、diff 视图、Esc 层级
public/diff.js     行级对齐 + 词级高亮 + 折叠（不碰 DOM）
public/vendor/diff jsdiff 8（MIT），逐文件拷的 ESM —— 这页没有打包器
test/              model.js / api.js 的测试（不测 DOM）
```

这一页是从 `68kazk`（skills 面板）抄的骨头，删掉了「上游」那一整块（模板仓没有上游这个概念），
字段从 `description` 换成 `summary`（摘要住在 `nous.json`，不进提示词）。
