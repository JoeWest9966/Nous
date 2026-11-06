# spec · skill 自持私库（脱钩上游编目）

**这份文件是四个并行 implementer 唯一的约定来源。** 票只写「执行本 spec 的哪一节 + 自己的验收」，不复制这里的形状。
改形状先改这里，再改票。

轨道：`JoeWest9966/Nous#4`（总票）。子票：`#5` A · `#6` C · `#8` D1 · `#9` D2 · `#7` E · `JoeWest9966/Nous-extend#1` B。

---

## 0. 一句话

skill 从「上游编目的投影」变成「我们自己 git 仓库里的文件」。照 Nous 扩展（`/opt/Nous-extend` + 块 `vbe4pt`）那套：仓库是唯一真相，`qrd3wh` 是唯一写入口，面板是纯静态视图。

**非目标**：上游编排 / 技能市场 / 技能搜索 / 从 GitHub 拉别人的技能 / 技能版本号。这些都不做，别顺手加。

> 2026-11 修订：**技能的「变体」从非目标里拿掉了**。当时把它划出去的理由是「复制一份新目录就完事」
> （票 Nous-4 的 Fog 段），可那句话把两件事混成了一件：复制是**另一条技能**，变体是**同一条技能的
> 另一个可装载名**。扩展那边（`vbe4pt` / `server.js` 的 `variantKey` + worktree + 本机绑定）证明这套
> 是能做的，技能照搬了：分支 `skill/<本名>-<id>`、树开在仓的隔壁、绑定在 `data/skills.local.json`、
> 接口按 `name` + `variant` 两个参数走。细节见 `content/new1/map/qrd3wh/AGENTS.md` 的「变体」一节。

---

## 1. 谁拥有哪些文件（并行纪律）

| 票 | 独占路径 | 一律不许碰 |
|---|---|---|
| A | `/opt/Nous-skills/**`（新仓库） | 其他任何地方 |
| B | `/opt/Nous-extend/skills/**`、`/opt/Nous-extend/index.json` | `content/`、`qrd3wh` |
| C | `content/new1/map/qrd3wh/server.js`、**新建** `content/new1/map/qrd3wh/skills-api.js`、`.../test/skills*.test.js` | `68kazk`、`Nous-extend` |
| D1 | `content/new1/map/68kazk/server.js`、`.../public/**`、`.../test/**`、`.../AGENTS.md` | `qrd3wh`、`Nous-extend` |
| D2 | `content/new1/map/68kazk/public/app.js` 里的接线部分 | 同上 |
| E | 跨仓删除 + 部署（最后一步） | 无人并行 |

`qrd3wh/server.js` 是 3177 行的单文件 —— **只有 C 能改它**。想「顺手修一下隔壁的坑」= 制造合并冲突。

---

## 2. `Nous-skills` 仓库布局

```
/opt/Nous-skills/
├── AGENTS.md
├── README.md
├── categories.json          {"v":1,"categories":["Basics"]}
├── index.json               生成物，别手改
├── index.mjs                扫盘器（导出 buildIndex，CLI 薄壳）
├── .archive/<name>/         废弃的技能整目录搬这儿（**扁的**，不跟分组），git 追踪，能恢复
├── <分组>/                   可选：只装技能的抽屉（`deprecated` / `engineering`），**最多一层**
│   ├── README.md           分组自己的说明（空抽屉靠它存在）
│   └── <name>/ …           技能形状同上
└── <name>/                  一个目录 = 一条技能，目录名 == 技能名 == 会话里的 API 键
    ├── SKILL.md             frontmatter + 正文
    ├── nous.json            {"name","summary","category":[]}
    └── …                    可选：这条技能自带的参考文档 / 脚本（**可以再分子目录**，相对路径引用）
```

**分组只是抽屉，不进名字。** `deprecated/demo/` 那条技能还叫 `demo`：`name` 是会话里的键
（`skill:demo`），`index.json` 里多一个 `path: "deprecated/demo"` 记着它住哪儿。
所以全仓**只能有一个** `demo` —— 两个抽屉各放一个，模型那边只有一个跑得起来，`buildIndex` 直接报错。

**没有** `package.json`、`node_modules`、示例技能。空仓就是空仓。

### 2.1 `SKILL.md` 的 frontmatter

```markdown
---
name: code-review
description: "审查从某个点以来的改动：标准 + 规格两条轴。"
# 可选
disable-model-invocation: "true"
---

正文…
```

- `name` **必须等于目录名**。不等 = 坏账（`load.state = "failed"`，见 §4）。
- `description` **必需且非空**。模型靠它判断「这事该不该用这条技能」；空的等于不存在。
- 解析器用现成的 `parseFrontmatter()`（`Nous-extend/skills/index.js`），手写、零依赖，别引 yaml。
- 其余字段一律不认（写了不报错、也不起作用）。

### 2.2 `nous.json`

```json
{ "name": "code-review", "summary": "一行话，给面板和 AGENTS.md 用", "category": ["Basics"] }
```

- `name` 必填且 == 目录名。
- `summary` 必填（可为 `""`）。它跟 `description` 不是一回事：`description` 进提示词、给模型看；`summary` 给人看。
- `category` 必填（可为 `[]`）。元素是**路径**，见 §3。
- 没有 `version` / `file` / `entry` / `needs` —— 技能不是代码。

### 2.3 `categories.json`

```json
{ "v": 1, "categories": ["Basics", "工具", "工具/搜索"] }
```

路径集合，扁平存，**没有父子结构**。`工具/搜索` 隐含 `工具` 也在词表里（写入端负责补）。

---

## 3. 类别的规则（三端一致）

1. 类别是**路径**：`/` 分段，`工具/搜索` 是 `工具` 底下的 `搜索`。父子关系不存盘，读取时按 `/` 现拆。
2. 真相是**两份取并集**：每条 `nous.json` 的 `category` + `categories.json` 的词表。词表存在的唯一理由是「刚建、还没挂到任何技能上的类别不能建了看不见」。
3. 规范化：去首尾空白、把连续 `/` 压成一个、去掉空段、每段禁控制字符、每段 ≤ 40 字。不合法 → 400。
4. `[]` = 未分类。**不写「未分类」三个字进文件**，那是读取时算出来的。
5. 改名 / 删除类别要级联改所有 `nous.json`；删整枝（`工具` 删掉连 `工具/搜索` 一起）。
6. 没有受保护的类别。别造 `PROTECTED_CATEGORIES`。

---

## 4. 扩展侧契约（票 B）

```js
createSkills({ dir, registry, api })   // 签名只有这三个键，多一个都不许
```

`dir` = **仓库根**（`/opt/Nous-skills`），不是 `data/skills`。扩展自己不猜默认路径。

### 4.1 `list()`

```js
[{
  name: "code-review",
  description: "…",          // frontmatter.description
  dir: "/opt/Nous-skills/code-review",
  file: "/opt/Nous-skills/code-review/SKILL.md",
  hidden: false,              // frontmatter["disable-model-invocation"] === "true"
  category: ["Basics"],       // 来自 nous.json，读不到就是 []
  load: { state: "ok" }       // 或 { state: "failed", error: "一行话" }
}]
```

- 只有 `state: "ok"` 的进内存名册、能进提示词。
- `failed` 的**必须留在 `list()` 里**（面板要显示红卡），但 `render()` 跳过。
- `load()` 返回同一个形状；`has(name)` / `missing(names)` 语义不变。
- **不再有** `receipt` / `source` / `entry` / `repo` / `sha` / `version` / `tag` / `commitsSince` / `describe` / `at`。

### 4.2 扫盘规则

- 遍历 `<dir>/*`，只认目录，跳过 `.` 开头（`.archive/` 天然不进）。
- **有 `SKILL.md` = 一条技能**；没有的当**分组**（`deprecated/` 那种），往里再找一层（最多一层，`MAX_GROUP_DEPTH = 1`）。
- 分组里只有 `README.md`、一条技能都没有：合法，直接跳过（空抽屉就该空着）。
- 有 `SKILL.md` 但 frontmatter 缺 `name` / `description`、或 `name !== 最后一段目录名` → `failed` + `error`。
- 两个抽屉里有同名技能：先扫到的留着，后来的 `failed`，`error` 里点出两条路径（名字是会话键，只能有一个生效）。
- `list()` 每项多一个 `path`（`demo` / `deprecated/demo`）：面板靠它开文件，会话靠 `name` 认技能。
- 不读 `.nw.json`、不读任何缓存文件。

### 4.3 `render(names)` 一个字节都不许变

沿用现在这段：

```
The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>…</name>
    <description>…</description>
    <location>/abs/path/SKILL.md</location>
  </skill>
</available_skills>
```

- 一条都没选 → 返回 `undefined`（整段不出现），**不吐空的 `<available_skills>`**。
- `section("skills", …, { tag: false })` 保持裸着进提示词。
- `render()` 每轮请求都跑：**只查内存 Map**，不碰文件系统、网络、时间。

### 4.4 名牌

`skill:<name>` 空白扩展名，`ensureStub()` 只补不卸。`skillNamesOf(agent)` 不变。

---

## 5. `buildIndex(root)` 契约（票 A）

`index.mjs` 必须是可测的模块，不是只能 `exec` 的脚本：

```js
export function buildIndex(root)   // → { skills: [...], categories: [...] }
```

- `skills[]` 按 `name` 升序：`{ name, summary, description, category: string[] }`。
- `categories[]` = 规范化 + 排序后的路径集合。
- 校验失败**抛错**（`Error` 带一行原因），不返回部分结果。校验项：
  - 目录名 == `SKILL.md` 的 `name`；
  - `description` 非空；
  - `nous.json` 的三个字段齐全、`name` 一致；
  - 每条 `category` 的每一段都能在 `categories.json` 里找到（写错了当场红）。
- CLI 形态：`node index.mjs` → 重写 `index.json` 并以 0 退出；校验失败打印原因、退出码 1、**不写文件**。
- `index.json` 形状：

```json
{
  "v": 1,
  "_about": ["生成物 —— 别手改。改了会跟目录对不上：跑 `node index.mjs` 重扫。"],
  "skills": [ { "name": "…", "summary": "…", "description": "…", "category": ["…"] } ]
}
```

---

## 6. HTTP 契约 `/v1/skills*`（票 C）

统一错误信封：`{ "error": "一行话，≤400 字" }` + HTTP 状态。`400` 校验失败 / `409` 冲突 / `404` 路径不存在 / `500` 内部。成功一律 `200`，`ok: true` 可选。

**根**：`SKILLS_ROOT = process.env.NOUS_SKILLS ?? "/opt/Nous-skills"`。

**实现形态**：不要往 `server.js` 里再塞 600 行。新建 `content/new1/map/qrd3wh/skills-api.js`：

```js
export async function handleSkills({ req, res, url, parts, root, skills, repo })
```

`skills` = 扩展那个 service（`{ list, load, has, dir }`），`repo` = 抽参数后的 git 工具集合（见 §6.3）。`server.js` 只做 `if (parts[0] === "v1" && parts[1] === "skills") return handleSkills(...)`。这样测试能注入假的 `skills` + 临时 git 仓，不用起引擎。

### 6.1 端点

| 方法 · 路径 | 请求 | 响应 |
|---|---|---|
| `GET /v1/skills` | — | 见 §6.2 |
| `GET /v1/skills/categories` | — | `{ categories: string[] }` |
| `POST /v1/skills/categories` | `{ op: "add"｜"rename"｜"remove", name, to? }` | `{ categories, skills: [...] }`（`skills` 是改后的 `list()`） |
| `POST /v1/skills/category` | `{ name, category: string[] }` | `{ skill, skills, categories }` |
| `GET /v1/skills/files?name=` | — | `{ name, files: [{ path, size }] }`（`<name>/` 下非点开头的文件，递归，最多 200 个） |
| `GET /v1/skills/file?name=&path=` | — | `{ name, path, text, etag }`；二进制/不存在 → 404 `{error}` |
| `PUT /v1/skills/file` | `{ name, path, text, base }` | `{ etag, path, created }`；**目标不存在 = 新建**（中途的目录一起造出来），`base` 与当前不一致 → 409 |
| `DELETE /v1/skills/file?name=&path=[&recursive=1]` | — | `{ name, path, kind }`；目录非空且没 `recursive=1` → 409；文件**不真删**（git 历史就是回收站） |
| `POST /v1/skills/mkdir` | `{ name, path }` | `{ name, path, kind: "dir" }`；里面铺一份 `.gitkeep`（空目录 git 不跟踪） |
| `POST /v1/skills/create` | `{ name, description, category?, group? }` | `{ skill, skills }`；`group` = `deprecated` 那种抽屉，建的时候顺手建它 |
| `POST /v1/skills/publish` | `{ message? }` | `{ ok, commit: {hash,subject} \| null, pushed: bool, warnings: string[] }` |
| `POST /v1/skills/archive` | `{ name }` | `{ ok, commit, pushed }`；分组里的搬进扁的 `.archive/<name>`，原址记在 `nous.json.archiveOrigin` |
| `POST /v1/skills/restore` | `{ name }` | `{ ok, commit, pushed }`；按 `archiveOrigin` 搬回原来的分组 |
| `POST /v1/skills/purge` | `{ name }` | `{ ok, commit, pushed }` |
| `GET /v1/skills/pending` | — | 见 §6.3 |
| `GET /v1/skills/pending/diff?path=` | — | `{ path, before, after }`（对 HEAD；未跟踪文件 `before: null`） |
| `GET /v1/skills/pending/commit?hash=` | — | `{ hash, subject, files: [{path,status}] }` |
| `POST /v1/skills/sync` | `{ paths: string[], message }` | `{ ok, commit, pushed }` |
| `POST /v1/skills/reload` | — | `{ ok, skills: <list()>, error? }` |

**不存在** `install` / `uninstall`。仓库里有就是装着。

### 6.2 `GET /v1/skills`

```json
{
  "root": "/opt/Nous-skills",
  "repo": { "head": "abc1234", "dirty": false, "ahead": 0, "behind": 0,
            "remote": "https://github.com/JoeWest9966/Nous-skills", "error": null },
  "skills": [ /* §4.1 的 list() 元素，外加一个 tracked */
    { "…": "…", "tracked": true, "category": ["Basics"] }
  ],
  "archived": ["old-thing"],
  "categories": ["Basics", "工具", "工具/搜索"]
}
```

- `tracked` = `git ls-tree -r --name-only HEAD` 里有东西住在这个 `path` 下面。`tracked: false` 且目录在 = **未发布**。
  **按 `path` 比，不是按 `name`** —— 分组里的技能（`deprecated/demo`）用名字去 HEAD 里找永远找不到，全都会被错标成未发布。
- `groups` = 顶层里**没有** `SKILL.md` 的目录名（`deprecated` 那种抽屉）。面板的新建弹窗拿它做建议，不用手打。
- `archived` = `.archive/` 下的目录名，升序。只给名字，恢复不需要别的。
- `repo.error`：git 命令失败时填一行原因，`head` 等给 `null`/`0`，**不要** 500 —— 面板要能显示「仓库读不了」而不是白屏。

### 6.3 复用而非复制（这是票 C 的主要工作量）

`server.js` 里这些函数现在硬编码 `EXTEND`，全部改成**收 `root` 参数**（不留默认值）：

`syncExtendRepo` `extendRepoState` `pendingState` `pendingFileDiff` `pendingCommit` `gitChanges` `isTrackedDir` `archivedEntries` `moveIntoArchive` `moveOutOfArchive` `readCategories` `writeCategories` `regenIndex` `listExtensionFiles` `readExtensionFile` `writeExtensionFile`

写锁：`serializeExtendWrite(task)` → `serializeRepoWrite(root, task)`，`Map<root, Promise>` 排队；同仓串行、两仓互不阻塞。所有写端点（category / file / create / publish / archive / restore / purge / sync / reload）都要在锁里。

**纪律照抄**（每条都有过踩坑记录，别重新发明）：

1. **发布过滤 `D`**：删 tracked 文件必然在工作区留 ` D`，不过滤的话「删了旧技能、隔天改一行点发布」会把删除一起推上去。`archive`/`restore`/`purge` 传 `includeDeleted: true`。
2. `git add` 不能 add 已删路径；add 完用 `git diff --cached` 判断真有无待提交 —— 纯删除没有可 add 的文件，但**必须提交**。
3. **发布 = 校验 + 重扫 + commit + push**，一步。校验是内容闸（§6.4），不是语法闸。
4. 发布成功后前端要重读文件刷新 `base`，否则 diff 还在跟旧 HEAD 比。
5. `purge` 允许「目录不在但暂存区有删除」收尾（上一次断网留下的半截）。
6. 断网 / 非 ff 时错误要上响应体（`pushed: false` + `error` 或 `warnings`），不能只打日志。

### 6.4 内容闸（publish 前必跑）

对每一条技能：① `SKILL.md` 有 frontmatter ② `name === 目录名` ③ `description` 非空 ④ `nous.json` 可解析。任一不过 → 400，`error` 里点名是哪条技能的哪一项（例：`code-review: description 是空的`）。**不要在 md 上跑 `node --check`。**

### 6.5 `create` 的规矩

1. 只问三样：`name`、`description`、`category`（默认 `[]`），外加可空的 `group`（`deprecated` 那种抽屉，最多一层）。
2. 名字：小写优先，允许 `[a-z0-9][a-z0-9._-]*`，≤ 64 字；撞已有目录 / 撞 `.archive/<name>` / 撞词表里的保留名 → 400 且说清撞了谁。
3. **先建 `.new-<name>-<ts>/`（点开头，扫盘天然跳过）→ 写 `SKILL.md` + `nous.json` → `buildIndex` 校验通过 → `rename` 成正式目录**。任何一步翻车整棵删掉。不留半截。
4. 骨架只有 frontmatter（`name` + `description`）+ 一行 `# <name>`。**不写示例正文。**
5. 建完**不提交**（第一个正确状态 = 未发布）。
6. `group` 非空时落在 `<分组>/<name>/`；分组目录不存在就顺手建，先验它不是一条技能（技能里不许套技能）。

### 6.6 正文抽屉的文件操作

- **新建文件 = `PUT` 一个还不存在的 `path`。** 中途的目录一起造出来 —— 这就是「新建文件夹」的实现，
  不另给端点。已存在的文件走同一条 PUT，靠 `base` 防覆盖。
- **新建空文件夹 = `POST /mkdir`**，里面铺一份 `.gitkeep`。空目录 git 不跟踪，没有这份占位，
  它发布那一刻就消失了（然后用户会来问「我建的文件夹呢」）。写进第一个真文件时服务端顺手收掉它。
- **删除 = `DELETE`**，直接 `rm`，不套 `.trash/`：git 历史就是回收站，再存一遍是白存。
  非空目录必须显式 `recursive=1`（409 的原文里说清还有几个东西）。
- 删完之后空掉的父目录一起收；文件列表是**现扫**的，不存索引。

---

## 7. 面板数据模型（票 D1 / D2）

### 7.1 卡片状态（按优先级，只取第一个命中）

| 条件 | 状态 | 视觉 |
|---|---|---|
| `load.state === "failed"` | 装载失败 | 实心红边 + 红角标 + 卡上一行 `error` |
| `tracked === false` | 未发布 | 琥珀虚线边 + 琥珀角标 |
| 其他 | 正常 | 实边 |

归档项**不在网格里**，沉到最后：一个「已归档 N」入口，点开是列表，`恢复` / `彻底删除`（手输名字才亮）。

**没有**「未装入 / 仅仓库 / 未发布到 origin」这些状态 —— 技能没有「装」这个动作。

### 7.2 左栏 facet

- 类别树由 `categories ∪ 每条 skill.category` 现算，`/` 分段。
- **计数对全量算**，不是对当前筛选结果算（facet 是导航，不是统计）。
- 点父级筛出整枝，结果区标题写「工具 · 含 2 个子级」。
- 每行右侧常驻 `⋯`：新建子类别 / 编辑类别名称（填全路径）/ 删除类别（带影响范围「N 条技能」）。

### 7.3 D1 的 mock

D1 不等 C：把 §6.2 的响应原样存成 `content/new1/map/68kazk/public/mock/skills.json`（覆盖 7 条：正常 ×3、未发布 ×1、装载失败 ×1、归档 ×1、未分类 ×1，类别含 `Basics`、`工具`、`工具/搜索`），`app.js` 里一个 `MOCK` 开关读它，接真端点是 D2。**mock 只存在于 D1 分支，D2 必须删掉这个开关和文件。**

---

## 8. 不变量（票里按编号引用）

- **I1** 运行时不联网。`Nous-extend/skills` 里出现 `fetch` / `git` / `api.github.com` / `codeload` / `NW_*` 就是错。
- **I2** 仓库是唯一真相。不读 `data/skills`、不读 `.nw.json`、不维护第二份名单（没有 `extensions.local.json` 的对应物）。
- **I3** `name === 目录名 === 会话里的键`。改名故意不做。
- **I4** 写入口只有一个：`qrd3wh`。面板不跑 git、不读仓库文件。
- **I5** 块之间只走 HTTP，地址只从 `__nous/ctx` 拿。代码里不许出现 `/p/…` 字符串。
- **I6** `render()` 每轮只查内存，不碰盘不碰网不看时间。
- **I7** 半截不留：任何多步写（新建、发布、归档）失败都回到开始前的状态。
- **I8** 面板不复刻语义闸。校验在服务端一处，UI 只摆结果。
- **I9** 链接写相对路径（`cp -r map/<id> ~/<id> && npm start` 还是同一页）。
- **I10** 已删的东西进 `.archive/` 或 `data/` 的垃圾站，不真删；只有 `purge` 才真删。
  技能**里面的**文件是例外：`DELETE /v1/skills/file` 直接 `rm`，因为 git 历史就是那份回收站
  —— 再套一层 `.trash/` 等于把已经安全的东西又存一遍。

---

## 9. 测试约定（每张票都要有）

- 框架：`node --test`，**零依赖**。测试放 `test/`，文件名 `<area>.test.js`。
- 断言用 `node:assert/strict`。
- 命令统一 `node --test test/`，票的验收里直接写这一行。
- 先写失败测试，再写实现（`tdd` skill 会按这个跑）。
- 临时目录用 `fs.mkdtemp(path.join(tmpdir(), "…"))`，`finally` 里删。
- 测的是**行为**：`list()` 的输出、`buildIndex` 的返回值、HTTP 状态码与响应体。**不测 DOM**；UI 归合并后的 `code-review` 看。
- 一个测试文件里可以造 fixture 目录树（`writeFile` 三五行的事），不要为了测装依赖。

---

## 10. 部署（票 E 的一节，别忘）

合并到 integration 分支 ≠ 生效：

1. `content/*` 是运行时数据，块的代码从这里跑：把合并结果落到 `content/new1/map/qrd3wh/` 和 `content/new1/map/68kazk/`。
2. 宿主会因为文件改动收掉进程；下一次请求带着新代码起来（**不会自动重启**）。
3. `Nous-extend/skills` 改了要触发热重载（`POST /v1/extensions/reload` 或等 30 秒轮询）。
4. 开页面点一遍：新建 → 改类别 → 改正文 → 发布 → 归档 → 恢复。

---

## 12. 上游（订阅 / 镜像 / 取用）

一句话：**上游就是一个 git remote。** 这一节之前不存在，是因为那时候上游是 warehouse 的编目 + tarball + 收据 —— 那是在用 HTTP 重建 git 已有的事实。

| 东西 | 存哪 | 进版本库 | 是什么 |
|---|---|---|---|
| `sources.json` | 仓根，跟 `categories.json` 并列 | **是** | 订阅了谁。内容，不是配置 —— 换台机器 clone 下来照样知道 |
| `data/upstream/<name>` | 块的 `NOUS_DATA` | 否 | **只装技能的稀疏工作区**（不是 bare、不是整仓）。缓存，删了下次 fetch 重 clone |
| `nous.json.upstream` | 每条技能自己的目录 | **是** | 注脚 `skills/skills/x/code-review@<tree-sha>`（source 名 / 仓内路径 / tree-sha），只写不读逻辑 |

### 12.1 `sources.json`

```json
{ "v": 1, "sources": [
  { "name": "skills", "url": "https://github.com/mattpocock/skills", "prefix": "skills",
    "version": { "commit": "4ae2317ddb22b046e26455e4f9c9b3aaaf62212d", "at": "2026-11-06T03:41:29.425Z" } }
]}
```

- `name`：`[a-z0-9][a-z0-9._-]{0,31}`，唯一。进 URL、进目录名、进注脚。
- `url`：git 认识的一切（`https://` / `ssh://` / `git@host:path` / `file://` / **绝对路径**）。最后一种让「另一台机器上的 Nous-skills」也能当上游。
- `prefix`：可选，只看仓库里的这一段。
- `branch`：可选，不给就用镜像 HEAD。
- `version`：可选，**我们认的那一版**（见 12.3.1）。`commit` 是 7-40 位十六进制，`at` 是 ISO 时间，
  `tag` / `behind` 是**人看的版本号**（`v1.3.2` / 在 `v1.3.2` 之后）。
- 用 `POST /v1/skills/sources` 整份替换写入，**不提交** —— 「多订阅了一个上游」跟加一条技能一样，走发布。

### 12.2 端点

| 方法 · 路径 | 说明 |
|---|---|
| `GET /v1/skills/upstream` | `{sources:[{…, mirror, head, headVersion, remote, newer, drift, dir, bytes}], entries:[…], errors:[{source,error}]}`。`head` 是镜像停的 commit，`remote` 是上次 `check` 问到的远端（`null` = 没查过），`newer` = `remote.head !== version.commit`（**上游更新了**），`drift` = `head !== version.commit`（**镜像里的东西跟认的不一样**）。两个都服务端算，前端不自己比。`dir` 能 `cd` 进去，`bytes` 是它占的地方。**不联网**：镜像不在就报 `missing` |
| `POST /v1/skills/upstream/refresh` | `{name?}` → 唯一联网动作（clone/fetch）。逐个 source 各报 `ok/error`。**首拉顺手记一次 `version` 基线**，之后不再动它 |
| `POST /v1/skills/upstream/pin` | `{source, commit?}` → 把 `version` 钉到镜像 HEAD（或给的 commit）。**这是唯一改 `version` 的入口** |
| `POST /v1/skills/upstream/check` | `{name?}` → 只 `ls-remote` 问远端 refs（**不下载**），返回 `{head, version, newer}`，结果缓存在镜像的 `.git/nous-check.json` |
| `POST /v1/skills/upstream/sync` | `{source}` → `fetch` + `pin` 一步。**这就是「更新到最新」**：拉下来并认这一版 |
| `GET /v1/skills/upstream/files?source=&path=` | 文件清单：`{path, size, state}` + `adopted`。`state` 是逐文件跟本地比 git blob sha（`same` / `diff` / `missing`），**没取用过就没有意义** |
| `GET /v1/skills/upstream/tree?source=` | `{head, files:[{path,size}]}`。**只有技能**：带 `SKILL.md` 的那些目录里的文件，路径已剥掉 `prefix`。仓里的 `README` / `.github` / `docs` 一条都不出现 |
| `GET /v1/skills/upstream/file?source=&path=&file=` | `{text, size}`，从镜像 `cat-file` 读。**只读** —— 这条路上只有 GET，POST 是 405 |
| `POST /v1/skills/upstream/adopt` | `{source, path, name?}` → 取用（见 12.4） |
| `POST /v1/skills/upstream/copy` | `{source, path}` → 整条抄成自己的技能（重名自动 `-2`）；`{…, file, to}` → 抄单个文件进 `to` 那条（撞名 409）。见 12.4.1 |

`entries[]` 的 `local` 字段（**服务端算，前端只摆**）：

| local | 意思 | 面板上的动作 |
|---|---|---|
| `none` | 本地没有这条 | 取用 |
| `changed` | 取用过，上游这个目录的 tree sha 又变了 | 看差在哪 + 说明「要跟这版得先归档本地那条」 |
| `taken` | 本地有同名但不是从这条取的 | 改名取用（输入框） |
| `same` | 取用过，上游没动 | 无（按钮灰着） |

### 12.3 镜像纪律：**只装技能，不装那个仓**

我们订阅的是**一个仓里的技能**，不是那个仓。`.github/`、`.changeset/`、`docs/`、根 README、
553 个 commit 的历史 —— 这些跟我们一点关系都没有，落到盘上就是垃圾。

判据只有一个：**带 `SKILL.md` 的目录算一条技能**（去掉点开头的、去掉 `prefix` 之外的）。
工作区、抽屉里那棵树、取用的单位，全从这一个判据出去。

四道闸一起上，缺一道整仓就下来了：

| 闸 | 挡掉什么 |
|---|---|
| `--depth=1` | 553 个 commit 的历史 |
| `--filter=blob:none` | 没读过的文件（blob 按需拉） |
| `--no-checkout` + `sparse-checkout set --no-cone /<技能目录>/…` | 工作区里不是技能的东西 |
| `git clean -fd` + 重设 sparse（每次 fetch 后） | 上游删掉的技能 |

**没有「失败了退回全量 clone」这条路。** 那条退路正是当初把整仓拉下来的入口 ——
184 个文件全躺在盘上，用户打开一看：这他妈是别人的仓。「省空间失败」的正确答案是报错，
不是偷偷下 184 个文件。

于是 `dir`（`data/upstream/<name>/<prefix>`）是一个**真的能 `cd` 进去的目录**：
`ls` 就是 39 个技能，`grep -r` 就是在这 39 条里找，粘到编辑器里就是那些 markdown。
「复制路径」复制的是它 —— 不是 GitHub 地址（点不进去、grep 不了）。

- 只在 `refresh` 里 clone/fetch。**`GET /v1/skills/upstream` 的调用路径上没有联网代码** —— 打开面板不打 GitHub。
- 清单从 `git ls-tree -r` + `git cat-file` 读，**不 checkout**：一条技能的清单是一次 tree walk。
- 刷新完顺手 `reflog expire` + `gc --prune=now`（best-effort）：不收拾的话，几十次刷新之后
  盘上全是松散对象，比技能本身还大。

### 12.3.1 版本号是上游的 tag，不是 commit 的前七位

`4ae2317` 是给机器看的。上游自己打了 `v1.3.2`，面板上就该写 `v1.3.2`。

- 拿法：`git ls-remote --tags <url>`（联网，只在 clone / fetch 那一刻做），
  结果缓存在镜像的 `.git/nous-tags.json`。**打开面板不联网**，读缓存。
- 正好打在 tag 上 → `v1.3.2`。
- 提交在 tag 之后（上游天天提交，tag 一周一次）→ `behind: "v1.3.2"`，面板写成 **`v1.3.2+`**，
  念作「v1.3.2 之后的版本」。
- 一个 tag 都没有的仓 → 不编版本号，前端才退回 sha（那时候确实没别的可显示）。
- 采集到的（`headVersion`）和认的（`version`）**都**给人类版本号 —— 不然两边一个 `v1.3.2`
  一个 `4ae2317`，看不出是不是同一版。

### 12.3.2 三个版本，各说各的事

| | 谁写的 | 谁改 | 存哪 | 意义 | 什么时候是它 |
|---|---|---|---|---|---|
| `version` | **人** | `pin` / `sync` | `sources.json`，进版本库 | 我认的是这一版 | 「版本」 |
| 镜像 HEAD | **采集** | 每次 `refresh` | `data/upstream/<name>` 的 git | 我们手上有什么 | 「上游现在」 |
| 远端 HEAD | **检查** | 每次 `check`（只读 refs） | `.git/nous-check.json`（缓存） | 上游此刻到底在哪 | 「上游最新」 |

`newer`（上游更新了）只看远端 ≠ 认的。`drift`（镜像漂了）只看镜像 ≠ 认的。
**两件事，两个布尔，别合并** —— 合并之后「上游有更新」和「你手上有新东西没认」会混成一句，
面板上就分不清该按「更新到最新」还是「记录这个版本」。

为什么要分开：`refresh` 是**读**，`pin` 是**决策**。让 `refresh` 顺手把 `version` 顶掉，
「记录」就自己会跑 —— 人再也说不出「我认的是这一版」，也没法看着 `4ae2317 → 9f2c1ab`
决定要不要跟。所以：

- 首拉（`version` 还空着）= **创建时的基线**，自动记一次。这是唯一一次自动写。
- 之后 `refresh` 只采集。`drift = true` 时卡片上是 `4ae2317 → 9f2c1ab`，浮层里一条
  琥珀提示 + 一个「记录这个版本」按钮。不按，它就一直是两行。
- `pin` 之后 `drift` 立刻变 `false`，按钮收掉。写盘不提交 —— `version` 是内容，走发布。

**同步（用户按的那条路）：**

1. 「检查更新」→ `check`（只读 refs，毫秒级，不下载任何对象）→ 卡片右上角「上游有更新」+
   底部 `v1.3.2+ → v1.4.0`。没检查过就没有这个角标 —— 我们不会偷偷联网。
2. 「更新到最新」→ `sync` = `fetch` + `pin`。一个动作，因为是同一个决定：**跟上游走**。
   拉完 `newer` 和 `drift` 同时归零。
3. 「刷新（fetch）」仍然在 —— 那是「只想看看，不想认」：镜像更新，`version` 不动，
   于是 `drift` 变成 `true`，浮层里出现「记录这个版本」。

### 12.4 `adopt` 的规矩

跟 `create` 同一个形状（I7 半截不留），区别只有一个：骨架不是空的，是从 `cat-file` 里摊出来的。

1. 摊到 `.new-<name>-<ts>/`（点开头，扫盘看不见）——逐 blob 写，二进制原样，不用 tar/shell。
2. **名字对齐**：`name` 默认取上游目录名；给了别的就把 frontmatter 的 `name:` 和 `nous.json.name` 一起改（I3：目录名 == 名字 == 会话里的键）。
3. 写 `nous.json`：`summary` / `category` 取上游的（没有就空），加 `upstream` 注脚。
4. 上游的类别补进词表（跟 `create` 一样的顺序：先词表后 `nous.json` 的地方不能反）。
5. 过内容闸（§6.4）→ 才 rename 进正式目录 → `regenIndex`。
6. **不提交。** 第一个正确状态是「未发布」；发布仍然只有一个按钮。
7. 目标名已存在 / 在 `.archive/` 里 → 409，不覆盖。

### 12.4.1 `copy`：抄，不是取用

**取用**是「我认这一条，将来要跟上游对版本」——写 `nous.json.upstream`，面板上带「上游」角标，
`localStateOf` 认它，`drift` 算它。

**抄**是「这文件我拿走了，现在是我的」——**不写 `upstream`**，没有角标，不算漂移，改了删了都跟上游无关。
只留一行 `copiedFrom: "skills/skills/engineering/tdd"`，那是便条不是契约：将来上游修了个 bug，
你知道该去哪个目录看一眼。

两条路同一个骨架（暂存目录 → 名字对齐 → 过闸 → rename → `regenIndex`），差别只有三处：

| | 取用 `adopt` | 抄 `copy` |
|---|---|---|
| 重名 | 409，让人起个名字 | **整条自动 `<名字>-2`、`-3`…**；单文件 409 |
| `nous.json` | 有 `upstream` | 只有 `copiedFrom` |
| 面板状态 | 「上游」角标，能对版本 | 就是一条自己的技能 |

**为什么整条 `-2`、单文件却 409**：一整条新技能叫 `tdd-2` 不丢东西，你一眼知道是副本；
同一个技能里堆 `notes-2.md`、`notes-3.md`，三天之后没人分得清 —— 那种情况让人先删再抄。

单文件（`{file, to}`）落在 `to` 那条技能的相对路径上（`deep/extra.md` 也能落，目录自动建）。
目标技能不存在 → 404，不在原地凭空造一个。路径 `..` → 400。

### 12.6 面板怎么摆（**一个订阅 = 一条技能**）

**订阅名 = 仓库名。** `https://github.com/mattpocock/skills` → `skills`。早先取的是倒数第二段
（`matt`：一个人的名字），净是误会 —— 订阅的是仓，名字就该是仓的名字。同名两个仓才需要人插手
（前端会拦，服务端 `repoName()` 兜底推）。名字同时是 `data/upstream/<name>/` 的目录名。

上游按「**一个仓 = 一条技能**」理解 —— 这是这一节唯一需要记住的一句。剩下的都跟着它走：

- 上游那一屏是**主区换页**（列的不是「某一条」，是一整张表），但表里**一个订阅只有一张卡**：
  `skills`，写着「整个仓 · 39 条技能」。没有目录卡、没有面包屑、没有 `‹ 上一层`，
  更没有「一条上游技能一张卡」—— 那等于把别人的仓拆成 39 个碎片。
- 点这张卡 = **半扇浮层**（跟本地那条同一扇）：地址、prefix、多少条待看、看正文 / 刷新。
- 浮层里 `看正文 ›` = **那只抽屉**，只读，左栏是**技能的文件树**：只有带 `SKILL.md`
  的那些目录（`engineering/tdd/`）和目录里的文件，`deprecated/` 这种只有 README 的
  分组不出现。路径剥掉 `prefix`，树根就是技能那一层。
- 这个「只装技能」的判据在服务端（`skillHomes`），工作区、树、取用三处共用一份 ——
  想改成「整仓」就只改它一处，但那等于把别人的 `.github` 搬到自己盘上。
- **取用仍然按「一条」算**（一个仓里 39 条技能是能分别取的），所以它挂在树里那条
  `SKILL.md` 的行尾；浮层里只留一句话指路。这是全节唯一的例外，有意的。
- **抄在行菜单里**（右键 / 行尾 `⋯`），不在行上挂徽标：一屏 108 个「抄」字是噪音墙，
  而且最后会把人逼到「要 hover 才看得见 = 不存在」。菜单里两项：
  - 目录（装着 `SKILL.md` 的）：「抄进我的技能」—— 整条进仓，重名自动 `-2`、`-3`；
  - 文件：「抄到我的技能…」—— 在该行下面展开一行「抄到 ⟨我自己的技能⟩」，内联选完就抄。
    不弹模态：弹模态会盖住你正在比的那棵树。
- **抄完给一条去路**：toast 是「抄进你的仓了：`tdd`」+「打开它 ›」，按了直接换成那条的
  可写抽屉。只说「成功」不说「在哪」等于没说完 —— 人刚才看的还是别人的仓。
- 左栏 = **订阅列表**（一个订阅一行）。目录不在这儿 —— 目录只在抽屉那棵树里。
- **地址只到订阅：`#upstream/skills`。** 老的深链接也只落到这一层，后面几段当没看见；
  名字对不上（改过名、`#upstream/matt`）就落到第一个订阅 —— 地址换了不该是白屏。
- 订阅多于一个时，左栏一行一个，主区一张卡一个。

`model.js` 里跟「清单折成树」有关的零件（`upstreamTree` / `upstreamAt` / `upstreamCrumb` /
`sortUpstream`）**全删了** —— 上游那一层已经没有清单这回事，留着就会有人再接回去。

### 12.5 不做

- 定时同步、后台 fetch、双向推送（撞 I1 / I6）。
- 「上游一改，本地自动跟着改」。取用是**一次性拷贝**：`nous.json.upstream` 只回答「这条哪来的」，不参与任何判断。让它变成活的追踪关系 = 回到会漂的第二份真相。
- 覆盖式取用。要跟上游的新版本：归档本地那条 → 再取用。

## 11. 不在范围内

- 技能之间的依赖声明（`needs` 那套）。
- 技能的版本号 / 上游 tag / 漂移比对。
- ~~技能变体（同一技能多个可装载名）~~ —— **已实现**（见 §0 的 2026-11 修订）。
- 多用户 / 权限 / token。
- 从别的仓库「导入」技能（要复制就 `cp -r`，然后自己 commit）。
