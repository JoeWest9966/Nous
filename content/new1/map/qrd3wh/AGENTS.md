# qrd3wh · session 引擎

一个磁盘一个 sqlite，里面 N 个 conversation。会话不是列表，是一棵树：`ConversationRecord.parent`
就是那根枝，不自己记。

## 模型从哪儿来：**Nous Warehouse 编目，不读本机文件**

```
https://j1900.tail8d0018.ts.net:8443/api/model?models=full   ← 编目（唯一真相）
        ↓ catalog.js 拉一次 + 字段映射 + 连密钥
pi-ai provider（内存）                                        ← 运行时只读它
```

上一版读 `~/.pi/agent/models.json`（`models-json.js`，**已删**）。改的原因不是文件不好用，
是**真相有两份**：那台机器上谁手写的 23 个模型，跟编目里登记过的 148 个，永远是两份会漂的事实。
现在编目是唯一真相，Nous 是它的投影。

三条硬规矩：

1. **密钥只在内存。** 启动时 `GET /api/model/{名}/secret` 拉一次，`auth.resolve()` 之后从 Map 取。
   不落盘、不进日志、不进 session 记录。代价说清：**编目里轮换了 key，本进程不重启就一直用旧的**
   —— 用 `POST /v1/catalog/refresh` 抵消（重拉清单 + 密钥，不用真重启）。
2. **编目说不出来的字段靠注解，不靠猜。** 上游 `/models` 不报「能不能思考」（volc-ark 一个字段都没有），
   所以 `reasoning` / `thinkingLevelMap` / `cost` / `compat` / `input` 是编目里**人写的注解**，
   `nw model register` 覆盖写会保住它们（见 Nous-Warehouse `preserveAnnotations`）。
   `catalog.js` 只做**格式映射**，绝不按 id 猜能力 —— 猜错一个，面板上就多一档点得下去、跑起来 400 的思考。
3. **编目挂了要说清。** `/v1/models` 的 `catalog` 块带 `error` / `at` / `skipped[]`：
   只回 `items` 的话，「编目挂了」在页面上长得跟「编目里没模型」一模一样，而这两种要干的事完全不同。

### 字段映射（别自己发明）

| pi-ai 要的 | 编目里可能长的样子 |
|---|---|
| `contextWindow` | `contextWindow` / `context_length` / `max_allowed_size` / `token_limits.context_window` |
| `maxTokens` | `maxTokens` / `max_output_tokens` / `token_limits.max_output_token_length` |
| `reasoning` | `reasoning`（注解）/ `supports_reasoning` / `only_reasoning` |
| `thinkingLevelMap` | `thinkingLevelMap`（注解）/ `reasoning_supported_efforts` / `supported_reasoning_levels[].effort` |
| `input` | `input`（注解）/ `modalities.input[]` / `modalities.input_modalities[]` / `supports_images` |

`xhigh` / `max` 只有写进 `thinkingLevelMap` 才算 —— pi-ai 的规矩，别给它补默认值。

**别名**：Ark 那类供应商的 `/models` 只列带日期的正式 id，人用的是 `glm-5.3` / `ark-code-latest`。
这些在编目里带 `local: true`，自扫不删。`catalog.js` 照单收，不需要特判。

## 思考等级

档位表**是模型的**，不是我们的：`levelsOf(model) = getSupportedThinkingLevels(model)`。
非 reasoning 只有 `["off"]`；reasoning 的上 `low..high` 默认都在。服务端收档位时用
`fitLevel()` 夹回模型真有的那一档再落盘（客户端发的值不可信）—— 面板显示 A、库里存 B 是要防的病。

## 自研扩展住在另一个仓库：`Nous-extend`

**源码不在这棵树里。** 在 `NOUS_EXTEND`（默认 `/opt/Nous-extend`，GitHub 同名仓库）——
块里只留两件事：去哪儿拿（`server.js` 顶部那两行动态 import）+ 钉着哪个 commit
（`extensions.lock.json`）。

```
/opt/Nous-extend/
  index.json        ← 扫出来的目录（`node index.mjs`），别手改
  nous/  nous.json + index.js
  skills/ nous.json + index.js
```

**每个扩展都是「造扩展的函数」，宿主递 api 进去**：

```js
const api = { defineExtension, section };
const { default: createNous } = await import(`${EXTEND}/nous/index.js`);
registry.install(createNous(api));
const skills = await createSkills({ dir, registry, api });
```

Why 不是直接 `import nous from "..."`：扩展**不许自己 import pi-durable**。仓库住在宿主的
`node_modules` 之外，自己去找它只剩符号链接或者本地重装一份两个下策 —— 前者离开宿主就断，
后者版本会漂。递 api 之后那个仓库只用 node 内置模块，能单独 checkout、单独跑测试。

`nous.json` 是给宿主和人看的清单（`file` / `entry` / `needs` / `sections` / `default`），
**不进提示词**。以前块里的 `extensions/manifest.json` 已拆成每目录一份，跟着代码走。

三条不进清单：

- **pi-durable 自带的**（`CodingTools` 那四件套）—— 不是我们开发的。谁写的代码谁进账。
- **`skill:<名>` 名牌** —— 运行时 `skillExtension(name)` 现造的家族，仓库里有几条技能就是几个空对象。
  列进清单会变成一行行生成物，没人看得懂。它归「skills 扩展派生」这一个条目。
- **技能内容** —— 落 `/opt/Nous-skills/<名>/`（另一个 git 仓，见下面「技能」那节），不进清单、不进本块。

**扩展名就是 API**：会话里存的是名字不是对象（durable 的 `AgentState`），改 `name` = 破坏性变更。

两条与清单字段有关的约定（都是「别让字段当装饰品」）：

- **`default: true` 现在真生效**（`POST /v1/sessions`）：新会话默认选中 = harness 里那三个
  硬编码的（`CodingTools` / `nous` / `skills`）**加**清单里 `default === true` 且已装的扩展。
  以前这个字段只有页面拿来画绿角标、没人读 —— 改了没反应，比没有更糟。
- **`installed` 只列本体，`loadable` 才列全**：变体在 registry 里是独立名字（`123@dev`），
  但「装了哪些扩展」这张表按本体算 —— 卡片是一张，变体在 `variants` 里按本体归位。
  谁要列出「这条会话能装哪些名字」，读 `loadable`（本体 + 已装载的变体 key），别自己去拼 `variants`。
- **`sections` 只是声明，真值在 registry 里**：`GET /v1/extensions` 回一个 `live`
  （`{扩展名: [真装上的 section 键]}`），扩展清单页拿它跟 `nous.json` 对账。
  页面显示的「提示词 N 段」是手写数，代码改了忘改清单时只有 `live` 能拆穿。

### 提交范围由调用方选：`paths`

`syncExtendRepo(message, { paths })` 只提交**点名的那几条路径**（不在 `paths` 里的脏文件
一个字节都不碰），并逐个对 `.js` 跑 `node --check` —— 这是面板「未同步」弹窗的实现，
也是把「一次点击推整个工作区」换掉的代价。

- 不带 `paths` = 老行为：`only: "categories"`（只词表/清单），类别那 8 秒 debounce 走它。
- 带 `paths: []` = 纯推送：没有新提交就只 `push`（「已提交没推」那一段用它）。
- 删除防火墙照旧：`includeDeleted` 为假时，` D` 的文件不进提交 —— 那是「删本地」的私事。
- 提交失败要说实话：push 挂了回 `{committed: true, hash, pushed: false, pushError}`，
  调用方据此把「已提交但没推」和「什么也没发生」分开显示。
- **`index.json` 里的未发布条目不上车**：类别那条是自动提交，它不能替谁发布。提交前把
  「HEAD 里没有这个目录」的条目从索引里摘掉，提交完再 `runIndexGenerator()` 扫回来
  （磁盘上会重新变脏，这正是该有的样子），回复里带 `skippedIndex[]` 让页面说一句。
  不这么做的话：新建/复制出来的未发布扩展，会在你下次「改个类别」时被自动推上 GitHub ——
  仓库说「我有它」，实际没有。踩过一次，见 `353ea42`。

**`publish` 只发被发布的那个扩展**：`publishExtensions({name})` 收窄成
`name/` 底下的一切 + `index.json`，语法闸也只查这些 js（别人工作区里的语法错误不再拖住
你的发布）。以前是「提交整份工作区」，抽屉里点一次发布能把别人未发布的目录带走 ——
测试副本就是这么上过 GitHub（`319fd58` → `748dcf5` 清掉）。

`index.json` 的摘除规则一句话：**「HEAD 里没有它」且「这次的 paths 里也没有它」才摘**。
所以发布一个新扩展时它的条目照常进索引，别人的未发布条目照常被摘。

`POST /v1/extensions/duplicate {from, variant?, name, summary?, category?, unbind?}`：复制成新扩展（暂存目录 +
改名 + 校验 + rename，历史不带）。**名字写死在入口里，所以复制必须先改名**：只认
`name: "旧名"` 这种字面量，找不到就整体回滚报错 —— 否则两个扩展注册同一个名字，新的顶掉旧的。

带 `variant` 时源换成**那个变体自己那棵树**（`variantRootOf()`），于是「转正」和「复制」是同一条
代码：变体的代码就这么进了主树，成为未发布的新扩展。`unbind: true` 才顺手解绑 —— 只动绑定，
树和分支不碰。`GET /v1/extensions/variant/usage?key=名称@变体` 回「还有几个会话点着这个名字」。

配套三个只读接口：`GET /v1/extensions/pending`（按**扩展**分组的改动 + 没推的提交）、
`/pending/diff?path=`（一个文件对 HEAD 的前后内容）、`/pending/commit?hash=`（一条没推的
提交改了哪些文件）。分组的单元是扩展不是文件 —— 人想的是「把 123 发出去」。

### 变体：`名称@变体` 就是一个注册名

会话里存的是名字，所以「同一扩展的多个版本 / 模式」在运行时只能是**多个注册名**：
`nous` 之外再装一个 `nous@review`。装载层只做三件事：

1. **来源两处**：`nous.json` 的 `variants`（只带 `params`，跟着仓库走）+
   `data/extensions.local.json` 的 `variants`（指向本机某棵 checkout，不跟着仓库走）。
   同一个 id 以本机为准，因为本机那份是「现在这台的代码在哪」。
2. **造法一样**：同一个工厂多递一个 `{ variant, ...params }`，装之前把 `name` 覆盖成
   `名@变体`（`{...extension, name}`）。扩展不用知道 `@` 的存在，registry 也只是按名字索引。
3. **核心两条的变体要装两处**：`nous` / `skills` 的本体在启动路径上硬编码 import，所以它们的
   变体走 `loadCoreVariants()` —— `syncExtraExtensions` 的过滤会把核心家族整体跳过。
   `installCore`（重装）和**启动序列**都要调它：只挂在前者上，进程一重启 `nous@review` 就没了，
   得等人点「重试装载」或推一次代码才回来 —— 又一个「页面说装了、进程里没有」。

**变体有自己的文件面**：`/v1/extensions/files`、`/file`、`/publish` 都认 `variant` 参数 ——
根目录换成那棵绑定目录，差异基线取**那棵树自己的** `HEAD`（拿主树 HEAD 比会显示成整篇重写），
发布则在那棵树上 `add` + commit + `push -u origin <分支>`。主树的发布路径一个字没改：
两条路都串在 `serializeExtendWrite` 里，但碰的是不同仓库。

`@` 是分隔符，别用 `:`：`skill:` 是运行时空白名牌的前缀，`skillNamesOf` 会误伤。
变体失败只记它自己那条 `load`（`名@变体` → failed），**不连坐本体**：变体可能指向另一份代码，
本体那份坏掉不该把它一起判死。

## 技能：文件在 `/opt/Nous-skills`，这个块只是写入口

技能的**仓库是唯一真相**（`docs/spec/skills-own-store.md`，I2）。`/opt/Nous-skills` 是自己的
git 仓，一条技能 = 一个目录：

```
/opt/Nous-skills/
├── categories.json          词表（给面板左栏用，不是事实源）
├── index.json               生成物：扫盘重写，别手改
├── index.mjs                扫盘器（CLI 薄壳，仓库自己带）
├── .archive/<名>/           归档的整目录（**扁的**，不跟分组），git 追踪，能恢复；只有 purge 才真删（I10）
├── <分组>/                   可选：只装技能的抽屉（`deprecated` / `engineering`），最多一层
│   └── <名>/SKILL.md …       形状同上
└── <名>/
    ├── SKILL.md             frontmatter（name == **最后一段**目录名、description 非空）+ 正文
    ├── nous.json            {name, summary, category[]}
    └── …                    可选：自带的文档 / 脚本，**可以分子目录**（`references/x.md`）
```

**分组只是抽屉，名字还是最后一段。** `deprecated/demo/` 那条技能叫 `demo`，`index.json` 里
多一个 `path: "deprecated/demo"`。所以这个块里所有按 `name` 找目录的地方都得先翻一次
（`skills-api.js` 的 `relOf` / `homeOf`）—— 谁自己拼 `path.join(root, name)`，分组里的技能就全 404。

**没有** `package.json`、`node_modules`、示例技能。空仓就是空仓。

`Nous-extend/skills/index.js` 是这份仓库的**投影**：`createSkills({ dir })` 里的 `dir` 是仓库根
（`NOUS_SKILLS`，默认 `/opt/Nous-skills`）。`load()` 扫 `<dir>/*`：有 `SKILL.md` 就是技能，
没有的当分组往里找一层（`MAX_GROUP_DEPTH = 1`），读 frontmatter 进内存名册。两个抽屉里有同名
技能时，后来的留成 `failed` 并点出先到的那条 —— 名字是会话键，只能有一个生效。
不联网、不拉 tarball、不读收据、不维护第二份名单 —— 盘上删掉一个目录，下次扫盘它就不在了。

跟 pi CLI 同一套规矩（同一份 `<available_skills>` XML、同样让模型用 `read` 去读正文、同样按
SKILL.md 所在目录展开相对路径），所以给 pi 写的技能直接能用。**`<location>` 是绝对路径**
`/opt/Nous-skills/<名>/SKILL.md` —— 模型拿它直接 `read`，不用猜前缀。

**三层，只有第三层跟代码有关系：**

| 层 | 是什么 | 在哪 |
|---|---|---|
| 内容 | `SKILL.md` + 这条技能自带的文档 / 脚本 | `/opt/Nous-skills/<名>/` 真文件（模型 `read` 它，`bash` 也能跑它） |
| 名册 | `{name, description, 绝对路径}` | 内存 Map（`load()` 扫一次；每个写端点之后重扫） |
| 钩子 | `defineExtension({ name: "skill:<名>" })` | **空名牌**，不含一个字的技能文本，只为在 `agent.extensions` 里占个名字 |

为什么不自己建一张「哪条会话会哪些技能」的表：`pi.agent.extensions` 就是那张表，而且
durable 白送 fork 继承、重启存活、热替换、**加一条只追加一行 `pi.system` 增量**（不重写历史，
provider 的提示词缓存还热着）。自己存一份 = 重造一个 fork 不继承、重启要迁移的轮子。

三条不许踩：

1. **`render()` 每轮请求都跑**，所以它只查内存 Map —— 不碰文件系统、不碰网络、不看时间、
   顺序按名字排。漏一样进去，就是每轮追加一条 `pi.system` 并打掉提示词缓存。
2. **空名牌不上默认选择**（`settings.extensions`）。上了 = 每条新会话一出生就吃
   仓库里每条技能一条描述，progressive disclosure 当场作废。默认只放真扩展（`CodingTools` / `Nous` / `skills`）。
3. **写失败要说清到哪一步**（多半是内容闸或 git）。只回一个红条，人得自己猜。

### 写入口只有一个：`/v1/skills*`（I4）

**技能正文和名册住在 `/opt/Nous-skills` 这个仓库里，`qrd3wh` 是它唯一的写入口；`68kazk` 只是视图。**
面板不跑 git、不读仓库文件、也不复刻语义闸（I8）—— 校验只在这里一处，UI 只摆结果。
三种状态的判据是**盘 + HEAD**，不是谁记的账：

| 盘上 | HEAD 里 | 是什么 |
|---|---|---|
| 有 `<path>/` | 有 | 已发布（`tracked: true`）—— 比的是 `path`（`deprecated/demo`），不是 `name` |
| 有 `<path>/` | 没有 | 未发布（`tracked: false`，面板上琥珀虚线） |
| `.archive/<名>/` | 有 | 已归档，能 `restore` 回来；`purge` 才真删 |

- **新建**只落工作区（`tracked: false`），**发布**才 commit + push。
- **发布 = 闸 + 重扫 `index.json` + commit + push，一步**：闸（frontmatter 的 name/description、
  `nous.json` 的三字段）不过，一个字节都不动（`index.json` 也不重扫，否则留下「扫了一半」的假象）。
- **每个写端点都在本仓的串行锁里，写完重扫名册**：不重扫的话，面板说有、会话里看不见 ——
  两份真相就是这么长出来的。

**上游只有读。** 那一侧（`upstream.js`）没有任何写入口：没有 `upstream/file` 的 PUT，
没有定时同步，没有双向推送。看正文和「看差在哪」都是 `git cat-file` 读镜像 ——
**镜像里只有技能**：`--depth=1 --filter=blob:none` + sparse checkout 到「带 SKILL.md 的目录」，
没有 `.github/`、没有别人的 README、没有 553 个 commit。判据是 `skillHomes()`，
工作区 / `upstream/tree` / 取用三处共用它。

「上游一改本地跟着改」是刻意不做的（那会回到会漂的第二份真相）。

历史一句：连到 2026-10，技能是「从上游编目把 tarball 拉进 `data/skills/`、按 sha 记收据、
用 bare clone 缓存算版本号」。那套的第二真相（收据 + 缓存 + 编目快照三份会漂的账）就是它
被删的原因 —— 收据格式、缓存目录、安装/卸载端点全没了，别照着老记忆写。

## 提示词模板：文件在 `/opt/Nous-prompts`，跟技能是兄弟但**单选**

一条模板 = `<名>/PROMPT.md`（正文）+ `nous.json`（`{name, summary, category[]}`）。
仓库形状、写入口（`/v1/prompts*`）、内容闸、归档都是照着技能仓的模子刻的；三处不同：

1. **正文整段进系统提示词。** 所以选择是**单值**：会话里最多一个 `prompt:<名>`。
   多条同时选中时 `render()` 只吐第一条 —— 两份互相矛盾的沟通规则一起进提示词，模型只会挑它喜欢的那个。
2. **没有 frontmatter。** 摘要只给挑模板的人看（正文 7KB，没有摘要等于闭眼选），所以它住在 `nous.json`。
3. **它是核心第三条扩展**（`CORE_EXTENSIONS`）。不是清单扩展：`syncExtraExtensions` 递的 `bag.dir`
   指向 `DATA/skills`，而模板仓在 `/opt/Nous-prompts`。所以跟 `skills` 一样走启动路径硬编码 import +
   `loadCoreVariants` 的 bag 里多一个 `promptsDir`。

`template` 三态别合并：字符串 = 选它，`null` = 显式不要，不传 = 用仓库 `settings.json` 的 `default`。
回参里 `template` 是**生效值**；`missingTemplates[]` 是「勾了但没装、已跳过」。

名牌跟技能同一档：`prompt:<名>` 不进 `installed`，只在 `nameplates` 里（统一用
`NAMEPLATE_PREFIXES`，别逐处写 `startsWith("skill:")` —— 漏一处就是面板里多一行生成物）。

## 接口

```
GET  /                            那棵树
GET  /v1/device                   我是谁、模型、编目状态
GET  /v1/models                   items[]（含档位表/窗口/输出上限）+ catalog{base,at,error,providers[],skipped[]}
POST /v1/catalog/refresh          重拉编目与密钥（换密钥 / 编目里加了供应商之后）
GET  /v1/skills                   仓库全景：root + repo{head,dirty,ahead,behind,remote} + skills[]（带 tracked）+ archived[] + categories[]
GET  /v1/skills/categories        词表（受保护类别：一个都不护，见 spec §3.6）
POST /v1/skills/categories        {op:"add"|"rename"|"remove", name, to}
POST /v1/skills/category          {name, category[]} → 写这条技能的 nous.json + 并进词表
GET  /v1/skills/files             ?name= → 这条技能的目录里有什么（递归，含子目录）
GET  /v1/skills/file              ?name=&path= → { text, etag }
PUT  /v1/skills/file              {name, path, text, base} → { etag, path, created }
                                  目标不存在 = 新建（中途目录一起造）；base 对不上 = 409
POST /v1/skills/mkdir             {name, path} → 建空文件夹，里面铺 .gitkeep
DELETE /v1/skills/file            ?name=&path=[&recursive=1] → 删文件 / 删目录，不提交
                                  目录非空且没 recursive=1 = 409
POST /v1/skills/create            {name, description, category[], group?} → 只落工作区，不提交
                                  group = `deprecated` 那种抽屉（一层），顺手建
POST /v1/skills/publish           {message} → 闸 + 重扫 index.json + commit + push
POST /v1/skills/archive           {name} → 挪进扁的 .archive/ 并提交；原址记进 nous.json.archiveOrigin
POST /v1/skills/restore           {name} → 按 archiveOrigin 搬回原来的分组并提交
POST /v1/skills/purge             {name} → 真删（只有这一条会真删）
GET  /v1/skills/pending           待同步清单（按目录分组）
GET  /v1/skills/pending/diff      ?path= → { path, before, after }（对 HEAD；未跟踪 before: null）
GET  /v1/skills/pending/commit    ?hash= → { hash, subject, files:[{path,status}] }（status 是 A/M/D/R）
POST /v1/skills/sync              {message, paths?} → 只提交点名的路径
POST /v1/skills/reload            重扫仓库（拿不准的时候用；写端点自己会扫）
GET  /v1/skills/sources           订阅了哪些上游（sources.json 是内容，进版本库）
POST /v1/skills/sources           {sources} 整份替换，不提交
GET  /v1/skills/upstream          {sources:[…mirror], entries:[…], errors[]}。**不联网**：镜像不在就报 missing
POST /v1/skills/upstream/refresh  {name?} 唯一联网动作（clone / fetch）
GET  /v1/skills/upstream/file     ?source=&path=&file= → {text, size}：从镜像 cat-file 读，**只读**
GET  /v1/skills/upstream/files    ?source=&path= → {files:[{path,size,state}], adopted}；state 是跟本地比
POST /v1/skills/upstream/adopt    {source, path, name?, group?} → 取用（未发布）
GET  /v1/prompts                  模板私库全景：repo + prompts[]（带 tracked）+ archived[] + categories[] + default
GET  /v1/prompts/categories       词表（同样一个都不护）
POST /v1/prompts/categories       {op:"add"|"rename"|"remove", name, to}
POST /v1/prompts/category         {name, category[]} → 写这条模板的 nous.json + 并进词表
GET  /v1/prompts/files            ?name= → 目录里有什么
GET  /v1/prompts/file             ?name=&path= → { text, etag }
PUT  /v1/prompts/file             {name,path,text,base} → { etag }；base 对不上 = 409
POST /v1/prompts/create           {name, summary, category[], body?} → 只落工作区，不提交
POST /v1/prompts/publish          {message} → 闸 + 重扫 index.json + commit + push
POST /v1/prompts/archive|restore|purge {name}
GET  /v1/prompts/pending          待同步清单（一个模板 = 一个单元）
POST /v1/prompts/sync             {message, paths?}
POST /v1/prompts/reload           重扫名册
GET  /v1/sessions                 会话树
POST /v1/sessions                 {name, model, thinkingLevel, skills, template}
POST /v1/sessions/:id/config      {readOnly, thinkingLevel, skills, template}
POST /v1/sessions/:id/messages    {content, requestId?, waitMs?}
GET  /v1/sessions/:id/events      SSE
GET  /v1/sessions/:id/entries     ?system=1 额外给 items[].delta + system（调试提示词用）
```

环境：`PORT`（默认 4780）、`NOUS_DATA`、`NOUS_SKILLS`（技能仓根，默认 `/opt/Nous-skills`）、
`NOUS_PROMPTS`（模板仓根，默认 `/opt/Nous-prompts`）、
`NOUS_EXTEND`（扩展仓，默认 `/opt/Nous-extend`）、`NW_BASE`（默认 tailnet 8443，**只给模型编目**）、
`NW_TOKEN`（走 panel-gate 才要）。

### 提示词是增量的 —— `?system=1` 得回放才看得见

durable 存提示词不存全文，存**差值**：每个 `pi.system` 条目只带变了的段
（`message.sections` 是 `{key: 文本 | null}`，`null` = 这一段被删），工具同理
（`toolsAdded` / `toolsRemoved`）。所以「这一条 system 里有什么」永远答不了
「这一轮提示词里有什么」。

`prompt-log.js` 的 `replayPrompt()` 按时间顺序回放整条会话，给出**此刻生效**的提示词。
`?system=1` 于是给两样，别搞混：

```
items[].delta   { sections, toolsAdded, toolsRemoved }   这一条自己带了什么（增量原文）
system          { sections, tools, scanned, truncated }  回放后此刻生效的（人真正想看的）
```

两条实测才知道的事：

- **`sections` 是对象不是数组**，而且只在**变化**时出现 —— 一条只换工具的 system 条目
  里根本没有 `sections` 这个键。原来那版直接读 `message.sections` 再当全量显示，
  结果是永远 `null`。
- **没跑过的会话，回放就是空的**，而且这是对的：段还没被渲染过，库里就没东西可回放。
  `scanned` 一栏就是为了让人能分辨「空」和「没读到」。

## 不做什么

- 不读 `~/.pi/agent/models.json` —— 那是 pi CLI 的配置，跟 Nous 无关。
- 不把密钥写进 `data/` —— 那目录会被 `cp -r` 带走，而块目录不该装秘密。
- 不按 id 猜 `reasoning` / 上下文窗口。宁可少一档思考，不要连着 400。
- 不读本机任何 `skills.json` / `~/.pi/agent/skills` —— 技能只认 `/opt/Nous-skills` 那份仓库，
  别的地方出现的技能清单都会漂。
- 不把技能做成树节点。技能是带目录结构的文件、有正文有脚本，树只有四个字段装不下；
  把别人的 README 导进盘面，盘就不是你的了。
