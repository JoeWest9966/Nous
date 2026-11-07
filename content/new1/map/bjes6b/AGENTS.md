# bjes6b · Nous-Use

微信样子的 Nous 会话页。**这一块不跑 agent。**

- **引擎在 qrd3wh**（`Nous/Nous-Functions/session`）。会话树、模型、工具、持久化全在它那。
- **这里只有两层**：`server.js` 一层 BFF（把 `/api/*` 翻成 `/v1/*`），`public/` 一层壳。
- **进程无状态**。宿主改文件就杀它、闲十分钟就收它 —— 那在本块里等于免费热重载，不丢任何东西。
  刷新、断线、重建，第一帧 snapshot 就把屏幕补回来。

## 三条硬规矩

1. **只认主线。** `GET /api/sessions` 把 `parent != null` 的 fork 全滤掉。会话树的分支在
   qrd3wh 里是真的，只是这一页不显示。要改这条先改这里。
2. **地址不拼。** qrd3wh 的 URL 只从 `__nous/resolve/qrd3wh` 拿，缓存在 `server.js` 的 `engines` 里。
   搬走、换域名、进容器，这里一行不改。`BJES6B_ENGINE_URL` 只是本地验流式用的逃生口。

   **宿主的地址按请求现算，不认死 `NOUS_ORIGIN`。** `NOUS_ORIGIN` 钉的是「外面看到的名字」
   （`http://<Host>`），从 Tailscale / 任何反代进来时它既丢了协议、也少了路径前缀，还跟着
   被复用的进程活一整轮 —— 那就是当初那句 `fetch failed` 和后来那句
   `resolve 失败（试过 https://… , http://… , http://127.0.0.1:80）`。

   所以宿主现在多给一个 `NOUS_LOCAL_ORIGIN`（块 `blocks.ts` 的 spawn env 里）：那是宿主
   **自己听的那个地址**，反代、隧道、域名都改不了它。候选表在 `originsOf()`，按可靠性排：

   1. `NOUS_LOCAL_ORIGIN`（`localhost` 一律换成 `127.0.0.1` —— 有的机器先解析到 ::1）
   2. `x-forwarded-proto` + `x-forwarded-host` —— 只有反代把整条前缀也透传时才通
   3. `NOUS_ORIGIN` 与从它抠出的回环

   全不通才报错，报错里把这几个候选一起说出来。**`/api/health` 看一眼 `origins` 就知道现在
   在试哪几个。**
3. **流式不要攒。** SSE 原样透传，`res.write` 之后不许等。前端那边按 `requestAnimationFrame`
   合帧，**一个 token 一次重排**是这一页最容易犯的错。

## 接口（这一块自己的）

```
GET  /api/health                 { ok, origins, engines }  ← 连不上引擎时先看这个
GET  /api/sessions               { items: [{id,name,model,busy,last}] }   ← 不含分支
GET  /api/sessions/:id/entries   { items: [] }                             ← 新的在前，这里翻正
GET  /api/sessions/:id/events    SSE：snapshot → message_update → message_end → run_end
POST /api/sessions/:id/messages  { content, requestId }                    ← requestId 幂等
POST /api/sessions/:id/abort
POST /api/sessions               { preset, name }                         ← 从一个模板开一条
GET  /api/presets                qrd3wh `v1/presets/Session-Template` 原文
```

上游是 qrd3wh 的 `/v1/*`，形状一模一样。**别在这一层加业务字段** —— 加了就是第二个真相。
`/api/presets` 尤其：引擎给什么就递什么，筛 / 补 / 算状态全在浏览器里（或在 3699b9），
BFF 里多一行就是多一个真相源。
`relay()` 也是这个意思：上游的状态码和 body 原样转出去，引擎那句 `{error}` 一个字都不包。

## 第二个 tab：Template（模板 = 通讯录）

第二个 tab 叫 **Template**，点进去是微信通讯录那一副骨头：顶部导航（标题 + 右上角两个图标）/
字母分组 / 右边的 A–Z 标尺 / 一行一个模板（字母砖 + 名字 + 摘要）。**它显示的就是 3699b9 对着的
那份数据** —— `Session-Template` 那一段预设（`Nous-Template` 仓）。

**不是把 3699b9 的页面搬过来。** bjes6b 一行代码都不 import 那边，也不拼它的地址：
两边各自问 `qrd3wh` 的 `/v1/presets/Session-Template`，BFF 把响应原样递出来。段名写死在
`server.js` 的 `PRESET_SECTION` 一处，换段只改那里。

- **能否启动，判据全是引擎算好的字段**（`tplState()`）：`specError` → 写坏了；
  `resolved.*.state === "invalid"` → 引用失效；`tracked === false` → 盘上有仓里没。
  这一块**不自己拼规则** —— 加了规则就等于可以跟引擎吵起来。
- 不能开的行发灰、按下去只 toast 一句为什么；能开的行点一下 = `POST /api/sessions {preset, name}`，
  `name` 带时间（同一个模板开两条，列表里分得出来），成功后直接进会话页。
- **认引擎回的生效值**：`missing[]` / `missingExtensions[]` / `missingTemplates[]` 要点名，
  静默降级比报错难查。
- 搜索不是常驻入口：顶部右上角是**放大镜 + 添加朋友**两个图标（微信同款），点放大镜才把导航栏
  换成搜索条 + 「取消」；取消 / 离开本页都退出搜索态。搜索是**内存里筛**（名字 / 摘要 / 说明 /
  类别），一次输入不发请求。添加朋友先占位（`data-todo`，弹「还没做」）—— 新建模板是 3699b9 的地盘。
- 分组按名字首字母，非 ASCII 归 `#`。要换成按类别分组，改 `letterOf()` 加一张表就够。
- 标尺只放真是这些字母的，按住上滑就跳（`pointercapture` + 邻近取中心）。点了没反应比少一颗更糟。
- 侧边标尺是绝对定位的，所以 `.row.tpl` 右边留 28px 给它。

**页面切换走 hash**：`#/` 列表、`#/templates` 模板、`#/c/<id>` 会话，`show(view)` 一处管三页
（会话页藏 tabbar 靠 `body[data-view="thread"]`）。后退键能用，是因为 hash 是唯一的状态源。

## 前端的两条性能约定

- 助手消息用 `blocks: Map<contentIndex, string>` 攒，`text_delta` 只改 Map，**rAF 里才写 DOM**。
- 气泡 `contain: layout paint`，滚动容器 `overscroll-behavior: contain`，键盘高度用 `visualViewport`
  而不是 `100vh`。这三条是手机上「像微信」和「像个网页」的分界线。

markdown 只渲染到「能读」：标题 / 列表 / 围栏 / 行内码 / 粗体。别引 parser，一个聊天页不值得。

## 故意还没做的

| 东西 | 为什么 |
|---|---|
| 头像 | 逻辑没定。现在是空方块（`.avatar`），换真图只改这一个 class 的来源 |
| 顶部右按钮、点进会话后的 `···` | 只有皮，点了弹「还没做」 |
| 底部另外两个 tab | 名字就叫「未开发」。第三个（发现）、第四个（我）先跟这条对齐 |
| 新建会话 | `＋` 走 `data-todo`。qrd3wh 的 `POST /v1/sessions` 要 `model`，怎么选还没定。（**从模板开**已经能用：`/v1/sessions {preset}`，见 Template 那一节） |
| 置顶 | **已做，照微信节奏**。长按 400ms 到阈值 → 震一下 + 这一行变暗（`holding`），菜单**不**出现；松手才从手指位置弹出黑浮层（`rgba(0,0,0,.8)`、带指向手指的小三角、弹簧 scale 动画、锚点默认手指下方不够就翻上方）。电脑右键当场弹。菜单项「置顶该聊天 / 取消置顶」。状态落本块 `data/pins.json`（**不进引擎** —— 列表排序是这一页的事），`POST /api/sessions/:id/pin {pinned}`。前端乐观更新，失败回滚。置顶区内按 pins.ids 的先后（新置顶的最前），非置顶区按 `last.id` 降序。探会话存在性用 `POST /v1/sessions/:id/config`（引擎没做 `GET /v1/sessions/:id`）。菜单项按住滑出松手 = 取消（`pointercancel` 也归这条）。右键**在 `contextmenu` 事件里 `preventDefault` + 弹菜单**（跟宿主 `NodeMenu.tsx` 同一套）——放在 `pointerdown` 里弹、再另挂 contextmenu 去拦是两套逻辑，原生菜单照样弹（多浏览器实测过）。别改回去 |
| 分支 / fork | 这条不是「没做」，是**不做**，见上面第 1 条 |

## 跑与验

```bash
node server.js                                  # 脱开宿主：PORT/NOUS_ORIGIN 用默认值
PORT=4812 BJES6B_ENGINE_URL=http://127.0.0.1:4811 node server.js   # 指一个假引擎验流式
```

Template 那一页有一份真点一遍的冒烟脚本（CDP 驱动 chromium，`test/smoke-templates.mjs`）：

```bash
# 先起 chromium 于调试端口，再把宿主的地址给它
chromium --headless=new --no-sandbox --remote-debugging-port=9231 --user-data-dir=/tmp/cdp about:blank &
node test/smoke-templates.mjs      # 默认 http://127.0.0.1:4790/p/new1/bjes6b/ ，BASE/PORT 可覆盖
```

它**会真开一条会话**（点一行 = 建会话），这是故意的：不真开就验不到「生效值」和跳转。
引擎没有删会话的路，跑完那几条 `scratch MM-DD HH:MM` 会留在列表里。

手机上装：这一页是 PWA，但**只有安全上下文能装**。仓库根那台机子已经挂好了：

```
https://9996696.tail8d0018.ts.net/nous/p/new1/bjes6b/
```

（`tailscale serve --bg --set-path /nous http://127.0.0.1:4790`，tailnet 内可见。）
`http://IP:4790` 下是没有 service worker 的 —— 别在那上面测离线，会以为自己写错了。
