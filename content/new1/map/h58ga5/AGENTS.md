# h58ga5 · Nous-Desktop

Nous 的**桌面客户端**（微信桌面版的样子）。GPUI + `gpui-kit` 原生应用，不是网页。

> **UI 框架钉死：必须用 [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)**
> （`gpui-kit = "0.9"`，crates.io 同名包，Apache-2.0）。
> **不许换框架、不许绕开它直接用 `gpui-pre-*`、不许把 Tauri / Slint / egui 加进来。**
> 组件怎么用、文档在哪、有哪些现成的例子 —— 一律以那个仓库为准：
> <https://github.com/longbridge/gpui-kit> · 文档 <https://gpui-kit.com>。
> 要换框架先说清楚为什么，写进 `decisions.md`，别默默换。

- **引擎在 `qrd3wh`**（Nous 的 session 块）。这一块**不跑引擎，也不是引擎的一部分**。
- 手机端是 `bjes6b`（Nous-Mobile）。两边是同一个产品的两个客户端，对同一个引擎。
- 这一块是**项目本体 + 文档的落脚处**。源码放这儿，跑在你自己的桌面上。

## 三条硬规矩

1. **这一块不是块页面。** 没有 `package.json` —— 有了 `scripts.start` 宿主就当它是网站，
   spawn 进程、给空白页、崩了留墓碑。别为此加脚手架。
2. **这台机器 build 不了。** headless Debian，没有 DISPLAY / Wayland。改代码在这儿，
   `cargo check` / `clippy` / `cargo test` 在这儿，看得见的东西去你自己的桌面上跑。
3. **地址只有一个来源。** 引擎地址从宿主的 `__nous/resolve/qrd3wh` 拿，配置里只写
   **宿主地址 + board**，禁止把引擎路径写进 `Cargo.toml` / 常量 / 文档示例。

## 工程规矩（别破）

- **踩一个坑就往 `坑.md` 加一条。** 三行以内：症状 → 根因 → 解。追加到末尾、编号顺延，
  别改写旧条目、别写成日记。同一个坑踩第二次是这一条失灵的信号。
- **SSE 合帧到 16ms 再 `cx.notify()`。** `bytes_stream` 往 buffer 里塞，定时器读一次再通知 UI。
  一个 token 一次 `cx.notify()` 是这一层最容易犯的错，和 bjes6b 手机上那条 rAF 规矩一个字不差。
- **未知字段用 `serde_json::Value` 收着原样透传。** 不许 `deny_unknown_fields` —— 那个 flag
  会让引擎每加一个字段这边就编译不过。
- **协议是哑的。** 客户端只转发 `/v1/*`，不加业务字段、不在客户端算状态。
  `/v1/*` 的类型变化先改 `crates/nous-engine`，UI 层一行 HTTP 都不许碰。
- **只吃 `gpui-kit` 的重导出。** 永远别在自己的 `Cargo.toml` 里再写一条 `gpui-pre`，
  否则版本对不上时会在 `Entity<T>` 不匹配上耗一整天。

## 置顶的真相

在 **bjes6b**（`data/pins.json`）。桌面端不自己存一份 —— 写穿过去走
`POST /p/demo/bjes6b/api/sessions/:id/pin`，地址从 `resolve` 拿。宁可多一次 HTTP、
坏了 404 响得很明显，也不要两份互不相干的置顶表。要改这条，见 `decisions.md`。

## 启动 / 开发

```bash
cargo check          # 这台机器上唯一的日常
cargo run            # 你自己的桌面上
```

### headless 机器上看窗口（本机 99966 已配好）

GPUI 能在 Xvfb 里跑，三个坑都踩过了：

1. **mesa 必须 ≥ 25.0.7**（bookworm-backports）。22.3.6 的 llvmpipe Vulkan WSI
   会让窗口黑屏/灰屏（swapchain 内容落不到窗口），症状是截图纯黑且无报错。
2. **要跑一个 compositor**（`picom --backend xrender`）：GPUI 用 depth32 带 alpha 的
   Visual，没有 compositor 合成，画面一样出不来。
3. **GPU 程序不需要指定**：GPUI 自己枚举并测试 adapter，选 llvmpipe (Vulkan)。

```bash
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 picom --backend xrender &
DISPLAY=:99 target/debug/nous-desktop &
DISPLAY=:99 import -window root /tmp/shot.png   # 截图看
```

注意 `CARGO_TARGET_DIR` 在这台机器上全局指向 `/opt/Warehousetui/target`，
二进制不在项目的 `target/` 里。

配置查找顺序：`NOUS_DESKTOP_CONFIG` → **exe 旁边的 `desktop.toml`**（发布包带的那份）
→ `%APPDATA%\nous\desktop.toml`。**具体某台机器的宿主地址不进仓库** ——
发布包里的 `desktop.toml` 由 CI 从仓库 secret `NOUS_HOST` 生成（见 `.github/workflows/windows.yml`）。
只有两个键：

```toml
host = "http://127.0.0.1:4790"  # 走 tailnet/反代就写那个地址
board = "new1"
```

启动时打一次 `<host>/p/<board>/h58ga5/__nous/resolve/qrd3wh` 拿引擎地址，然后直连引擎。
引擎版本进日志 + About 面板 —— 以后「怎么突然坏了」先看是不是引擎先动的。

## 文档

- `README.md` —— 给人看的：这是什么、怎么跑。
- `wire.md` —— **唯一会漂的那份**：我们依赖了引擎 `/v1/*` 的哪些字段、引擎改了会怎么坏。
- `decisions.md` —— 选了什么 + 为什么 + 什么时候该反悔。一行一条，追加式。
- `坑.md` —— **开发中踩的每个坑**。症状 / 根因 / 解，三行以内。踩到新的就加。
