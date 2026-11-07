# jtjzw5 · Ultimate-Template —— 终极模板那一段的页

`Nous-Template` 仓里 `Ultimate-Template/` 这一段的视图。**只发静态文件**：数据全是浏览器去问
`qrd3wh` 的 `GET /v1/presets/Ultimate-Template`，这一块不读仓库、不跑 git、没有 `data/`。

跟 `3699b9`（同一仓的另一段 `Session-Template`）是同一套骨架、不同的段。两页不互相窜：
段名各写各的（`public/api.js` 的 `SECTION`），改这一段碰不到那一段。

## 现在这一版是只读的 —— 这不是偷懒

`ultimate.json` 的**形状还没定**。`Nous-Template/sections.json` 里那一段的 `checker` 是 `null`，
所以 `index.mjs` 明着说「跳过」、引擎对这一段回 503。

形状没定就把编辑器长出来的结果，是写完就扔。所以这一版只做一件事：
**把一条终极模板长什么样说清楚** —— 叠了哪几条预设、钉的是哪条提示词、坏在哪儿、好坏各什么脸色。

没有的：新建、编辑、归档、启动（按钮是灰的，`title` 里写着为什么）。

| 形状定下来之后要做的 | 在哪儿 |
|---|---|
| `index.mjs` 注册 `ultimate` 的 checker（键表 + 条数上限 + 引用必须带段名） | `Nous-Template/` |
| `qrd3wh/presets.js` 补 ultimate 的引用校验 + 展开 | 引擎 |
| 把 `demo.js` 删掉，`rowOfApi` 接真数据 | 这一页 |
| 编辑器 / 新建 / 归档三件套（照 3699b9 抄） | 这一页 |

## 示例数据：**说得出来**，不是偷偷用

这一段还装不了东西时，页面画 `public/demo.js` 里那五条。规矩两条：

1. **顶上必须有黄条**写清「这些是示例数据，不是仓里的东西」。黄条是 `#banner`，内容来自
   真实的那个 503 原文 —— 引擎说「这一段还没装好」，页面就把这句话摆出来。
2. **只在 503 时切**。别的错误（没连上引擎、块被删了）照实报，不要拿假数据糊过去。

五条示例正好盖住四种卡状态：`ok` / `unpublished`（只在本地）/ `invalid`（引用的预设没了）/
`failed`（自己写坏了）。少了哪一种，红卡那一套就是没测过的。

## 卡状态：`failed > invalid > unpublished > ok`

顺序就是「先修哪儿」：

| 状态 | 长相 | 意思 | 修哪儿 |
|---|---|---|---|
| `failed` | 红边实线 + `specError` 一行 | 这条自己写坏了 | 这里 |
| `invalid` | 红边 + 点名哪条引用不在 | 引用的东西没了 | **那边** |
| `unpublished` | 琥珀虚线 | 盘上有、HEAD 没有 | 点发布 |
| `ok` | 实边 | 好的 | — |

`failed` 压在 `invalid` 上面是有理由的：写坏了的那条**连解析都做不了**，这时报「引用失效」
是骗人 —— 引用可能全是好的。

## chip 的三态

| chip | 三种长相 |
|---|---|
| 预设 | `预设 默认`（没这个键，灰）/ `预设 无`（`[]`）/ `预设 N 条` / 红（`N 条 · M 条不在`） |
| 提示词 | `提示词 默认`（缺席，灰）/ `提示词 无`（`null`，正常色）/ `提示词 <名>` / 红 |
| 只读 | 只在 `readOnly: true` 时出现（`false` 是常态，不占一行） |

「默认」和「无」必须是两句不同的话。混成一句，三态就没了 —— 这条在 `3699b9` 上已经踩过一次。

## 文件

```
package.json        type: module + scripts.start（宿主凭这一条起进程）
server.js           纯静态（剥 NOUS_SELF、403 目录穿越、no-store），默认 PORT=4787
public/index.html   顶栏 / 黄条 / 左栏 / 筛选 / 网格 / 详情浮层
public/model.js     纯计算：卡状态 / chip / facet 树 / 筛选 / 叠放顺序（一个 DOM 都不碰）
public/api.js       ctx 解析、段名补齐、错误信封（503 → SectionNotReady）
public/app.js       DOM 那一半：渲染、详情、抽屉、Esc
public/demo.js      示例数据（**形状定了就删**）
test/               model.js / api.js 的测试（不测 DOM）
```

## 不做的事

- **不写 `data/`。** 真相只有 `Nous-Template` 仓。
- **不跑 git。** 发布 / 归档全走 `qrd3wh`。
- **不复刻引擎的判定。** 名字规则、引用存在性、条数上限 —— 一处（`qrd3wh/presets.js`）。
  这一页只翻译结果，服务端那一行错误原样显示。
- **不在窄屏藏掉类别。** 左栏在手机上是个抽屉，所以工具栏上必须有那个能拉开它的按钮。
- **不拿示例数据糊错误。** 只有 503（这一段还没装好）才切示例，且必须挂着黄条。
