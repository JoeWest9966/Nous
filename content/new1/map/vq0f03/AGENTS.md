# vq0f03 —— 这个项目 vps 侧的控制台

一张拓扑 + 一个检查器。左边是清单的唯一来源（j1900 上的 `nw-panel`），右边是按标签分组的机器；
连线上的毫秒是**本机对编目里那个端口**的实测握手耗时。点一台机器：看详情、覆盖登记、删掉它。

页面读 `/api/bootstrap` 一次拿全，写操作走同源代理到上游 —— 这个块自己只存一样东西：
`data/vps.json`，也就是「这台机器是干嘛的、有哪些坑」这层注解，上游没有这层。

## 上游契约（从面板前端 bundle 里核出来的，不是猜的）

```
GET    /api/vps                 nw/vps-ls.v1        { machines[] }
GET    /api/vps/<name>/secret    vq0f03/vps-secret.v1 { name, kind, key }  ← 只转**密钥型**；只打回环面板
POST   /api/vps/<name>/bootstrap-key  vq0f03/vps-bootstrap.v1 { name, kind:"key", fingerprint, action }
POST   /api/vps                 nw/vps-register.v1  {name, ips[{label,addr}], user?, port, note?, tags?, password? | keyFile{name,content(base64)}}
PUT    /api/vps/<name>          同上，凭证留空 = 保留旧的（= 重新登记）
DELETE /api/vps/<name>          nw/vps-rm.v1
GET    /api/tags                nw/tag-ls.v1        { tags[{name, refs[{kind,id}]}] }
POST   /api/vps/<name>/tags     {name}              ← 只认已存在的标签
DELETE /api/vps/<name>/tags/<tag>
```

三条会咬人的语义：

1. **登记会先做 SSH 连线测试**（打第一条地址，10 秒超时），连不上不落盘。所以写接口超时给 30 秒，
   前端写着「连线测试中…」。
2. **密码明文进仓**（面板 #194 自己写的）。密钥走 `keyFile{name, content(base64)}`，只进 stdin。
   这个块的代码里一行日志都不打凭证，前端提交完就把密码从内存里清掉。
3. **名字是唯一键，没有 rename**：改名 = 删旧建新。所以编辑表单里名字是锁死的。

免费附赠的一条：**删机器不会清 `tags.json`**，标签会继续指着已经不存在的名字。页头那条黄提示
（`标签 X 指向的机器已经不在编目里了`）就是在说这个。

## 本块的 API

页面用的那几个，agent 也能直接用（信封和 nw 同形：`{schema, ok, errors[], warnings[], at, data}`）。

| 端点 | 说什么 |
|---|---|
| `GET /api/bootstrap?fresh=1` | 页面一次拿全：机器 + 标签 + 上游状态 + 注解 + 坑 |
| `GET /api/vps?fresh=1` | 机器清单 |
| `GET /api/vps/<name>` | 一台（带注解） |
| `GET /api/vps/<name>/ssh` | 连它的命令（现拼，不存盘） |
| `GET /api/vps/<name>/secret` | 那台机器的凭据本体。**只转密钥型**：密码型回 409 并说明要装公钥。上游那条在回环面板上（`X-NW-Reveal: 1`，`NW_SECRET_BASE` 默认 `http://127.0.0.1:18080`），远程门里没有 |
| `POST /api/vps/<name>/bootstrap-key` | 密码型 → 密钥型，**一次性**。现生一对 ed25519 → 用密码把公钥追进目标机的 `authorized_keys` → 用新私钥**验通** → `PUT` 覆盖登记（只给 `keyFile`，上游的凭证闸会把密码清掉）。密码不进 argv / 日志 / 响应：走 `SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force`。验不通就不登记 |
| `POST /api/vps` | 登记（代理 POST） |
| `PUT /api/vps/<name>` | 覆盖登记（凭证留空 = 保留旧的） |
| `DELETE /api/vps/<name>` | 从编目里删掉 |
| `POST /api/vps/<name>/tags` | 挂一个**已存在的**标签 `{name}` |
| `DELETE /api/vps/<name>/tags/<tag>` | 摘标签 |
| `GET /api/tags` | 标签编目，带它挂着哪些机器 |
| `GET /api/probe?name=<n>[&addr=&port=]` | 现探一次 TCP，回 ms + banner + 解析出的服务/发行版 |
| `GET /api/health` | 自己 + 上游活没活 |
| `GET /api/catalog` | 给 agent 的全量：机器 + 注解 + 上游端点表 + 坑 |
| `GET /api/upstream` | 上游原始信封，一字不改 |

写路由是**白名单**（就那 5 条），不做通用透传 —— 否则整个 nw 面板的写面就挂在这个块的地址上了。
探活同样是白名单：只探编目里写过的 `(addr, port)`。

## 右键菜单（三个入口，一套菜单）

| 右键在哪 | 菜单里有什么 |
|---|---|
| 机器卡 | 打开详情 · 探测 · 复制 ssh · 复制地址 · 覆盖登记 · 挂/摘标签 · **从编目里删除…** |
| 组框（标签） | 探测这一组 · 复制机器名 · 把整组挂到另一个标签 |
| nw-panel 那块 | 打开面板 · 检查一次 · 刷新 · 打开 api/vps · 复制 catalogPath |
| 画布空白 | 登记服务器 · 刷新 · 检查上游 · 复制整份清单 JSON · 打开 api/catalog |

几个决定，都是被坑出来的：

- **不顺手打开抽屉。** 右键只出菜单：抽屉一开，画布重排，菜单底下的东西就在脚底下动了。菜单里的动作
  拿的是「打开菜单时的那台」，不看 `S.selected`。
- **标签直接摆成动作，不做二级菜单。** 「挂 important」平铺一行，比 hover 出一层再点一次强 ——
  二级菜单在触屏上是「点进去、点回来、点丢了」。
- **删除不在菜单里直接执行。** 它只把抽屉打开并把第二段确认摆好：确认只有一套，第二套就是第二套谎话。
- **触屏长按 550ms = 右键**，手指一动就取消（那是在滚画布）。长按开完菜单后手指抬起会补一个 `click`，
  那个 click 必须吞掉，否则「长按看菜单」变成「长按看菜单 + 抽屉也开了」。
- **键盘一样能用**：Shift+F10 / 菜单键打开，↑↓ 绕圈、Home/End、Enter、Esc（焦点还回卡片）。
- 菜单是 `fixed`，所以滚动/ resize 一律关掉 —— 不然它飘在另一个东西上面。

写操作可能撞上上游的写锁：`409 LOCK_CONFLICT`，说明是「数据仓 /opt/Data-001 已有写命令在跑(pid X)」，
hint 直接给锁文件路径。这是上游的真相，别在页面上改写成一个「失败了」的糊话。

## 文件

```
server.js          代理 + 注解层 + 探活 + 静态；零依赖
public/index.html  壳（拓扑 + 抽屉 + 表单）
public/style.css   暖白纸 + 发丝线；数字一律等宽
public/app.js      拓扑是 DOM 卡 + SVG 折线（坐标从 DOM 量，resize 重量）
data/vps.json      注解层 + 上游端点表 + 坑（5 秒缓存，改完不用重启）
```

跑：`npm start`。宿主给 `PORT` / `NOUS_DATA`；`NW_BASE` 可换上游（指个死端口就能验 503 那条路）。

## 加一台机器

正规路子是页面上点「登记服务器」—— 它走的是上游的 `nw vps register`，会 SSH 连线测试、会落盘。

绕过去直接改 `j1900:/opt/Data-001/vps/vps.json` 也行，但那就跳过了测试和凭证校验，
下一次别人在页面上覆盖登记时会覆盖你。

注解（`data/vps.json` 的 `machines.<name>.role/facts/caveats`）只影响这一块的页面和 API：
上游多一台这里没注解的机器，页面照常画出来，只是「它是干嘛的」那一段空着 —— 那不该由这里替它编。
