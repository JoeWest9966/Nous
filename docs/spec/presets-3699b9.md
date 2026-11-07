# spec · 会话预设（preset）—— 块 3699b9 + 私仓 Nous-Template（`Session-Template` 段） + qrd3wh 的 /v1/presets*

**这份是唯一的约定来源。** 别的票/文档只写「执行本 spec 第几节 + 自己的验收」，不复制这里的形状。
改形状先改这里。

被引用的既有实现：`content/new1/map/6ysg01/`（面板骨架，抄它）、`content/new1/map/vbe4pt/`（管理页版式，抄它）、
`content/new1/map/qrd3wh/prompts-api.js`（写入口模板，抄它）、`docs/spec/skills-own-store.md`（仓库纪律，同一套）。

---

## 0. 一句话

`hb7k3j` 是**现场挑一套 agent 配置**（模型 / 思考档 / 技能 / 扩展 / 模板 / 只读 → `POST /v1/sessions`）；
`3699b9` 是**把那一整套存成一个有名字的 JSON**，下次点一下就开。

一块预设 = 一份对其他命名空间的**引用清单**。它不拥有任何内容：正文在 `Nous-prompts`，
技能在 `Nous-skills`，扩展在 `Nous-extend`，模型在编目。它只说「这次开会话，用这几个」。

```
Nous-Template/Session-Template/<名>/preset.json  ← 引用（provider/modelId + 技能名 + 模板名 + 档位 + 只读）
Nous-Template/index.mjs         ←   扫盘（按段） + 形状校验 + 写索引（这个仓自包含，不 import 别人）
qrd3wh/presets.js               ←   名册 + 引用校验 + 展开（纯模块，像 prompt-log.js）
qrd3wh /v1/presets*             ←   唯一写入口 + 唯一校验口 + 建会话时的那次展开
3699b9 3699b9/public/*          ←   纯静态视图：facet 树 + 卡网格 + 表单抽屉 + 启动
```

**`Nous-extend` 一个字节都不改。** 预设**不是扩展**：不建 `preset:<名>` 名牌、不进提示词、
不需要 registry 认它。所以它不住扩展仓 —— 那套机制（`createPresets` + `CORE_EXTENSIONS` +
热重载代理）换不来任何东西，只会多改一个仓。先例是 `qrd3wh/prompt-log.js`：纯逻辑，直接 import。

**非目标**（别顺手加）：变体 / worktree / 本机绑定 / 默认预设 / 版本历史 / 从别人的仓导入 /
参数化（`{参数}` 那种占位符）/ 预设之间的继承 / 把 preset 做成树节点 / **复制**。理由见 §8。

---

## 1. 三个词的边界（这段必须先读，不然一定会写混）

| 词 | 是什么 | 存在哪 | 谁读它 |
|---|---|---|---|
| **模板 template** | 一段**系统提示词正文**，整张贴进每轮 prompt | `Nous-prompts/<名>/PROMPT.md` | 模型 |
| **技能 skill** | 一个**目录**，` | `Nous-skills/<名>/SKILL.md` | 模型 |
| **预设 preset** | 一组**开会话的选择**，可以引用模板 | `Nous-Template/Session-Template/<名>/preset.json` | 人 + 建会话那一下 |

三者关系：preset **引用** template / skill / model，永不复制正文。

**名字纪律**：面板、代码、API、仓库一律用 `preset`；`template` 这个词在本仓里只保留它原有的
意思（提示词）。树节点名 `New-Template` 不改 —— `id` 才是引用键，`name` 是人看的那层皮。

---

## 2. 数据形状：`preset.json`

### 2.1 一个完整例子

```json
{
  "v": 1,
  "name": "weekly-report",
  "summary": "把一周的提交、聊天、会议整理成能直接发出去的周报",
  "category": ["写作", "写作/周报"],
  "description": "周五收工用。模板自带口径，别在会话里再解释一遍。",
  "model": { "provider": "volc-ark", "modelId": "deepseek-v4-flash-260425" },
  "thinkingLevel": "medium",
  "skills": ["weekly-report", "code-review"],
  "extensions": ["nous", "herdr-agent-state"],
  "template": "demo-weekly",
  "readOnly": false
}
```

同目录放一份 `PRESET.md`（可省）：给人看的说明书，**永远不进提示词**，面板详情里渲出来。

### 2.2 每个字段的三态（这是本 spec 最容易写错的地方）

引擎 `POST /v1/sessions` 的 `template` 已经有三种语义（选它 / 显式不要 / 不传用默认），preset 必须
把这套不对称**原样搬到 JSON 里**，并且**只有 template 有这个不对称**：

| 键 | 缺席 | `null` | 字符串 / 数组 |
|---|---|---|---|
| `template` | 用 `Nous-prompts/settings.json` 的 `default` | **显式不要模板** | 用这一条 |
| `model` | 用设备默认模型（`GET /v1/device` 的 `model`） | 同缺席（**规范化掉**） | 用这一台 |
| `thinkingLevel` | 设备/引擎缺省 `off` | 同缺席 | 用这一档 |
| `skills` / `extensions` | **不发这个字段** = 服务端按默认全开 | 同缺席（规范化成缺席，见 §2.3 第 7 条） | 就发这一份，`[]` = 一个都不要 |
| `readOnly` | 可写 | 同 `false` | 只读 |

三件必须写进代码注释的事：

1. **`template: null` 和没有 `template` 键是两件事**，绝不能都归成「不传」——否则「我点了不要模板，
   它又给我塞回来」这个 bug 会换个地方重演一遍（这个坑引擎注释里已经记过一次了）。
2. `model: null` 和 `model` 缺席**是同一件事**，校验层规范化成缺席。别为它保留第三种状态，
   没有「显式不要模型」这种东西。
3. `skills: null` **不是**「一个都不要」，是「按默认」。想要一个都不要就写 `[]`。

面板的新建/编辑表单里，这三处必须画成**三态单选**（不是勾选框、不是下拉 + 一个假「无」）：
模板：`仓库默认 / 无 / 某一条`；技能：`默认全开 / 都不要 / 勾这几个`；扩展同理。

### 2.3 校验规则（判定只在引擎一处，见 §4；这里写的是规则本身）

1. 顶层键白名单：`v name summary category description model thinkingLevel skills extensions template readOnly`。
   出现别的键 → 400，点名是哪个键。**未知键必须报错不能忽略**：`tempalte` 拼错静默通过，
   等于这条预设带着「本来想要的模板」跑成「仓库默认」。
2. `v` 必须是 `1`。
3. `name` 可省；给了就必须等于目录名。目录名就是 API：会话里存 `preset:<名>`，改名 = 破坏性变更，
   **没有改名按钮**（照 `vbe4pt` 的「改扩展名故意不做」，同一句理由）。
4. 名字字符集 `^[a-z0-9][a-z0-9._-]*$`，≤64 字（与 `PROMPT_NAME_RE` / `EXTENSION_NAME_RE` 同档，
   别发明第四种）。
5. `summary` 必填、非空、≤200 字。它是卡片和启动确认框上那一行 —— 正文一屏，摘要一行，
   没有摘要等于让人闭眼开对话。`description` 可省、≤2000 字，只在详情里出。
6. `category`：字符串数组，走 `repo.normalizeCategories`（`/` 分段、每段 ≤40 字、去空去重排序）。
7. `model`：缺席或 `{provider, modelId}` 两个非空字符串。不接受 `provider:"auto"` 这类**魔法串** ——
   「跟随设备默认」用缺席表达，别在数据里造第二种语言。
8. `thinkingLevel` ∈ `LEVELS = ["off","minimal","low","medium","high","xhigh","max"]`（`qrd3wh` 里已有这份顺序，
   从它导入，不复制字面量）。若同时给了 `model`，档位必须落在那台模型的 `levelsOf(model)` 里 → 否则 400。
   只给档位没给模型（模型跟随设备默认）→ **warning**：「这条的档位会按设备默认模型重新夹」。
9. `skills` / `extensions`：非空字符串数组，逐个必须在运行时可用 —— 技能看 `skills.has(name)`，
   扩展看 `registry.snapshot().installed()`。**变体的键（`name@id`）照 hb7k3j 发的那套写**，别在这层自己拼。
   `extensions` 里出现 `prompt:<名>` 或 `skill:<名>` 前缀 → 400（名牌不是扩展，有各自的字段）。
10. `template`：见上表；给了字符串必须在 `prompts.has()` 里。
11. `readOnly`：布尔。

### 2.4 为什么是 JSON 而不是 md + frontmatter

技能要 frontmatter，因为 `description` 得进提示词、正文得能被模型单独 `read`。
预设**两者都不做**：它是给建会话那一下读的纯配置，`JSON.parse` 一个解析器，没有第二种事实源。
`Nous-prompts` 那边同理也没有 frontmatter —— 抄它，别抄技能。

### 2.5 为什么不存进 `tree.json` 的节点

`Node` 只有四个字段（`id/name/value/children`），装不下一个引用清单；布局、顺序、类别树都得另写一遍，
那就成了树里的第二份真相。与技能/模板同一结论：**带内容的东西住仓库，住树里的是「有一件事」这个事实。**
盘面上 `3699b9` 那个格子就是它在树里的全部存在。

---

## 3. 仓：`/opt/Nous-Template` —— 专属私仓（按段，这一段在 `Session-Template/`）

```
/opt/Nous-Template/
├── index.json         生成物（`node index.mjs` 扫盘写出），别手改
├── index.mjs          扫盘 + 校验 + 写索引（同步、纯、不读环境变量）
├── categories.json    类别词表 {v:1, categories:[]} —— 存在的唯一理由：刚建、还没挂预设的类别不能看不见
├── README.md          人看的索引
├── AGENTS.md          这个仓自己那份（agent 进门读它）
├── test/              index.mjs 的纯逻辑测试
└── <名>/
    ├── preset.json    上面 §2 那一份
    └── PRESET.md      可选。人看的说明书，永远不进提示词
```

**没有 `settings.json`。** 预设没有默认值（§8.3），别为「以后可能想要」留一个空文件。

- **专属 = 这个仓的 remote 是用户自己的。** `origin` 由用户自己配（`git remote set-url`），
  面板只播报 `repo.remote`（脱敏：只显 `host/user/repo`，token 在 URL 里就不显），
  **面板和引擎都不写 remote、不删 remote、不碰凭据**。这是宿主状态之外的宿主状态，越界一次就收不住。
- 盘上有目录 = 这条存在；HEAD 里有目录 = 已发布。**没有「安装」这个概念。**
- 数据不落 `data/`：preset 的真相只有仓。所以 **`3699b9` 这一整块没有 `data/` 目录**，一行磁盘状态都不写。
- `index.json` 是 `categories.json` 的生成物：只提交词表不提交索引 = 当场自相矛盾，
  `regenIndex` 已经防了这个（照 `Nous-prompts` 的 `index.mjs` 抄，把 `PROMPT.md` 换成 `preset.json`）。

---

## 4. 引擎侧改动（只在块 `qrd3wh` 里）

改动面两个文件：**新建 `qrd3wh/presets.js`**（纯逻辑）+ **新建 `qrd3wh/presets-api.js`**（HTTP 面），
`server.js` 只接线。`Nous-extend` / `Nous-prompts` 一行不改。

### 4.1 `qrd3wh/presets.js` —— 名册 + 引用校验 + 展开（纯模块）

跟 `prompt-log.js` 同一个位置级别：纯函数 + 一个扫盘，没有 `createExtension`、没有 section、
没有 registry 副作用。所以它能裸跑 `node --test`。

```js
// 纯函数（导出给测试、给 presets-api、给面板的校验端点）
shapeProblems(raw, {dirName})   → string[]        // 形状（与仓的 index.mjs 同一套，见下）
referenceProblems(spec, ctx)    → { errors, warnings }
expandPresetSpec(spec)          → { model?, thinkingLevel?, skills?, extensions?, template?, readOnly? }
                                   // 严格「键在不在」：absent 不出现，null 原样是 null
resolveSpec(spec, ctx)          → { model, thinking, skills[], extensions[], template, readOnly, absence, errors }

// 名册
createPresets({ dir })          → { load(), list(), has(), get() }
presetContext({ skills, prompts, registry, models, levelsOf, defaultModel })  → ctx
```

`ctx` 是一串**问句函数**（`skillsHas` / `extensionsHas` / `promptsHas` / `catalogHas` / `levelsOf` /
`deviceModel`），不是对象 —— 这样纯函数不必知道 registry 长什么样，测试递假函数就行。
**宿主侧判定只有 `presetContext` 一处**：面板、建会话、校验端点三处都走它，所以「面板说有效」
和「启动能跑」是同一个判定。

形状规则**在两处存在**（仓的 `index.mjs` + 这里的 `shapeProblems`），有意的：仓要能单独
checkout 单独跑，不 import 引擎。跟 `Nous-prompts/index.mjs` ↔ `prompts-api.js` 那一对同一个形状。
两边各持一份同样的测试表，漂了会红。

`template:"auto"` 的兼容（p3 才做，先留好位置）：`expandPresetSpec` 把它规范化成**键缺席**。

### 4.2 `qrd3wh/server.js` 接线（三处，逻辑零处）

```
const PRESETS_ROOT = process.env.NOUS_PRESETS ?? "/opt/Nous-Template";
import { createPresets, presetContext } from "./presets.js";
import { handlePresets } from "./presets-api.js";

const presets = await createPresets({ dir: PRESETS_ROOT });       // 扫一次；写端点自己会 load()
// 校验/解析用的是**活的** registry + skills + prompts + models，所以 ctx 现造：
const presetCtx = () => presetContext({ skills, prompts, registry, models, levelsOf, defaultModel });
```

- 路由：`if (parts[0] === "v1" && parts[1] === "presets") return handlePresets({ req, res, url, parts, root: PRESETS_ROOT, presets, repo, ctx: presetCtx });`
- **没有 `CORE_EXTENSIONS`、没有 `NAMEPLATE_PREFIXES`、没有热重载代理。** 它不是扩展，
  不装在 registry 里，所以那三样都不该有它。`presets.load()` 在写端点里按需调。
- **`/v1/prompts*` 一个字不改。** 抄 `prompts-api.js` 的骨架（错误信封、串行锁、`regenOrRollback`、
  `unitOfPath` 的 `unitKind`），新建 `presets-api.js`。
- 会话树 `tree()` 里顺手带上 `origin`（p2，见 §4.5）。

### 4.3 `qrd3wh/presets-api.js` 的接口表

```
GET  /v1/presets
     → { repo{head,dirty,ahead,behind,remote,error},
         presets[]{name,summary,category[],description?,spec,specError?,tracked,
                   load{state,error?},files[],mtime},
         archived[]{name,summary,category[]},
         categories[] }
     不给 counts / facet 计数 —— 那是客户端 model.js 算的，服务端算第二套一定漂（6ysg01 同规则）。

GET  /v1/presets/categories                    → { categories }
POST /v1/presets/categories  {op,name,to?}     → { categories, touched }   add / rename(整枝) / remove(整枝)
POST /v1/presets/category    {name,category[]} → { name, category }        写这条的 preset.json + 并进词表
GET  /v1/presets/files?name=                   → { name, files[{path,size}] }
GET  /v1/presets/file?name=&path=              → { name,path,text,etag }
PUT  /v1/presets/file   {name,path,text,base}  → { etag }；base 对不上 → 409
POST /v1/presets/create {name,summary?,category?,description?,spec?,body?} → { name }   只落工作区，不提交
POST /v1/presets/validate {name} | {spec}      → validatePresetSpec 的原样输出
GET  /v1/presets/resolve?name=                 → { name, spec, resolved }
POST /v1/presets/resolve {name} | {spec}       → { resolved }              编辑框 debounce 用
POST /v1/presets/publish  {message?}           → 闸 + 重扫 index.json + commit + push
POST /v1/presets/archive|restore|purge {name}  → 同 prompts（移进 .archive/，不真删）
GET  /v1/presets/pending (+ /diff?path= + /commit?hash=)
POST /v1/presets/sync  {message,paths?}
POST /v1/presets/reload
```

`resolved` 的形状（面板和引擎共用，前端不许自己算）：

```json
{
  "model":      { "state": "auto", "ref": null, "value": "volc-ark/deepseek-v4-flash-260425", "thinkingLevels": ["off","low","medium","high"] },
  "thinking":   { "state": "ok", "ref": "medium", "value": "medium" },
  "skills":     [ { "state": "ok", "ref": "code-review" } ],
  "extensions": [ { "state": "invalid", "ref": "herdr-agent-state" } ],
  "template":   { "state": "ok", "ref": "demo-weekly", "value": "demo-weekly" },
  "readOnly":   false,
  "name":       "weekly-report"
}
```

**状态只有三种，别自己加第四种：**

| state | 条件 | chip 色 | 挡不挡启动 |
|---|---|---|---|
| `ok` | 引用有效 | 正常 | 不挡 |
| `auto` | 只有 model 会有：键缺席，跟随设备默认（默认变了 chip 文案跟着变） | 灰 | 不挡 |
| `invalid` | 引用了不存在的东西：技能/扩展没装、模板不在、模型不在编目、档位不在那台模型的档位表 | 红 | **挡**，确认框点名 |

`template:null` / `skills:[]` 是**有效值**，state 记 `ok`，UI 显成「无」「都不要」，不是灰不是红。

### 4.4 `POST /v1/sessions` 加一个 `preset` 字段

请求语义与 `template` 一字对齐（三态，别合并）：

| 请求 | 含义 |
|---|---|
| `preset: "<名>"` | 按这条预设展开后建会话 |
| 不传 `preset` | 手动创建（hb7k3j 现在这条路径，行为完全不变） |
| `preset: null` | 同不传（规范化掉，别留第三种状态） |

处理顺序（**建会话的同一个 commit 里落定**，不许先建再 configure —— 那条纪律的原话在 `server.js` 注释里）：

```
1. spec = presets.get(name)；没有这个 name → 400 { error:"没有这条预设：<名>" }
2. resolved = presets.resolve(spec)   // 引用不存在的：model 报 invalid、skills/extensions 从 wanted 里剔掉并进 missing
3. body 里显式给了的字段**覆盖**预设展开值（preset 打底，请求说了算）——
   这一条是给「带着初值的手动创建」留的口子（§7 p2/p3 依赖它），不做的话那次改动要写第二套合并逻辑
4. 其余沿用现有代码路径：model 默认、fitLevel、skills.has、template 三态、extensions 过滤
5. 201 的响应里加 `preset: <名|null>`，并且 `missing` / `missingTemplates` 继续照实回
```

**不许静默降级**：预设里技能没装了，建出来的会话少了那个技能，而面板上什么都没写 ——
这是这套东西最坏的一种失败。回 `missing[]` / `missingExtensions[]` / `missingTemplates[]`，前端必须显。

**`readOnly` 是唯一一个「建完再 configure」的例外**，理由写清：`createConversation` 的 agent 里
没有这个字段（它只认 `model` / `thinkingLevel` / `extensions`），只读是 `modelTools.remove` 的一次
configure。「同一个 commit」那条纪律要防的是「第一条消息已经跑出去了配置还没落」，而这里调用方
拿到 201 之前根本发不出消息 —— 所以建完立刻 configure 是安全的。**201 的响应里要回生效的
`readOnly`**（`!agent.tools.some(t => WRITE_NAMES.includes(t.name))`），别让前端猜。

### 4.5 会话里的出处

preset 不是扩展、不是提示词，它是**一次建会话的来源**。存法：`Sessions` 那张 per-session 的 CRDT doc
（`server.js:1181`，现在里面只有 `names`）加一个平行的表，`names` 怎么写它就怎么写：

```js
const Sessions = defineDoc({
  kind: "qrd3wh.sessions",
  version: 2,                       // 1 → 2：多了一个 origin 表
  scope: "session",
  initial: () => ({ names: {}, origin: {} }),
  checkpointWhen: () => true,
});

// 写：建会话那个 commit 里顺手带上（不要第二次 commit）
const setOrigin = (conversationId, preset) =>
  harness.commit(async (tx) => {
    const doc = await tx.doc(Sessions);
    (doc.origin ??= {})[key(conversationId)] = preset;   // ??= 兜老 session 没有这张表
  }, ctx);

// 读：tree() 里每个节点加 `origin: origins[id] ?? null`
```

两条注意：

- **`version` 从 1 到 2 要确认 chord 的读法**：老 session 的 doc 里没有 `origin` 键，所以所有读处必须
  `doc.origin ?? {}`，写处必须 `(doc.origin ??= {})`。别假设 initial 会在旧记录上重跑。
- **不进 `AgentChange`，不进提示词** —— 模型不需要知道自己是哪个预设开的，那是人要看的信息。
  fork 出来的会话 `origin` 记 `null`（fork 不是「用预设开会话」这个动作）。

---

## 5. 页面 `3699b9`（本体）

### 5.1 架构：纯静态，跟 6ysg01 / vbe4pt 一模一样

| 谁 | 干什么 |
|---|---|
| 本块 | 发 `public/` + 在浏览器里画 |
| `qrd3wh` | 唯一写入口 + 唯一校验口（`/v1/presets*`、`/v1/sessions`） |
| `Nous-Template` 仓 | 唯一真相 |

- 地址**只**从宿主 `__nous/ctx` 找 `qrd3wh`；这一块里一个 `/p/…` 都不拼。
- `server.js` 直接拷 `6ysg01/server.js`：剥 `NOUS_SELF`、目录穿越 403、目录→`index.html`、`no-store`。
  **默认端口 `4786`**（4780/4781/4782/4784/4785 已占），且只当兜底 —— 真端口是宿主给的 `PORT`。
- 没有 `data/`、没有配置、没有 token、没有后端状态。
- 静态之外一律 404。

### 5.2 信息架构

```
┌ top ──────────────────────────────────────────────────────────────┐
│ new1/3699b9 · 6 条预设     [HEAD a12f34c]  [已同步|待同步 N|失败]  [↻] │
├ rail ──┬ filters ─────────────────────────────────────────────────┤
│ 类别    │ 全部 / 未分类 / 未发布 / 装载失败 / 已归档      6 条  ＋新建预设 [重扫] │
│ ▸写作   ├──────────────────────────────────────────────────────────┤
│  ▸周报  │  卡网格                                    ← 主区，一屏到底   │
│ 代码    │  ┌───────────────┐ ┌───────────────┐ ┌───────────────┐    │
│ 未分类  │  │ W weekly-report│ │ O one-shot    │ │ D deep-dive   │    │
│ +新建   │  │ 把一周…发出去  │ │ 只回一句话…   │ │ 一个技术问题… │    │
└────────┴──┼ 写作/周报 ──────┤ │ Basics        │ │ 工程          │    │
  左栏 facet│ 模型 自动·volc… │ │ 模型 无（灰）  │ │ …             │    │
            │ 技能 2          │ │ 模板 无        │ │               │    │
            │ 模板 周报助手   │ │ [▶ 启动]      │ │               │    │
            │ [▶ 启动]   [⋯] │ └───────────────┘ └───────────────┘    │
            └──────────────────────────────────────────────────────────┘
```

版式照 `vbe4pt`（左栏类别 facet + 主区卡网格），骨架代码抄 `6ysg01`（它本身就是从 `68kazk`
抄的骨头 —— 三页同一份骨头，第四页必须还是它）。

### 5.3 逐栏规格

**顶栏**：`<board>/<id> · N 条`；右边 HEAD 短哈希 + 同步 chip（`已同步` / `待同步 N` / `推送中` /
`失败` / `远端多 N 个` / `本地已删 N 条`）+ 重扫按钮。`repo.error` 有值时这一整行变红并显原文 ——
git 失败要看得见，不是白屏。专属私仓的地址显在左栏底部一行灰字（`仓在 origin · github.com/JoeWest9966/Nous-Template`），
点开可复制；**不给编辑入口**（§3）。

**左栏类别**：缩进类别树（`/` 分段），每行带计数，底部「＋ 新建类别」。点一下筛选，父级筛出整枝
（结果区写「写作 · 含 N 个子级」）。每行 `⋯` 菜单三项：新建子类别 / 编辑名称（原地变输入框，
**填全路径**，改前缀 = 搬家）/ 删除整枝（确认框写影响几条预设）。内置类别 `Basics` 受
`PROTECTED_CATEGORIES` 保护，只能新建子类别。长按移动 >10px 当滚动取消；松手补的那个 `click` 必须吞掉。
输入框必须等整串 facet 挂进文档之后再 `focus()`（6ysg01 踩过的坑，原样写进注释）。

**筛选栏**：全部 / 未分类 / 未发布 / 装载失败 / 已归档。**非零才出现，图例就是筛选**，不另写说明。
色点只是辅助，文字才是本体。「已归档」只在 `.archive/` 非空时出现，点开就是回收站：一行一条 + `恢复` / `彻底删除`。

**卡片**（一张卡 = 一条预设）：
- 字母砖，色相由名字 hash（与 6ysg01/vbe4pt 同一函数）。
- 名字 + `summary` 一行（截断，`title` 给全）。
- 类别 chip。
- **引用摘要 chip 行**（这是这一页区别于其它三页的唯一新东西）：

```
模型 [auto|volc-ark/deepseek-v4-flash]   思考 [medium]   技能 [N 条]   扩展 [N 条]   模板 [周报助手]   只读 [●]
```

  - `model.state === "auto"` → 灰 chip，文案 `自动 · volc-ark/deepseek-v4-flash`（值来自 `resolved`，别自己猜）。
  - `template` 是 `null` → 正常 chip 写 `无`（不是灰、不是虚线：显式不要 = 有效决定）。
  - `skills`/`extensions` 数组空 → 写 `都不要`；键缺席 → 写 `默认`（灰）。
  - 任何 `invalid` → 那张 chip 变红并显 `ref` 原文。
- 右上角常驻 `⋯`（菜单入口，卡上不放按钮）。
- **整卡状态优先级：`failed > invalid > unpublished > ok`**（`model.js` 的 `cardState()`，与 6ysg01 同函数形状）。

| 状态 | 边的样子 | 角标 | 意思 |
|---|---|---|---|
| `ok` | 实边 | — | 能启动 |
| `unpublished` | 琥珀虚线边 | 琥珀「未发布」 | 磁盘上有、HEAD 里没有，别人 pull 不到 |
| `invalid` | 实红边 + 卡上一行点名哪条引用坏了 | 红「引用失效」 | **启动不给点** |
| `failed` | 实红边 + `load.error` 一行 | 红「装载失败」 | JSON 坏了 / 校验没过 / 未知键 |

`invalid` 和 `failed` 的区别要说清并写在卡上：前者是「引用的东西没了」（仓里这条没坏，去修那边），
后者是「这条自己写坏了」（去修这里）。混成一格，人就分不清该动哪个文件。

**详情**（点卡开半屏浮层，不换页）：
`summary` / `description`（Markdown 渲染自 `PRESET.md`，若无则不显这一节，**不摆「暂无说明」那一行**）/
类别（唯一可改的地方，就地改 chip）/ **引用一览**（每个引用一个链接，跳去对应块的页面 —— 模板去 `6ysg01`、
技能去 `68kazk`、扩展去 `vbe4pt`、模型去 `k5411g`；地址一律从 ctx 拿，拿不到就只是文本，**不许拼路径**）/
仓库（发没发布、`.archive` 在不在、绝对目录、文件列表）/ 底部 `编辑 · 归档 · 启动`。
`failed` 时最上面一条红字 + 「重试装载」（只重扫，**不替你修文件**）。

**编辑抽屉**：一个 JSON 编辑槽 + 一个结构化表单，两个 tab 编的是**同一份 `preset.json`**：

| tab | 有什么 |
|---|---|
| 表单 | 名字（只读）/ 摘要 / 类别 chip / 描述 / 模型（单选，来自 `GET /v1/models`，含 `provider/` 前缀 + 一个「跟随设备默认」选项）/ 思考档（档位表**跟着所选模型**，模型为「跟随」时全档并写一句「按设备默认模型夹」）/ 技能（checkbox）/ 扩展（checkbox，按 family 折叠，**不列变体**）/ 模板（radio 三态 + 一个「跟随仓库默认」）/ 只读（switch） |
| JSON | `preset.json` 原文编辑器 + 与 HEAD 的行级/词级差异视图（直接拷 6ysg01 的 `diff.js` + `vendor/diff`，jsdiff 8 MIT，逐文件拷 —— 这页没有打包器） |

  - 表单和 JSON **单向同步**：切 tab 时表单 → JSON 立刻生成，JSON → 表单只在 `validatePresetSpec` 通过时回填；
    不过就停在 JSON 并在底部显服务端原文。别做双向实时同步（一定会漂，且一定在非法 JSON 上炸）。
  - 校验在引擎：`POST /v1/presets/resolve {spec}`，600ms debounce，结果显示成 chip 上的红/灰（同一套 `resolved`，
    所以「面板说有效」和「启动能跑」是同一个判定）。**客户端不写第二套规则**（6ysg01 的「三条不许踩」第 1 条原样适用）。
  - **保存和发布是两个按钮，不许合并**：保存 = `PUT /v1/presets/file`（带 `base`/`etag`，撞了 409）；
    发布 = `POST /v1/presets/publish`（闸 + 重扫 + commit + push）。发布成功后**必须重读这个文件**（基线跟着 HEAD 走了）。
  - 409 那一句固定：`文件已被改，重新加载`。`ConflictError` 单独一个类，别靠 `status===409` 散落各处判断。
  - 抽屉**不建文件、不删文件**：只读写已经存在的文件。（新建走新建框，删除走卡菜单，见下。）

**「＋ 新建预设」**：在结果计数旁边。只问四样：名字 / 摘要 / 类别（可空 = 未分类）/ 起点。
名字是 API、建完改不了 —— **这句必须挂在输入框下面**。起点三个选项：
`空白`（`template` 键缺席、其余全缺席）/ `从当前设备默认起手`（`GET /v1/device` 的 model 写死进去）/
`从 hb7k3j 带过来`（p3，见 §7；本期这个选项 disabled + title 写明「等 p3」）。
落盘 = `POST /v1/presets/create`，只进工作区。**新预设的第一个正确状态是「未发布」**。

**卡 / 行的 `⋯` 菜单**（桌面右键、手机长按 520ms、键盘 `Shift+F10`；点卡上 `⋯` 同一条菜单）：

| 条目 | 服务端 | 确认 |
|---|---|---|
| 启动 | 见 §5.4 | 名字冲突时手输；引用失效时**禁用** |
| 编辑 | 开抽屉 | — |
| 发布 | `POST /v1/presets/publish` | 有未提交改动时列出范围 |
| 归档 | `POST /v1/presets/archive`（移 `.archive/`，不真删，可恢复） | 手输名字（未发布时） |
| 恢复 | `POST /v1/presets/restore` | — |
| 彻底删除 | `POST /v1/presets/purge` | 手输名字；允许「目录不在但有暂存删除」那条收尾路径（6ysg01 第 5 条坑原样适用） |

「删除」和「归档」是两件事：未发布的（HEAD 里没有）菜单上写 `删掉`，已发布的写 `归档` —— 与 `vbe4pt` 同一句
`label: unpublished ? "删掉" : "归档并推送"`。别做一个真删除按钮，仓里的东西要能反悔。

### 5.4 启动流程（就是那个「启动按钮」）

点卡片上的 `[▶ 启动]`：

```
1. 确认框（半屏，从卡片长出来 —— transform-origin 对准触发点，苹果那套）：
     名字输入框        预填「<预设名> 11-06 14:32」，可改（服务端 slice(0,120)）
     生效配置一览      模型 / 档位 / 技能 N / 扩展 N / 模板 / 只读 —— 全部来自 /v1/presets/resolve
     红字（有才出）     「这些没装，会跳过：技能 X、扩展 Y」
     灰字（有才出）     「模板跟随仓库默认」「档位会按设备默认模型夹」
2. POST <qrd3wh>/v1/sessions { preset: <名>, name? }          ← 一个 commit 落定，不发第二次的 configure
3. 认服务端回来的**生效值**（model/thinkingLevel/skills/template/missing[]/missingTemplates[]），
   不认我们发上去的；有 missing 就在 toast 里点名，红字。
4. POST <qrd3wh>/v1/selection { id: created.id }               ← 盘上那只眼睛跟着切过去
5. window.open(<qrd3wh 的 url>, "_blank", "noopener")          ← 新标签页，不在这一页内嵌聊天，
                                                                  也不在本页跳转（会丢掉树的展开/滚动/撤销栈）
6. toast：「已开「<名>」· 模板 <生效模板 ?? '无'> · 预设 <名>」
```

不在这页内嵌聊天 —— 与 6ysg01「用这条开会话」同一条理由：壳管不了块的页面，两块没法对着看。

### 5.5 一屏无滚动 + 键盘 + 手机

- `html/body overflow:hidden`，主区自己滚（只有最外层滚，嵌套滚动在触屏上必翻车 —— 这条是宿主的规矩）。
- **焦点是列不是项**：`↑↓` 在网格内移动（到头就停，不绕圈）、`←→` 换行/换列、`↵` = 点它（走同一个
  `actOn`，不写第二套判断）、`N` 新建、`/` 进搜索框、`Esc` 一层层退（抽屉 → 确认框 → 菜单 → 筛选）。
  焦点存 `kb`（`{row,col}`），不靠 `document.activeElement` —— `render()` 每次重建整块 DOM，
  真焦点会跟着旧节点一起消失（hb7k3j 注释里那句「键盘失忆」在这页同样成立）。重画后 `paintKb()` 把环钉回同一格
  + `scrollIntoView({block:"nearest"})`。键盘环比 hover 重一档。
- 手机：左栏收成抽屉（`vbe4pt` 那套）、卡片单列、长按出菜单、底部 `安全区` padding。参照 `mobile-native` 的清单。
- `reduced-motion`：弹窗直接显隐，不做缩放。

### 5.6 空态与错误态（写死，不许发明第四种说法）

| 情况 | 显什么 |
|---|---|
| 仓一条都没有 | 「私仓里还没有预设」+ **真能走的链接**：`去手动建一个（hb7k3j）`、`去写模板（6ysg01）`，地址从 ctx 拿 |
| qrd3wh 不在 ctx 里 | 一行红字：`没找到会话引擎（qrd3wh）` —— 说清是谁不在，「加载失败」四个字修不了任何东西 |
| 编目挂了 / 模型列表空 | 抽屉里模型 tab 显 `catalog.error` 原文 + 「去模型块（k5411g）」；**不空列表**（空列表 = 这条预设不能建） |
| 引用的技能/模板被删了 | 卡片红边 + `invalid` 那行点名 + 详情里那条链接（跳去修那边） |
| JSON 坏了 | `failed` 卡 + `load.error` 一行 + 「重试装载」 |
| git 失败 | 顶栏红 + 原文 |

---

## 6. 数据流（一页画完）

```
      ┌──────────────────────── 3699b9（浏览器，纯静态）────────────────────────┐
      │ app.js ── 渲染/菜单/手势/抽屉/确认框                                     │
      │   ├─ model.js  纯计算：cardState / facet 计数 / 排序 / 引用 chip 文案    │
      │   └─ api.js    ctx 解析、错误信封、409、类别 debounce、发布后重读        │
      └──────┬────────────────────────────────────────────────────────────────-┘
             │ 只走 HTTP，地址只从 __nous/ctx 来
             ▼
   qrd3wh  GET /v1/presets ──── 读 /opt/Nous-Template（repo + 扫盘 + load 状态）
           POST /v1/presets/* ─ 串行锁 → 写盘 → regenIndex（不过就整体回滚）→ load()
           POST /v1/presets/publish ─ 闸 + commit + push
           POST /v1/presets/resolve ─ qrd3wh/presets.js 的同一份校验+展开
           POST /v1/sessions {preset} ─ 展开 → 同一个 commit 建会话
             ▲                                    │
             └── /opt/Nous-Template（专属私仓，git）┘  新标签页打开 qrd3wh 会话
```

三条不许踩（前两条是从 6ysg01 抄来的，不是我发明的）：

1. **不复刻服务端的闸。** 名字字符集、撞目录、撞 `.archive/`、保留名、`summary` 非空、引用存在性 ——
   全部 qrd3wh 一处判定。这一页只播报服务端那一行原文，不重写、不加前缀。
2. **改完要重扫。** 写端点自己会 `presets.load()`；命令行改了 `preset.json` 之后要么点「重扫」，要么走抽屉保存。
   不 load 的话面板显示新预设、会话里还看不见它 —— 两份真相就是这么长出来的。
3. **面板上写的 = 实际跑的。** chip 上的模型/档位/模板一律来自 `resolve`（服务端算的），
   不来自我们发上去的那份 spec。这条破了，这一页存在的意义就没了。

---

## 7. hb7k3j 的关系与分期

| 期 | 内容 | 依赖 | 独立可验收 |
|---|---|---|---|
| **p0** | 仓 `Nous-Template`（`Session-Template` 段）+ `qrd3wh/presets.js` + `qrd3wh/presets-api.js` + `POST /v1/sessions {preset}` | — | `curl` 全流程跑得通，有测试 |
| **p1** | 页面 `3699b9`：列表 / 类别 / 详情 / 编辑 / 新建 / 归档·恢复·彻底删除 / **启动** | p0 | 手工验收清单 §9 |
| **p2** | 会话出处：`Sessions` 那份 doc 加 `origin` 表存 `preset`（§4.5），`GET /v1/sessions` 回它，`qrd3wh/index.html` + `pxbgng` 会话树上显小标签 | p0 | 单测 + 手看 |
| **p3** | `hb7k3j` 的上下文域开「预设」picker：选中 → 调 `resolve` → **预填**四域的勾选（不替用户决定），Create 时带 `{preset, …被改过的字段}`。同时把 preset 里的 `template:"auto"` 换成真正的键缺席语义 | p0/p1 | hb7k3j 的验收单独写 |

**p3 是唯一让「preset」和「manual」可能混淆的一步**，规则写死：**preset 是初值，不是裁决。**
用户在 hb7k3j 上改过的字段以请求为准（§4.4 第 3 步那条覆盖规则就是为它准备的），改了之后卡片上不动原预设
—— 存成新预设是显式动作（手工建一条，或以后那个「另存为」）。

p1 期间**不碰 hb7k3j 一行代码**，也不碰 `/v1/prompts`、不碰 `Nous-extend`。这条是并行纪律：
p0/p1 的改动面只有 `qrd3wh/presets.js`（新）、`qrd3wh/presets-api.js`（新）、`qrd3wh/server.js`（三处接线）、
`content/new1/map/3699b9/**`（新），加上上面那个新仓。

---

## 8. 不做的事，和为什么

1. **没有变体。** 用户点名不要。技术上还有一层：变体的价值是「同一条东西的另一个可装载名」
   （worktree + 分支 + 本机绑定），预设只有一条生命轴 —— 仓里的版本。要做「同一条预设的分支版」，
   那就是**两条预设**。想清楚这条就不眼馋 `vbe4pt` 那套了。
1.5 **没有复制。** 也点名不要。`cp -r <名> <新名>` 加改一行 `name` 就是全部，为它开一个按钮 + 一个端点 +
   一套撞名确认，换来的只是省一条命令。
2. **不进提示词，不建 `preset:` 名牌。** 技能有 `skill:<名>`、模板有 `prompt:<名>` 是因为模型要在
   运行时知道它们。preset 是**开会话那一下的选择**，跑起来之后它已经全部展开成别的了。
   建个名牌 = 每轮塞一段模型用不上的家规 + 给 registry 添一个假成员。
   人想看的「这条是哪个预设开的」走 §4.5，那是元数据，不是提示词。
3. **没有默认预设。** `Nous-prompts/settings.json` 的 `default` 有真消费者（引擎在「没人说」时必须挑一条贴进系统），
   「默认预设」没有消费者：hb7k3j 的定义就是手动，3699b9 的启动是显式点击。一个默认预设会让「新建对话」
   这个动作随「谁最后点了哪条」而变，那是最坏的隐式状态。真想要「每次都这套」，用 p3 的 picker 预填。
4. **不做参数化 / `{…}` 占位符。** 「这条模板每次开头换个仓库名」是**两条预设**。参数化会要求
   启动确认框变成表单，而这套东西的全部价值是「一次点击」。
5. **不做继承 / 预设套预设。** 环、优先级、覆盖语义、报错信息全跟着来，收益只有省几次点击。
6. **不做版本历史。** git 就是历史（`/v1/presets/pending/diff` 已经给了和 HEAD 比的能力）。
7. **不改名。** 目录名 = `name` = 会话里的键。抽屉里没有改名按钮是设计。
8. **不做导入/导出、市场、搜索、协作。** 与 `skills-own-store.md` 同一份非目标清单。
9. **本块没有 `data/`。** 一行磁盘状态都不写。要写 `NOUS_DATA` 的东西就说明它不该在这一页。

---

## 9. 测试与验收

### 9.1 自动化（抄 6ysg01 的布局：只测纯逻辑，不测 DOM）

```
/opt/Nous-Template/test/            index.mjs：扫盘、形状、非法 spec 进索引标坏账、词表并进索引
qrd3wh/test/presets.test.js        shapeProblems / expandPresetSpec / resolveSpec 的三态矩阵（§2.2 那张表
                                   逐条）+ 引用失效不变成 shape 错误（那是「现在开不了」不是「写坏了」）
qrd3wh/test/presets-api.test.js    错误信封、etag/409、create→publish 的 git 顺序、归档/恢复/purge
                                   的收尾半截、串行锁、regen 失败整体回滚
qrd3wh/test/presets-sessions.test.js  POST /v1/sessions {preset}：
                                   缺席 / null / 名字 / 名字不存在 400 / 技能失效→missing /
                                   档位夹过 / body 覆盖 preset / 201 里 preset 回显
3699b9/test/                       model.js：cardState 优先级、facet 计数、chip 文案（auto/null/[]/invalid）、
                                   删除类别的影响范围；api.js：ctx 解析、409、发布后重读、debounce
```

`expandPresetSpec` 的三态测试是**这份设计的安全带**：`template` 缺席和 `template:null` 必须打出不同的请求体。
没有这条测试，那三种语义一定会在某个 refactor 里合并掉（引擎注释说它已经错过一次）。

### 9.2 p1 手工验收清单

- [ ] 冷启动 `/p/new1/3699b9/` 直接出列表（不靠 hash 路由）
- [ ] 断网 / 停掉 qrd3wh：页面给出「没找到会话引擎」那一行，不是白屏、不是空网格
- [ ] 新建 → 卡片立刻出现且是**琥珀虚线**（未发布），仓里 HEAD 没有它
- [ ] 发布 → 变实边，顶栏 chip 变「已同步」，`git -C /opt/Nous-Template log -1` 看得到那个 commit
- [ ] 编辑 JSON 写一个未知键 → 卡变 `failed` 红边 + 服务端原文；改对之后点「重扫」能活
- [ ] 引用的模板删掉（在 6ysg01 那边删）→ 这边卡变 `invalid`，启动按钮 disabled，红字点名
- [ ] 启动 → 新标签页打开 qrd3wh，会话按预设建好，确认框写的和会话里生效的**一字一样**
- [ ] `template:null` 的预设启动后，会话里确实没有模板（不是被仓库默认塞回来）
- [ ] 类别改名（改父级前缀）→ 底下所有预设跟着走，筛选不落空
- [ ] 长按滚动不弹菜单；菜单不被松手那下 `click` 误触
- [ ] 手机竖屏：一屏不横滚，卡片单列，左栏抽屉可用，启动确认框能单手点到底
- [ ] `Esc` 层级：抽屉 → 确认框 → 菜单 → 筛选，一次一层
- [ ] 最坏数据（`break-ui`）：200 字的 summary、名字撞保留名、5 个类别、0 条技能、emoji、超长中文

---

## 10. 文件清单

```
content/new1/map/3699b9/
├── AGENTS.md              这份 spec 的块内摘要（四段：它做什么 / 页面规格 / 三条不许踩 / 不做的事）
├── package.json           { name:"3699b9", private, type:"module", scripts:{start:"node server.js", test:"node --test test/index.js"}, engines:{node:">=22.19.0"} }
├── server.js              纯静态：拷 6ysg01/server.js，默认 PORT=4786
└── public/
    ├── index.html         骨架：顶栏 / 左栏 / 筛选 / 网格 / 详情 / 抽屉 / 确认框 / 新建 / 回收站
    ├── style.css          拷 6ysg01/style.css 改（同一套 Flexoki 变量 + 卡 + facet + 抽屉）
    ├── model.js           纯计算，无 DOM 无 localStorage
    ├── api.js             数据层，同上
    ├── app.js             DOM 那一半
    ├── diff.js            拷过来
    └── vendor/diff/       jsdiff 8 逐文件拷（MIT，无打包器）
test/model.test.js, test/api.test.js, test/index.js(barrel)

Nous-Template/             整个仓（private，origin 已配）—— 按段放，Session-Template 段含 4 条起步预设
qrd3wh/                    presets.js（新，纯逻辑）+ presets-api.js（新，HTTP）+ server.js（三处接线）+ test/
```

`qrd3wh/server.js` 是运行时唯一真相所在，改动面必须小：**接线三处，逻辑零处**（全在 presets.js /
presets-api.js 和新仓里）。`Nous-extend` / `Nous-prompts` / `Nous-skills` 一行不改。

---

## 11. 开工前要先定的两件（其余我已经定了）

1. **`Nous-Template` 的 remote。** 面板不碰（§3）；已经建好 private 仓并推过首条，
   要么部署脚本铺一次。归**用户自己**，并且页面在他没配时把顶栏 chip 显成「没有远端」而不是报错 ——
   本地单用户也是合法用法。
2. **`extensions` 这一维要不要进第一版 UI。** 引擎支持（`body.extensions`），hb7k3j 也支持（按 family 折叠 + 变体）。
   我的判断：**存，但第一版表单里折叠在「高级」里、默认不显**。理由：扩展清单是 `registry` 的进程内状态，
   比模型/技能更容易漂，把它摊到每个卡片上会让「这套是什么」变成八行小字，而这一页的价值是一眼看完。
   数据里必须留着，UI 上先别吆喝。
