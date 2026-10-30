---
name: nous-skill-catalog-panel
description: "本块（68kazk）是一个 skill 编目管理面板：读 nw-panel 的 REST API，写同一套端点（改类别 / 重扫 / 删源 / register 两段式）。要改它、部署它、或排查「面板一片空」时读这一份。"
---

# skill 编目面板（68kazk）

一个块的独立进程：`node server.js`，零依赖、零构建。它**不碰数据仓**，也不 import
`nw` 的任何代码 —— 一切读写都经 `https://j1900.tail8d0018.ts.net:8443` 的 REST，
和 `nw` CLI 是同一批端点（`nw source` 命令组的 HTTP 皮）。

一句话区别：这是**另一块屏幕**，不是第二个后端。编目仍然只有 `nw` 一个写入口。

## 文件

```
package.json   scripts.start = node server.js（宿主凭这一条起进程）
server.js      /api/nw/* 反代到 warehouse + 发 public/ + 存 base/token
public/        index.html / app.js / styles.css（原生 JS，无构建步骤）
               UI 照 nw-panel SourcePane 的 #205 店面裁定：网格主体 = **源（仓库）卡**
               （字母砖 + 物种角标 + N 条目 + 版本 + 标签色点），不是 skill 条目卡；
               左栏 facets（类别树/标签，**计数对全量算** —— facet 是导航不是结果统计；
               物种 facet 砍掉了 —— 这块只管 skill 那本账，extension 是另一本，j1900 上
               也一本 ext 都没有，摆着就是空转的摆设）；
               抽屉 = 源详情（字段 + 条目列表带内搜 + 条目级标签，ref 两段 id 整段 encode）；
               对账是独立报告视图（400 + 漂移照样渲染，不落错误页）。
               **版本漂移那一行挂「重新登记」**：打开同一个 register 预览弹窗，把账面那
               一条的 repo / name / category 预填进去（只预填，不预跑）—— 写路径还是 register
               两段式，闸还是那道闸；漏填类别就会把分类清掉，所以弹窗里明说这一句。
               落账按钮**预览通过且表单未变才可点**（SourcePane 那条「红的不让确认」纪律）。
data/          跑起来之后才有：config.json（base + 可选 token）、upstream.json（上游 tag 缓存）
```

## 为什么中间要有一个反代

浏览器的页面在 `NOUS_ORIGIN/p/...` 下，warehouse 在另一个 origin（tailnet 8443）。
直接 `fetch` 它 = 跨域 + 要 CORS + 备用入口的 Bearer token 泄漏到页面里。
所以浏览器只跟本块说话，`server.js` 那一跳带上 token。**token 永远不进 `public/`**，
只活在进程内存和 `data/config.json`。

两档超时（照 nw-panel 自己的 #156 裁定）：读 20s，写 180s。超时回 504 时说的是真话 ——
「上游可能已经写了」，不要凭猜重跑。

## 端点对照（面板动作 → warehouse 端点 → CLI）

| 面板上的动作 | HTTP | CLI |
|---|---|---|
| 读全部源 | `GET /api/source` | `nw ls sources` |
| 改类别 | `PUT /api/source/skill/{名}/category` | `nw source category` |
| 重扫（源 / 全部） | `POST /api/source/recheck` | `nw source recheck` |
| 登记·预览 | `POST /api/source` | `nw source register`（不带 `--yes`） |
| 登记·落账 | `PUT /api/source/{名}` | 同上 + `--yes` |
| 删源 | `DELETE /api/source/skill/{名}` | `nw source rm` |
| 标签增摘 | `POST` / `DELETE /api/source/skill/{名}/tags[/{标签}]` | `nw tag add/rm` |
| 读上游 tag（面板自己） | `GET` / `POST /api/upstream` | 无 —— 见下 |

envelope 原样透传：`{schema,nw,ok,errors[],warnings[],data}`。**面板不复刻语义闸** ——
kind 值域、类别词法、验收 6 行全归上游；面板只负责把它们摆到屏幕上。

纪律照抄编目：`sources[]` 与 `entries[]` 按 name 排序；类别空数组就是「未分类」（读取时算的，
文件里没有这两个字）；`version` 是登记那一刻的上游 release tag，抓不到就是 `""`，不参与比对。

### 版本两个数字：账上 vs 上游（**不合并**）

```
账上  = 编目里那条 version  = nw 注册那一刻抓的 latest **release** 的 tag（会过期，今天 matt 是 v1.3.1）
上游  = 面板自己查的        = 最新 **semver tag**（git ls-remote，今天 v1.3.2）
```

**这一列为什么不由 nw 给**：nw 的 `source recheck` 也报版本漂移，但它在 j1900 上今天是红的
（真 clone 上游，那台机器出网坏，91 秒后失败）。本块跑在 Nous 这台机器上，出网正常，
一条仓库 `git ls-remote --tags` 一次 ~1 秒，**而且 git 协议不占 api.github.com 那 60 次/小时**
（整台机器共享，页面开几次就穿）。

所以 `GET /api/upstream`（读缓存 + `stale[]`）/ `POST /api/upstream`（`{repos[], force}`，
并发 3、每仓库 12s 超时）：

- 只认 semver（`mattpocock-skills@1.0.0` 这种不算版本号），按数字排取最大；
- **按 repo 去重** —— `matt` 和 `mattpocock` 是同一条仓库的两份账，只查一次；
- 查不到只记 `error`，**不算「落后」**（断网 / 私有仓 ≠ 版本落后，这是两种病）；
- 缓存落 `data/upstream.json`，TTL 30 分钟，重启不丢、不冷启动就打 GitHub；TTL 过期由人点
  「检查上游」，**不每次开页面就查**。

显示成 `v1.3.1 → v1.3.2` 这样一个 chip：绿（一致）/ 黄（落后）/ 灰（`→ ?` 或 `未打 tag`）。
**不写编目**：改账还是走那个「重新登记」弹窗 —— nw 的写入口，闸还是那道闸。

## 踩过的坑（面板自己的）

1. **前端请求必须是相对这块自己的根。** 写成 `/api/nw/...` 在「自己跑」时是对的，搬进
   Nous（`/p/<board>/<id>/`）就**越出前缀打到宿主身上**：Nous 的 SPA 回退回一张 200 的
   `index.html`，于是面板报「上游回了非 JSON（200）」并吐一堆 HTML。`app.js` 里的
   `ROOT` / `api()` 就是为这一脚设的；新加请求一律走 `api("/…")`，别手写 `"/api/…"`。
2. **忙碌期渲染出来的按钮是 `disabled` 的。** `runOp` 必须在 `busy` 清掉之后再画一次，
   否则按钮永远按不动 —— 看起来像「面板没反应」，其实是自己把自己锁死了。
3. **`append()` / `replaceChildren(null)` 会得到一个字面量 `"null"` 文本节点。**
   条件渲染一律走 `mount()`（本仓 `app.js` 里那个小函数），别直接传 `null`。
4. **预览 → 落账是两份意图时不能落。** 预览是快照；表单改了就重来，不然写进去的是上一版参数。
5. **recheck 在这台机器上今天是红的**：`git clone` 取不到上游（2026-10-29 实测，单源 91 秒后
   返回 `取回 … 失败(git clone)`）。面板照样把它完整摆出来 —— 那是机器的病，不是面板的病。
   修它要修 j1900 上的 git 认证 / 出网，不是改这里。
6. **`--root` 那类旗子面板一概不带**：base 是唯一的连接参数，其余由 nw-panel 启动时的
   `--root /opt/Data-001` 决定。面板一片空先怀疑那个 root，不是这里的 base。

## 搬出去跑

```sh
cd map/68kazk
NW_PANEL_BASE=https://j1900.tail8d0018.ts.net:8443 PORT=8080 node server.js
```

链接全是相对路径，不依赖宿主前缀。`data/` 跟着文件夹走。
