# decisions — 选了什么、为什么、什么时候该反悔

追加式，一行一条。新决定加在**顶部**，别改写旧条目。

---

## 2026-11-08 · 仓库改为公开（MIT）

**为什么**：公库的 Actions 分钟数免费 —— Windows release build 每次 20 分钟，私有库是实打实计费；
这个项目也没有必须藏的东西。
**做了什么**：删掉硬编码的私有宿主地址（默认 host 改 `http://127.0.0.1:4790`）、
测试里的真实 URL/绝对路径换假、补 MIT LICENSE、**改写 git 历史并删库重建**
（force push 不够：GitHub 不回收对象，旧 commit 靠 SHA 仍可访问）。
**反悔点**：真要藏什么的那天，别指望删文件 —— 得换仓库。

## 2026-11-08 · UI 里跑 HTTP 的桥：一个 tokio runtime 挂全局

**为什么**：GPUI 的 executor 不是 tokio，而 nous-engine 的 HTTP（reqwest）全在 tokio 上。
main 里起一个 runtime、Handle 存 `OnceLock`，UI 的 `cx.spawn` 里用 `handle.spawn(...)` 跑 HTTP，
结果回传再 `cx.notify()`。
**反悔点**：以后要高频请求（SSE 重连、轮询），统一从 `crate::TOKIO` 走，别到处开新 runtime。

## 2026-11-08 · 用 GPUI + gpui-kit，不用 Tauri / Slint

**为什么**：原生、无 webview，`gpui-kit` 0.9.0 已上 crates.io，Apache-2.0，生产可用
（Longbridge Pro 在跑），组件里有 `bubble` / `message` / `stream-markdown` / `virtual_list` / `dock`。
**反悔点**：Linux 端中文输入法（ibus / fcitx）验下来真的不能用，再退回 Tauri。
**当时放弃的**：Tauri（webview，不真原生）；Slint（自绘，文本排版 / IME 全是自己写，工作量 3-4 倍）。

## 2026-11-08 · 这是独立项目，不是块页面

**为什么**：桌面客户端是常驻应用（托盘、通知、全局快捷键、关窗不退），不是浏览器里的一张网页。
宿主「打开」是开标签页，在这里跑不了。
**反悔点**：将来要一个跨平台又不想碰原生 build 时，回浏览器方案（bjes6b 加个 `/d/`）。
**代价**：文档页（`__nous/doc`）是这个块的正门，不是应用本体。

## 2026-11-08 · 不建 BFF，客户端直连引擎

**为什么**：原生应用不是宿主子进程，没有免费的反代。多一层 BFF 就是多一个进程要管、
多一层会漂的协议。
**反悔点**：引擎给客户端的形状越来越丑 / 需要聚合多端点时，加一个薄 BFF，但只做转发。
**代价**：`/v1/*` 的线协议是硬耦合，我们成了引擎的第三个消费者（bjes6b、3699b9、我们）。

## 2026-11-08 · 置顶的单源在 bjes6b

**为什么**：不搞两份互不相干的置顶表。桌面端跨块写 `POST /p/demo/bjes6b/api/sessions/:id/pin`，
地址从 `resolve` 拿。坏了 404 响得很明显，好过悄悄分叉。
**反悔点**：bjes6b 那个接口成了私有依赖、老变，就改成桌面端自己存 `~/.config/nous/pins.json`，
并在这里写明「两个端置顶不共享，故意的」。
