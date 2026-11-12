# NousDesk

Nous 的桌面客户端。微信桌面版的样子，原生应用，不是网页。

- **引擎**：`qrd3wh`（session 引擎）。这个客户端只做皮 + 一个哑的协议层，不跑 agent。
- **手机端**：`bjes6b`（Nous-Mobile），同一套引擎，另一个客户端。
- **技术**：Rust + GPUI，UI 框架是 [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)（`gpui-kit 0.9`）。没有 webview，没有 PWA。

## 跑起来

```bash
cargo check          # 只检查，不跑（开发机 headless）
cargo run            # 自己的桌面上
```

配置查找顺序（先到先得）：

1. `NOUS_DESKTOP_CONFIG` 环境变量指向的文件（调试用）
2. **exe 旁边**的 `desktop.toml` —— 发布包里带一份，解压即用
3. `%APPDATA%\nousdesk\desktop.toml`（Linux `~/.config/nousdesk/desktop.toml`）
4. `%APPDATA%\nous\desktop.toml` —— 改名前的旧目录，**只读**：老用户的配置还能接上，
   写回时仍然写回它（不搬家，免得用户以为配置丢了）

```toml
host = "http://192.168.1.7:4790"
board = "new1"
```

一个都没有也能开：用默认值（`192.168.1.7:4790`），连不上时左栏会把该改的路径显示出来。

启动时从 `<host>/p/<board>/h58ga5/__nous/resolve/qrd3wh` 拿引擎地址，然后直连引擎。

## 结构

```
crates/nous-engine/   协议客户端：/v1/* 的 serde 类型 + SSE 流。UI 一行都不许进。
crates/nousdesk/  GPUI 应用：三栏布局、会话、输入、置顶。
```

分层是硬的：UI 层不碰 HTTP，协议层不碰 GPUI。以后想换 UI（Slint / 别的）只动 `nousdesk`。

## 三条规矩

1. 引擎地址只从 `resolve` 拿，不写进配置、不写进常量。
2. 协议是哑的：不加业务字段、不在客户端算状态。字段变化先改 `nous-engine`。
3. 置顶的真相在 bjes6b，桌面端跨块写，不自己存一份。

详情见 `AGENTS.md`（给 agent 的）和 `wire.md`（协议契约）。

## New Manual（「盘面」）

点左栏右上角的「+ New Manual」——**只顶替右侧聊天区那一格**，左导航和会话列表
永远在。Esc 或点左栏任意一条会话就回到聊天。外形照 qrd3wh 的面板页
`/p/<board>/hb7k3j/`（Pele / Flexoki dark）：四块象限 + 中间一颗虚线椭圆，
模型 / 上下文 / 扩展 / 权限各占一角，建会话的参数全在椭圆里。右格里**没有顶栏**。

- `crates/nousdesk/src/board.rs` —— 状态 + 渲染 + 五个弹层
- `crates/nousdesk/src/board_art.rs` —— 调色板、cqmin 尺寸、那张 Cynefin 图的路径
- 四块象限和 cliff 用 `PathBuilder` 现画（`canvas` + `window.paint_path`）：
  `svg()` 元素只走单色 alpha 蒙版，四块色会糊成一块
- 尺寸全走**右格 cqmin**（`art::M`）：右格短边 = `min(右格宽, 窗高)`，
  右格宽由 `app.rs` 里那几个栏宽常量算出来。按整窗 vmin 算会在窄右栏里挤爆。
  右格变了跟着缩，一屏不滚 —— 和那个页面一样
- 模型那一格是三列**铁路图**（供应商 › 模型 › 思考），技能 / 模板 / 目录是平铺清单：
  前者是有依赖的语法链，后者不是。为版式统一把语义搞错不划算

调试两个开关（只为了不用每次手点）：

```bash
NOUS_OPEN_BOARD=1 ./nousdesk.exe              # 一启动就开在右格的盘面上
NOUS_OPEN_BOARD=1 NOUS_OPEN_SHEET=model ./…exe    # 再直接开在某个弹层上
# sheet ∈ model | skills | prompt | dir | machine
NOUS_DESKTOP_OPEN=settings ./…exe                 # 一启动就开在左栏底部「≡」的设置页
# menu | settings
```
