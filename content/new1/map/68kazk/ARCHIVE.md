# 存档：skill 的上游编目那一套（2026-11 之前）

**这个分支只读，别在上面开发。** 它的唯一用途是：将来真要重新做「上游编排 / 技能市场」时，
从这里抄得回一份当时跑得起来的东西。

`main` 上这些代码会被删掉或改写（见 `JoeWest9966/Nous#4`）：

| 这份存档里的 | 现在（main）的状态 |
|---|---|
| `content/new1/map/68kazk/server.js` —— 反代 `/api/nw/*` → warehouse，带 Bearer token，两档超时（读 20s / 写 180s） | 改成纯静态服务器（票 D） |
| `content/new1/map/68kazk/public/app.js`（1409 行）—— 源卡网格 + 类别/标签 facet + 对账报告 + 版本漂移 chip + 「重新登记」两段式弹窗 | 整套作废，换成 `vbe4pt` 那套卡片/抽屉/菜单（票 D） |
| `content/new1/map/68kazk/public/index.html`、`styles.css` —— nw-panel #205 店面裁定的实现 | 同上 |
| `content/new1/map/68kazk/AGENTS.md` —— 「编目仍然只有 nw 一个写入口」「这是另一块屏幕，不是第二个后端」 | 整份重写（票 E） |
| `content/new1/map/qrd3wh/server.js` 的 `GET/POST /v1/skills`、`/v1/skills/install`、`/v1/skills/uninstall` —— 背后是 `skills.refreshCatalog()` / `skills.install()` | 换成 `/v1/skills*` 全套读写（票 C） |

配套的另一半在 `JoeWest9966/Nous-extend` 的 `archive/upstream-skills-2026-11` 分支：
`skills/index.js` 里那 467 行「拉编目 → 抓 tarball → 写 `.nw.json` 收据」的抓取逻辑。

## 想重新启用时，先看清这几件事

这些不是「过时」，是**当时就是坑**：

1. **`recheck` 在 j1900 上是红的** —— 真 `git clone` 出网失败，单源 91 秒后返回，这是机器的病。
2. **`api.github.com` 全域 60 次/小时**（整台机器共享），所以版本号得走 `git ls-remote`。
3. **编目里没有版本层** —— 所以「你装的是哪一版」只能靠 `.nw.json` 收据，而这个收据只记 sha，不记语义版本。
4. **同一个仓库会被登记两遍**（`matt` / `mattpocock`），按 `repo + path` 去重是硬需求。
5. **重名/漂移两份账**：`/var/opt/Data-001/skills.json` 与 8443 上的编目会互相漂。

## 不值得带回来的

- `http://j1900.tail8d0018.ts.net:8443` 这个具体地址 —— 换机器就没了。
- 面板里那套「源 / 条目」两层结构（源是一等公民，条目挂在源下）：真重做多半会直接做条目。
- 「重新登记」两段式（预览 → 落账）本身没错，但它服务的是 `nw` 的写入模型；换成自持仓库以后，
  发布走 `git commit`，这套闸就没有对应物了。
