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

只做一件事：**选配置 → 创建新对话**。现在的配置有两项：**模型**和**思考档位**。

- 模型清单来自 `qrd3wh` 的 `GET /v1/models`（只列这台机器真能用的，不列内置目录）。
  清单里每条都带 `thinkingLevels` —— 这个模型真支持哪几档，由 pi-ai 的
  `getSupportedThinkingLevels` 算出来，非 reasoning 模型就是 `["off"]`。
- 选配置走**苹果风格弹窗**：半透明材料（`backdrop-filter`）+ 压暗 scrim +
  从被点节点长出来（`transform-origin` 对准触发点），选中即收回、Esc/点 scrim 原路返回；
  `prefers-reduced-motion` 退化成淡入淡出。
- 弹窗正文是**三列铁路图**（pele.run/examples/railroad 的语法重绘）：`供应商 │ 模型 │ 思考`。
  每列一条规则、每行一支药丸（choice 分支纵向排，首行在主轨道、其余行汇入左右两根锚定竖线），
  高度 = 最长一列的行数 × 40px。宽度装不下时列会摞成行（同一个 `drawRule` 换个位置，
  不是另一套版面）；思考列只在这个模型有得选（档位 > 1）时才出。
- 点「创建」→ `POST qrd3wh /v1/sessions`，带 `{model, thinkingLevel, name?}`，成功后
  `POST /v1/selection` 把盘上的选中切到新对话。
- **键盘流**：焦点是**列**不是项。`↑↓` 列内移动（到头就停，不绕圈）、`←→` 换列、
  `↵` 就是「点它」（走同一个 `actOn`，不写第二套判断）、`Esc` 收回、
  在药丸上直接打字 = 回搜索框接着打、从最顶上再 `↑` = 回搜索框。
  焦点存 `kb`（`{on,col,row}`）而不是靠 `document.activeElement`：`renderDiagram()`
  每次都把整个 `<svg>` 重建，真焦点会跟着旧节点一起消失 —— 换个模型列表就「键盘失忆」。
  重画后 `paintKb()` 把环钉回同一个格子并 `scrollIntoView({block:"nearest"})`。
  视觉上键盘环比 hover 重一档（`stroke-width:2` + 一圈光），暗底上分得出「我刚按到哪儿」。

### 「面板上写的 = 实际跑的」

这条是弹窗的唯一硬要求，别为了省事破它：

- **档位由模型能力决定，不由我们决定。** 面板只画 `thinkingLevels` 里那几档；换了模型，
  本地档位立刻按 pi-ai 同一套顺序（先往上找、再往下找）重新夹 —— 和服务端那次夹**同序**，
  否则就会出现「面板显示 A、存进去是 B」。
- **认服务端回来的值。** 创建后拿 `created.thinkingLevel` 覆盖本地 state，副标题也跟着它走；
  档位默认值是 `off`（对齐 pi-durable 的 `state?.thinkingLevel ?? "off"`），
  所以「没选」显示的就是真会跑的，不是「不知道」。
- 服务端还会再夹一遍（客户端发的值不可信），两边夹的规则一字不差，
  所以正常路径上第二次夹什么都不改。

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
