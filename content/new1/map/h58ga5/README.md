# Nous-Desktop

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
3. `%APPDATA%\nous\desktop.toml`（Linux `~/.config/nous/desktop.toml`）

```toml
host = "http://127.0.0.1:4790"
board = "new1"
```

一个都没有也能开：用默认值（本机 `127.0.0.1:4790`），连不上时左栏会把该改的路径显示出来。

启动时从 `<host>/p/<board>/h58ga5/__nous/resolve/qrd3wh` 拿引擎地址，然后直连引擎。

## 结构

```
crates/nous-engine/   协议客户端：/v1/* 的 serde 类型 + SSE 流。UI 一行都不许进。
crates/nous-desktop/  GPUI 应用：三栏布局、会话、输入、置顶。
```

分层是硬的：UI 层不碰 HTTP，协议层不碰 GPUI。以后想换 UI（Slint / 别的）只动 `nous-desktop`。

## 三条规矩

1. 引擎地址只从 `resolve` 拿，不写进配置、不写进常量。
2. 协议是哑的：不加业务字段、不在客户端算状态。字段变化先改 `nous-engine`。
3. 置顶的真相在 bjes6b，桌面端跨块写，不自己存一份。

详情见 `AGENTS.md`（给 agent 的）和 `wire.md`（协议契约）。
