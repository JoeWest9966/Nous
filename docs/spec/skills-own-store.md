# spec · skill 自持私库（脱钩上游编目）

**这份文件是四个并行 implementer 唯一的约定来源。** 票只写「执行本 spec 的哪一节 + 自己的验收」，不复制这里的形状。
改形状先改这里，再改票。

轨道：`JoeWest9966/Nous#4`（总票）。子票：`#5` A · `#6` C · `#8` D1 · `#9` D2 · `#7` E · `JoeWest9966/Nous-extend#1` B。

---

## 0. 一句话

skill 从「上游编目的投影」变成「我们自己 git 仓库里的文件」。照 Nous 扩展（`/opt/Nous-extend` + 块 `vbe4pt`）那套：仓库是唯一真相，`qrd3wh` 是唯一写入口，面板是纯静态视图。

**非目标**：上游编排 / 技能市场 / 技能搜索 / 从 GitHub 拉别人的技能 / 技能版本号 / 技能的「变体」。这些都不做，别顺手加。

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
├── .archive/<name>/         废弃的技能整目录搬这儿，git 追踪，能恢复
└── <name>/                  一个目录 = 一条技能，目录名 == 技能名 == 会话里的 API 键
    ├── SKILL.md             frontmatter + 正文
    ├── nous.json            {"name","summary","category":[]}
    └── …                    可选：这条技能自带的参考文档 / 脚本（同目录，相对路径引用）
```

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
- 没有 `SKILL.md` 的目录跳过（可能是留了别的东西），**静默**，不算坏账。
- 有 `SKILL.md` 但 frontmatter 缺 `name` / `description`、或 `name !== 目录名` → `failed` + `error`。
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
| `PUT /v1/skills/file` | `{ name, path, text, base }` | `{ etag }`；`base` 与当前不一致 → 409 |
| `POST /v1/skills/create` | `{ name, description, category? }` | `{ skill, skills }` |
| `POST /v1/skills/publish` | `{ message? }` | `{ ok, commit: {hash,subject} \| null, pushed: bool, warnings: string[] }` |
| `POST /v1/skills/archive` | `{ name }` | `{ ok, commit, pushed }` |
| `POST /v1/skills/restore` | `{ name }` | `{ ok, commit, pushed }` |
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

- `tracked` = `git ls-tree --name-only -d HEAD` 里有这个目录。`tracked: false` 且目录在 = **未发布**。
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

1. 只问三样：`name`、`description`、`category`（默认 `[]`）。
2. 名字：小写优先，允许 `[a-z0-9][a-z0-9._-]*`，≤ 64 字；撞已有目录 / 撞 `.archive/<name>` / 撞词表里的保留名 → 400 且说清撞了谁。
3. **先建 `.new-<name>-<ts>/`（点开头，扫盘天然跳过）→ 写 `SKILL.md` + `nous.json` → `buildIndex` 校验通过 → `rename` 成正式目录**。任何一步翻车整棵删掉。不留半截。
4. 骨架只有 frontmatter（`name` + `description`）+ 一行 `# <name>`。**不写示例正文。**
5. 建完**不提交**（第一个正确状态 = 未发布）。

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
| `data/upstream/<name>` | 块的 `NOUS_DATA` | 否 | bare 镜像。**缓存**，删了下次 fetch 重 clone |
| `nous.json.upstream` | 每条技能自己的目录 | **是** | 注脚 `matt/skills/x/code-review@<tree-sha>`，只写不读逻辑 |

### 12.1 `sources.json`

```json
{ "v": 1, "sources": [
  { "name": "matt", "url": "https://github.com/mattpocock/skills", "prefix": "skills" }
]}
```

- `name`：`[a-z0-9][a-z0-9._-]{0,31}`，唯一。进 URL、进目录名、进注脚。
- `url`：git 认识的一切（`https://` / `ssh://` / `git@host:path` / `file://` / **绝对路径**）。最后一种让「另一台机器上的 Nous-skills」也能当上游。
- `prefix`：可选，只看仓库里的这一段。
- `branch`：可选，不给就用镜像 HEAD。
- 用 `POST /v1/skills/sources` 整份替换写入，**不提交** —— 「多订阅了一个上游」跟加一条技能一样，走发布。

### 12.2 端点

| 方法 · 路径 | 说明 |
|---|---|
| `GET /v1/skills/upstream` | `{sources:[{…, mirror:"ready"｜"missing"}], entries:[…], errors:[{source,error}]}`。**不联网**：镜像不在就报 `missing` |
| `POST /v1/skills/upstream/refresh` | `{name?}` → 唯一联网动作（clone/fetch）。逐个 source 各报 `ok/error` |
| `GET /v1/skills/upstream/files?source=&path=` | 逐文件跟本地比 git blob sha → `same` / `diff` / `missing` |
| `POST /v1/skills/upstream/adopt` | `{source, path, name?}` → 取用（见 12.4） |

`entries[]` 的 `local` 字段（**服务端算，前端只摆**）：

| local | 意思 | 面板上的动作 |
|---|---|---|
| `none` | 本地没有这条 | 取用 |
| `changed` | 取用过，上游这个目录的 tree sha 又变了 | 看差在哪 + 说明「要跟这版得先归档本地那条」 |
| `taken` | 本地有同名但不是从这条取的 | 改名取用（输入框） |
| `same` | 取用过，上游没动 | 无（按钮灰着） |

### 12.3 镜像纪律

- 只在 `refresh` 里 clone/fetch。**`GET /v1/skills/upstream` 的调用路径上没有联网代码** —— 打开面板不打 GitHub。
- 清单从 `git ls-tree -r` + `git cat-file` 读，**不 checkout**：一条技能的清单是一次 tree walk。
- `--filter=blob:none` 省空间；服务端不支持就退回全量 clone（「省空间失败」不等于「上游用不了」）。

### 12.4 `adopt` 的规矩

跟 `create` 同一个形状（I7 半截不留），区别只有一个：骨架不是空的，是从 `cat-file` 里摊出来的。

1. 摊到 `.new-<name>-<ts>/`（点开头，扫盘看不见）——逐 blob 写，二进制原样，不用 tar/shell。
2. **名字对齐**：`name` 默认取上游目录名；给了别的就把 frontmatter 的 `name:` 和 `nous.json.name` 一起改（I3：目录名 == 名字 == 会话里的键）。
3. 写 `nous.json`：`summary` / `category` 取上游的（没有就空），加 `upstream` 注脚。
4. 上游的类别补进词表（跟 `create` 一样的顺序：先词表后 `nous.json` 的地方不能反）。
5. 过内容闸（§6.4）→ 才 rename 进正式目录 → `regenIndex`。
6. **不提交。** 第一个正确状态是「未发布」；发布仍然只有一个按钮。
7. 目标名已存在 / 在 `.archive/` 里 → 409，不覆盖。

### 12.5 不做

- 定时同步、后台 fetch、双向推送（撞 I1 / I6）。
- 「上游一改，本地自动跟着改」。取用是**一次性拷贝**：`nous.json.upstream` 只回答「这条哪来的」，不参与任何判断。让它变成活的追踪关系 = 回到会漂的第二份真相。
- 覆盖式取用。要跟上游的新版本：归档本地那条 → 再取用。

## 11. 不在范围内

- 技能之间的依赖声明（`needs` 那套）。
- 技能的版本号 / 上游 tag / 漂移比对。
- 技能变体（同一技能多个可装载名）。
- 多用户 / 权限 / token。
- 从别的仓库「导入」技能（要复制就 `cp -r`，然后自己 commit）。
