# zefsvf · tailnet 面板

这台机器（`9996696`）眼里的整个 tailnet：谁在、IP 是什么、通不通、走直连还是 DERP。

## 它怎么拿数据

只有一条来源：本机的 `tailscaled`。

```
tailscale status --json     →  nodes / services / daemon / self
tailscale ping -c 2 --timeout=1s <ip>  →  直连还是 DERP、多少毫秒
```

不要 Cloud API key、不联网、不在 `data/` 里维护第二份机器清单。
云端 control plane 是 tailscaled 的上级；这一页要看的是「此刻这台机器能连到谁」，
所以真身是本机的 daemon，别的都是缓存。

## 接口

```
GET /api/status[?fresh=1]   快照：{ ok, stale, at, tailnet, daemon, self, nodes[], services[], counts }
GET /api/ping?ip=<ip|id>    一次 ping：{ ok, ms, direct, region, addr } | { ok:false, error }
GET /api/tcp?ip=<vip>&port= 服务的端口探活：{ ok, ms, error }（只测它自己广告过的端口）
GET /api/registry           服务台账：{ ok, at, registryPath, entries[] | error }；每条带 doc / docExists
GET /api/probe?node=&port=  台账里某条现在通不通：{ ok, ms, error } | 404「台账里没有这一条」
GET /healthz
GET /…                      public/ 里的静态文件
```

- `status` 缓存 2.5 秒（多个标签页同时轮询时只打一次 tailscaled）。
- `ping` 结果缓存 30 秒；只接受**快照里存在**的地址，别把它变成任意地址探测器。
- `svc:` 服务不能用 `tailscale ping` —— 它不是 peer，只会超时。所以服务卡片上那个按钮
  是 TCP 探活，不是 ping。这是两种不同的真相，按钮上就说了两种不同的话。
- tailscaled 死了：返回上一次落在 `data/status.json` 的快照，`ok:false`、`stale:true`，
  页面顶部挂横幅。宁可给你看 4 分钟前的真相，也不给一片空白。

## 服务台账：tailscaled 不知道的那一半

tailscaled 说的是「谁在、IP 是什么」。它不知道 j1900 的 `8443` 上是 nw 面板、该用 https
还是 http、要不要带 token、挂了重启哪条命令 —— 这些是**人写的**，住 `data/registry.json`
（和 `status.json` 一样落 `NOUS_DATA`，`cp -r` 连它一起走）。

一条台账长这样，加一条服务 = 往 `entries` 里加一个对象，不用改代码：

```json
{
  "id": "warehouse",
  "name": "Nous Warehouse",
  "kind": "nw 面板 + REST 面",
  "node": "100.126.95.105",
  "port": 8443,
  "url": "https://j1900.tail8d0018.ts.net:8443",
  "doc": "/opt/Nous/content/new1/map/zefsvf/warehouse.md",
  "scheme": "https 还是 http、谁终止 TLS",
  "auth": "要不要 token、token 在哪",
  "facts": [{ "k": "API base", "v": "…" }],
  "caveats": ["踩过的坑"]
}
```

两条纪律：

- **台账同时是白名单。** `/api/probe` 只认台账里写过的 `(node, port)` 对，别的回 404 ——
  这个端点因此还是探活，不是端口扫描器。别把它改成「传什么探什么」。
- **台账不是第二份机器清单。** 机器、IP、在线状态永远只有 tailscaled 一个来源；
  这里只写它答不出来的那部分（哪个端口值得点、怎么连、怎么重启）。
- **`doc` 是给 agent 读的。** 绝对路径，指向一份按 skill 写的说明（frontmatter + “项目是干嘛的 +
  API 怎么用 + 挂了怎么重启”）。页面上那个 `skill` 按钮复制的就是这个路径，不是正文 ——
  文件是活的，贴给 agent 的全文是快照。文件没了就不生成按钮，meta 行改显「说明缺失」。
  这个路径**只能**来自 `registry.json`，不接受请求参数，否则就是「任意文件存不存在」的探针。

改地址、改重启命令时，`registry.json` 和它指向的那份 `doc` 两处都要改 —— 目前两份都在手写。

## 页面：一屏结构，细节按需

这一版是重写的。上一版把 8 台机器的 IP、OS、收发、key 到期、5 个 VIP、台账的 9 条 fact
全摊在同一页上往下滚 —— 那等于什么都没说清。现在是**图 + 抽屉**，形状照
[pele 的 architecture 示例](https://pele.run/examples/architecture)：48px 图标方块、集群框、
直边箭头，连线吸附在方块边上。

```
这台机器 ──┬── 在线 3 台          ┌ 台账（人写的：哪个端口值得点）
           │                     └ 服务 VIP（tailscaled 广告的，不知道挂在哪台机器上）
           └── 离线 4 台（暗着，虚线框）
```

### 硬规矩

- **图按容器宽度 1:1 画。** `viewBox` 用容器宽度，不缩放：缩到 0.5，12px 的字就是 6px。
  窗口一变就重排（ResizeObserver），不是缩放一张大图。
- **`#map` 自己就是那张 svg，不许再嵌一层 `<svg>`。** 踩过：内层 svg 的 `width` 属性
  是布局宽度，外层 CSS 的 `width:100%` 管不到它，手机上直接横向溢出。
- **几何常量都在 `app.js` 顶上**：`CELL_W 124` / `TILE 48` / `ROW_H 100` / `LABEL_H 36`。
  `ROW_H` 留到 100，是为了「上一行的 IP」和「下一行的链路标签」不打架（92 时差 4px，会叠）。
- **线先算直线，撞到别的方块才改直角。** 撞没撞用 `crossesTile()` 采点判断（连方块下面的
  名字一起算）。窄屏折成两行时全靠这条，否则线会从别人的图标上碾过去。
- **跨列的那条线（台账挂在哪台机器上）只在宽屏画。** 窄屏是竖着摞的，一条横穿五层的线
  不如让方块自己写 `j1900:8443` —— 所以台账方块的副标题是「机器:端口」，不是「:端口」。
- **服务 VIP 和任何机器都不连线。** tailscaled 只告诉你「有这个 VIP 和这个端口」，
  没告诉你在哪台机器上；画一条猜的线，就是把猜的当事实。台账那条线是人写的，才画。
- **离线的不掉出图。** 暗一点 + 虚线框 + `76 天前`，比一个 `offline` 有用。
- **线只表示关系。没 ping 就是灰的、没有数字。** 别用颜色暗示一个没测过的链路。
- **首次加载自动 ping 一遍**（每台一次、串行、30 秒服务端缓存），以后按需。
  图上那些线因此一进来就有数字；每 5 秒烧一遍网络这种事不做。全部重测点「全部 ping」。
- **ping 的颜色就是链路质量**：绿=直连、黄=DERP、红=不通、灰=没测。
- **点方块开详情抽屉**（桌面是右列，手机上是从底下推上来的 sheet + 遮罩）。
  IP、OS、收发、key 到期、ping 按钮、台账 facts/caveats、doc 路径，全在那儿 —— 一次只出现一块。
- **搜索只改 `opacity`**，不重画抽屉；输入焦点在搜索框上，抽掉它才是毛病。
- **复制的是路径不是全文**（台账里的 `skill`/doc 按钮）：文件是活的，贴在对话里的快照不是。
- **IP 还是主角**，只是从「一行一个大号 IP」变成「方块下面那行等宽小字 + 抽屉里能点着复制」。
  方块底下的副标题就是 IP（台账是「机器:端口」，VIP 是「:端口」）。

## 文件

```
server.js          tailscaled 的适配层 + 静态服务，零依赖
public/index.html  骨架：顶栏 / 图 / 详情抽屉 / 图例
public/app.js      取数 → 布局（scene/place/box）→ 画 svg → 抽屉；ping、探活、复制
public/style.css   暗色；svg 里的方块/框/线各一段，≤900px 抽屉变底部 sheet
data/status.json   上次成功的快照（宿主不建，第一次成功时自动建）
data/registry.json 服务台账（人写的；不存在就是「还没记过」，页面不报错）
warehouse.md      台账里那条 `doc` 指向的说明，按 skill 写（agent 进去先读它）
```

`app.js` 里的分工，改之前先认路：

| 东西 | 干嘛的 |
|---|---|
| `tiles()` | 快照 + 台账 → 方块（label / sub / icon / tone / hay） |
| `scene(width)` | 排座位：`box()` 造集群，`fitCols()` 定列数（4 块给 2×2，不给 3+1），`place()` 落坐标 |
| `edgeGeom()` | 直线 / 直角绕行 / 右列翻越；`crossesTile()` 是它的判据 |
| `renderMap()` | 上面那些 → 一段 svg 字符串；`#map.innerHTML` 只放子元素 |
| `renderInspector()` | 选中的那个方块 → 抽屉正文（按 kind 分三种） |

## 版本

这个文件夹自己有一个 git —— 外层 `content/` 整个被宿主仓库忽略（那是刻意的，见根 `AGENTS.md`），
所以块的历史只能落在这儿：

```sh
git -C content/new1/map/zefsvf log --oneline
```

- `data/registry.json` **跟着版本走**：它是人写的台账，改错了要能退回去。
- `data/status.json` **不进版本**：那是 tailscaled 的一次缓存，谁 checkout 都会拿到一份过时的
  「上次成功」，不如让它重新抓。
- 跟宿主仓库互不干扰：`.git` 只在这一层，外层 `.gitignore` 挡着 `content/`。
- commit 之后宿主会看见文件变了、把这个块的进程收掉，下次请求自己起来 —— 无害。

## 搬走跑

```sh
cp -r map/zefsvf ~/zefsvf && cd ~/zefsvf
PORT=4781 node server.js
```

`PORT` 必须给；`NOUS_DATA` 不给就落在自己文件夹的 `data/`。
页面里所有链接都是相对路径，所以 `/p/new1/zefsvf/` 底下打开和 `localhost:4781` 打开是同一页。
