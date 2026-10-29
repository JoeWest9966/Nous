# hb7k3j · session-new —— 新对话的发起页

本块是一个纯静态页面（`index.html`，宿主内置空跑 server 端出去，没有自己的后端）。

## 页面结构

盘面是 **Cynefin**（pele.run/examples/cynefin）的**原路径重绘**，不是仿形：

- 几何：`viewBox 0 0 640 440`，中心 (320,220)，confusion 椭圆 rx88/ry64，
  四域边界、cliff 全是 pele 的 `d` 平移 −8/−44 得来，`preserveAspectRatio="none"` 拉伸填满，
  描边用 `vector-effect="non-scaling-stroke"` 保持像素级粗细。
- 配色：pele 的 dark 主题原变量（Flexoki）—— bg `#100f0f`、盘面与节点填 `#1c1b1a`、
  边 `#403e3c`、正文 `#cecdc3`、次级 `#878580`、accent `#4385be`，
  系列色 s1红 `#d14d41` / s2橙 `#da702c` / s3黄 `#d0a215` / s4绿 `#879a39` / s5青 `#3aa99f`。
  域填充 0.14、中心椭圆 0.18——数值与线上 demo 像素级对齐。
- 字体：站点同款，`-apple-system/Inter` 正文 + `ui-monospace/SFMono` 代码性标签。
- 只有 **r1（左上，红域）是活的**：一支模型节点，点它开弹窗；其余三域（上下文/工具/权限）
  只排标题 + 两行斜体副标题，跟 demo 的空域一个样子。
- **中央椭圆 = 新对话的出生点**：名字输入 + 创建。
- **一屏无滚动**：`html/body overflow:hidden`，盘面吃满视口；竖屏下椭圆收小、
  域的标题往外侧推（上两域顶、下两域底），保证谁也不盖谁。

## 它做什么

只做一件事：**选配置 → 创建新对话**。目前配置只有「模型」一项。

- 模型清单来自 `qrd3wh` 的 `GET /v1/models`（只列这台机器真能用的，不列内置目录）。
- 选模型走**苹果风格弹窗**：半透明材料（`backdrop-filter`）+ 压暗 scrim +
  从被点节点长出来（`transform-origin` 对准触发点），选中即收回、Esc/点 scrim 原路返回；
  `prefers-reduced-motion` 退化成淡入淡出。
- 点「创建」→ `POST qrd3wh /v1/sessions`，带 `{model, name?}`，成功后 `POST /v1/selection`
  把盘上的选中切到新对话。

## 它不做什么

- **没有对话列表** —— 那是兄弟块 `pxbgng`（session.2）的事。
- **没有对话内容** —— 那是父组 `qrd3wh`（session 引擎）的事。
- 不存任何状态：没有 `data/`，本页选中哪个模型只是页面内的临时选择；
  「默认模型」是 `qrd3wh` 的状态，存在它的 `data/default-model.json`。

## 对话的概念

fork 和 new sessions 都是「开启一个新对话」。区别只在入口：
本块发的是顶层新对话；fork 从某条历史长子枝，走 `qrd3wh` 的 `/v1/sessions/:id/fork`。

## 块间规则

- 地址只从 `__nous/ctx` 拿（先按 id `qrd3wh`，认不出按组名 `session` 找）。
- 只走 HTTP，不 import 别的块、不读别的块文件夹。
- 链接写相对路径，页面才能 `cp -r` 走了照样跑。
