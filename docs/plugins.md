# 插件清单（唯一的分层真相）

> 本文件是**分层与插件归属的唯一真相**。目录只表达所有权（`crates/<vendor>/<module>`），
> **层（layer）只在这里**。判据：`ls -d crates/*/*/` 的模块集合 == 本文件的模块集合
> （双向逐行比对，多一个 = 红，少一个 = 红）。

## 规则

1. **归位制**：新增任何东西（crate、扩展、数据目录、管理面），先在本文件加一行，再落代码。
   清单里没有的模块目录 = 红；清单里写了但目录不存在 = 红（`待建` 行除外，见第 4 列）。
2. **`layer` 四值**：`kernel`（真相层，零进程零网络零直接时钟）· `platform`（能力层，允许 IO
   与进程，但必须有"无 IO 替身"）· `app`（应用层，唯一 spawn 处、唯一监听处）· `dev`（见下）。
   > `dev` = 测试夹具与探针，**不进任何产品产物**（不进 systemd、不进发布包）。
   > 判据里凡是"排掉探针/夹具"的地方，一律改成"减 `dev` 层"，不许点名目录。
3. **依赖判据**：`layer = kernel` 的 crate，依赖闭包不许出现 `layer = platform` 或 `layer = app`
   的 crate（`cargo metadata` 可查）。
4. **`crate` 列的三种写法**：
   - `nous-durable` —— 已存在且包名就是这个（目录改名不改包名）
   - `(待建) nous-sessions` —— 还没建，目录不存在**不算红**
   - `—` —— 不是 crate（声明式 / 运行时外挂）
5. **`必需`= 是** 的模块缺失时，发行版**拒绝启动并指名**缺失项（不许"少一块还能跑"）。
6. **路径选择子**：`scripts/layer-paths.sh <layer>` 只读本表的 `module` / `vendor` /
   `crate` / `layer` 四列，输出 `crates/<vendor>/<module>`（空格分隔），跳过 `crate` 列为
   `(待建) …` 或 `—` 的行。**脚本里不写死任何模块名。**
7. **每个插件一份人话 README**：`crates/<vendor>/<module>/README.md` 必须有，**中文 300 字
   以内**（按汉字计数），用大白话说清三件事 —— 它是什么 / 什么时候用它 / 它不负责什么。
   不写 API 表格、不写内部实现、不复制 `plugin.toml` 的 `description`（那是列表行用的一行摘要）。
   模板见 `docs/plugin-md-template.md`，判据见 `scripts/check-plugins.sh` 的
   `README_MISSING` / `README_TOO_LONG`。桌面端「关于 → 插件」直接读这份正文（随产物发布到
   `<产物目录>/docs/plugins/<module>.md`，换文档不用重编译）。

8. **插件版本独立**：`plugin.toml` 的 `version` 是**这个插件自己的** semver，不再等于
   workspace / `Cargo.toml` 的版本（那是发行版号，跟着 `chore(release)` 走）。Magento 2 的模块
   版本也是各走各的，根仓不压一个数。每个插件的当前版本是**按仓库痕迹推出来的**，见
   `docs/plugin-versions.md`（表 + 口径 + 每个 minor 档的来处），之后各自涨。
   `/v1/capabilities` 的 `modules[].version` 报的就是这个数（`server/build.rs` 从
   `plugin.toml` 生成静态表，不手写第二份；取不到才回退发行版号并带 `versionSource: "release"`）。
   **跨度要求**（一张票动到几件，按最大的那条涨一次，别连涨三档）：

   | 动了什么 | 涨到 |
   |---|---|
   | 端点契约（新增 / 改形状 / 改状态码 / 改字段名）、`capability` 或 `layer` 改名、删端点 | `0.Y+1.0`（minor） |
   | 只改内部实现 / 日志 / 性能 / 错误文案 / README / 测试 | `0.Y.Z+1`（patch） |

   `0.x` 阶段允许 minor 带破坏性变更，但**必须在票面与 `decisions.md` 里写出破的是谁**。
   判据只守两条机械规则（`scripts/check-plugins.sh`）：合法 `X.Y.Z`、不比我 `git show HEAD:`
   里的低（文件还没入库就不判）。**「该涨没涨」脚本不猜语义**，由 review 对着上表看；
   所以「只改 README」也要 +patch，除非这一票明确写了「不动版本」。

9. **模块名只有一处真相：英文模块名。** 客户端（NousDesk）不存任何「模块名 → 中文名」的
   对照表 —— 名字是什么就显示什么，`/v1/capabilities` 报了新模块，客户端列表自动多一行，
   **加插件不用改客户端**。中文只准活在**模块自己的 `README.md`**（规则 7）与
   `docs/plugins.md` 的描述列里；这两处是散文，不是名字的第二份拷贝。
   Magento 2 同理：模块名永远是 `Magento_Catalog`，中文在 `i18n/zh_Hans.csv`，不在代码里另起名字。
   判据：客户端仓 `grep` 不到任何模块名常量（NousDesk 侧那条由它自己的测试守）。

10. **`vendor` 是出品方（Magento 2 语义），不是功能分类、也不是"全仓一律 nous"。**
    Magento 里 `Magento_Catalog` → composer 包 `magento/module-catalog`、`Amasty_Shiprestriction`
    → `amasty/module-shiprestriction`：**同一个 Magento 装里可以有多家 vendor 并列**，每家自己的
    模块用自己的前缀，vendor 就是那家出品方。我们照抄这条，包名用扁平形（不写 `module-`）：

    - 目录：`crates/<vendor>/<module>`
    - 清单行、`plugin.toml` 的 `vendor` / `name` 与目录一致
    - **包名**（`Cargo.toml` 的 `[package] name`）= `<vendor>-<module>`，全小写连字符：
      `nous-stats`、`data-data-manager`

    所以「换出品方」不是改一行文档：目录挪到 `crates/<新 vendor>/`、包名跟着换前缀、
    `docs/dependency-graph.toml` 与 `docs/harness.md` §7 的边名一起改（这些是票面要写清的）。
    现状四家：`Nous`（多数）、`Data`（`data-manager` / `archive`）、`Element`（`Models`）、
    `Trigger`（`Trigger_Core`）。后三家是「同一台装里另一个出品方」的真用例 ——
    `Trigger_Core` 就是照这条整份换供应商的（目录、包名、全名、capability 一起动，
    URL 与私库键一个字没动），详见 `docs/decisions.md` 2026-11-26 那条。
    判据：`scripts/check-plugins.sh` 逐条比对（目录 / 清单行 / `plugin.toml` / 包名四处同源）。

11. **端点只有一个真相：插件的 `routes()`。** `plugin.toml` **没有** `endpoints` 字段
    （曾经有个永远为 `[]` 的栏，看着像"这个插件没有端点"）。端点的三处读口都由代码声明生成：
    `GET /v1/capabilities` 的 `modules[].endpoints`、`GET /v1/_routes`、`GET /v1/openapi.json`
    —— 同一份 `routes()`，不可能分叉。清单只管**身份**（name / vendor / version / layer / crate /
    requires），不管"我有哪些 URL"。
    判据：`scripts/check-plugins.sh` 见到 `provides.endpoints` 就红（`endpoints 已作废`）。

12. **三套写法，各归各的世界** —— 照 Magento 2 的 `app/code/Amasty/Shiprestriction` + `module.xml`
    + composer 包名三件套：

    | 概念 | 形态 | 例 |
    |---|---|---|
    | 模块全名（`plugin.toml` 的 `name`、`/v1/capabilities` 的 `modules[].name`、`GET /v1/plugins/:name/*`） | `<Vendor>_<Module>`，`_` 分段，每段首字母大写 | `Amasty_Shiprestriction` / `Nous_PresetsAdmin` |
    | vendor（出品方，规则 10；`plugin.toml` 的 `vendor`、`/v1/capabilities` 的 `modules[].vendor`、目录名） | **PascalCase**，等于全名第一段 | `Amasty` / `Nous` / `Element` / `Data` |
    | 目录 | `crates/<Vendor>/<Module>`，Module = 全名去掉 vendor 段、按大写字母断词后**每段首字母大写** | `crates/Amasty/Shiprestriction` / `crates/Nous/PresetsAdmin` |
    | 包名（`Cargo.toml` 的 `name`） | `<vendor 小写>-<module 小写连字符>`：cargo 的世界全小写 | `amasty-shiprestriction` / `nous-presets-admin` |
    | capability（`provides.capability`） | 同「小写连字符模块名」，判据与机器比对用 | `shiprestriction` / `presets-admin` |

    四句话记住：
    ① **全名与 vendor 首字母大写**（命名空间的样子，`Nous_PresetsAdmin` / `Nous`）；
    ② **目录跟着 PascalCase 走**（`crates/Nous/PresetsAdmin`，与 Magento 的 `app/code/...` 同构）；
    ③ **包名与 capability 全小写连字符**（cargo 与机器名只认这个，`nous-presets-admin` / `presets-admin`）；
    ④ 三者**由全名推出来，不另存第二份**：`check-plugins.sh` 的 `_pascal` / `_kebab`、
    `layer-paths.sh` 的 `dir_of`、`plugin-versions.rs` 的 `dir_of` 是同一算法的三份实现，改一处必须改全部。

    **断词规则**：尾段是 CamelCase 或 kebab，都先按大写字母/连字符拆成词，再拼 ——
    `PresetsAdmin` / `presets-admin` → 目录 `PresetsAdmin`、包名 `presets-admin`。只 `.lower()`
    会把 `presets-admin` 推成 `presetsadmin`，跟仓里目录对不上（这条踩过）。

    「vendor 永远是 B 形态」：**vendor 是目录与包名的一部分，模块由 vendor 独立命名，包名 `vendor-module`**
    —— 不存在「vendor 只活在字段里、目录平铺」的 A 形态。这句话是一次性决定，别再翻案。
    老模块的小写连字符全名（`sessions`、`env`、`repo`）继续有效：无 `_` 时目录名 = 全名断词后
    PascalCase（`Sessions` / `Env`）、包名 = `<vendor 小写>-<全名>`，**逐个改成全名**，
    不做一次性改名。
    判据：`scripts/check-plugins.sh` 由全名**推出**目录名 / 包名 / capability，再与磁盘、清单、
    `docs/plugins.md` 三处逐条比对；`vendor` 必须等于目录名（PascalCase），且全名第一段 == vendor 的小写。

## 表

| module | vendor | crate | layer | 形态 | API 面 | 必需 | 备注 |
|---|---|---|---|---|---|---|---|
| `wire` | Nous | `nous-wire` | kernel | 编译期 | `/v1/*` 事件与实体类型 | 是 | 契约层，零依赖优先 |
| `chord` | Nous | `nous-chord` | kernel | 编译期 | context / json / delta / sqlite / watch / output | 是 | 语义基元 |
| `durable` | Nous | `nous-durable` | kernel | 编译期 | 提交 / 文档 / 会话与 entry 查询 / scans | 是 | 存储与事务边界 |
| `env` | Nous | `nous-env` | platform | 编译期 | `FileSystem` + `Shell` 两张 trait + 四条私库路径解析（`content.rs`：#63）+ **自带 `GET /v1/fs/dirs`**（`routes.rs`；`NOUS_START_DIRS` 只在这里读，远端分支是宿主能力 trait） | 是 | 契约；实现见 `env-local` / `env-ssh`。`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` / `NOUS_EXTEND` 一处解析，unit 不写；写回私库归 `skills-admin` / `presets-admin`。自带 `GET /v1/fs/dirs`（远端分支是宿主能力 trait：`nous-env-ssh` 的 requires 里有本 crate，反向依赖会成环） |
| `repo` | Nous | `nous-repo` | platform | 编译期 | 六套私库的文件 CRUD + git 发布 / 归档三代 / pending / diff / history（#75）+ 技能/扩展变体 worktree + 绑定表 + prune/publish/usage（#76）+ 上游追踪（sources / mirror / check / pin / sync / tree / file / copy / adopt；#77）；`/v1/repos/*` 三十条端点自带 handler（`routes.rs`，宿主实现 `Host`） | 是 | 唯一的 git 进程调用面；六条私库路径解析沿用 `env` |
| `env-local` | Nous | (待建) `nous-env-local` | platform | 编译期 | `ExecutionEnv` 本地实现 | 是 | 今天的 `os_env.rs` 落点 |
| `env-ssh` | Nous | `nous-env-ssh` | platform | 编译期 | `ExecutionEnv` 远端实现（ssh/scp + ControlMaster + 临时密钥） | 否 | 今天 `ssh-env.js`（631 行）；#78 落地 |
| `ai-types` | Nous | `nous-ai-types` | kernel | 编译期 | TSchema / Message / Usage（纯） | 是 | 从 `nous-ai` 拆出，零 IO 零时钟（T2 已落地） |
| `ai` | Nous | `nous-ai` | platform | 编译期 | provider trait + HTTP/SSE 实现 | 是 | 今天一个包兼纯/IO 两半；纯的那半已拆成 `ai-types` |
| `ai-providers` | Nous | (待建) `nous-ai-providers` | platform | 编译期 | provider 实现（拆完只留 IO 那半） | 是 | T2 拆完由 `ai` 改出：目录改名 `ai` → `ai-providers`、包改名 `nous-ai` → `nous-ai-providers` |
| `tools` | Nous | `nous-tools` | platform | 编译期 | read / write / edit / bash / grep / find / ls | 否 | 第一方工具，唯一进程创建面 |
| `runtime` | Nous | `nous-runtime` | platform | 编译期 | Host 配置 / 认证 / 执行绑定 | 是 | |
| `telemetry` | Nous | `nous-telemetry` | platform | 编译期 | span / event 契约，默认 noop | 否 | |
| `store` | Nous | `nous-store` | platform | 编译期 | `open` / `backup_to` / `schema_version`，`triggers` / `fires` / `reclaim` | 否 | Nous 自己的库 `nous.db`（第二个数据面入口）：独立迁移链，**不依赖 `nous-durable`**（#56） |
| `Device` | Nous | `nous-device` | app | 编译期 | 机器编目 / 模型编目 / 默认模型 / 自省 | 是 | #65：从 server 提成独立插件；自带 9 条端点（机器 / 模型三件套 / `device` / `_routes` / `capabilities` / `openapi.json` / 插件 README）；自省三件套的**原料归宿主、视图归插件** |
| `sessions` | Nous | `nous-sessions` | app | 编译期 | 起轮次 / 取消 / 终态五值 | 是 | **pi 本体**（#44 落地）：最小轮次与取消收敛 + 会话读投影（`source`）/ 写命令（`commands`）/ 请求形状（`records`）/ 事件流（`hub` / `coalesce` / `project` / `observer`）；`/v1/sessions*` 十七条里 十六条自带 handler，`messages` 留在宿主侧（要组装根的 driver / completions 与提示词注入） |
| `documents` | Nous | `nous-documents` | app | 编译期 | —（frozen：不注册端点） | 否 | #90：v1 下线，内容真相只有四个私库；保留行记历史 |
| `Nous_Prompts` | Nous | `nous-prompts` | app | 编译期 | 6 条端点自带 handler（`crates/Nous/Prompts/src/routes.rs`）：`GET /v1/prompts` 只读投影 + `files`/`file`/`create`/`publish` 编辑面；盘面规矩在 `nous-repo`，状态码翻译在 `nous-routes` | 是 | 默认根 `NOUS_PROMPTS=/opt/Nous-prompts`（#72，路径解析归 `env`）；`PROMPT.md` + `nous.json` + `settings.json` 的只读扫描与纯 `render()` 仍在 `lib.rs`；盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，与 `Skill` / `Extend` 共用同一份；`RepoError → 状态码` 的翻译在 `nous_repo::Refusal`（HTTP 回包壳在 `nous_routes::HttpRefusal`）；自带 6 条 `/v1/prompts*` 端点 |
| `Extend` | Library | `library-extend` | app | 编译期 | 扩展装载 / 帧协议 / 进程监管 / 熔断 / 热加载 / 热重载 / 真清单 + 15 条端点自带 handler（`crates/Library/Extend/src/routes.rs`）：清单 / 重载 / 管理面 9 条（create / duplicate / install / uninstall / categories / category / trash）+ 编辑页 4 条（`files` / `file` / `publish`，盘面规矩与 git 提交复用 `nous-repo`） | 否 | 已落地（#17–#37，#81 解冻：真清单 / 熔断 / 热重载，#84 补管理面）；自带 15 条 `/v1/extensions*` 端点（扩展管理面 + 编辑页 4 条走 `nous_repo::entries`）；`ext-host` 行是它后续的重划 |
| `ext-probe` | Nous | `nous-ext-probe` | dev | 编译期 | 起一个扩展、逐帧验协议 | 否 | 探针 binary（`dev`：不进 systemd、不进发布包） |
| `ext-sdk` | Nous | `nous-ext-sdk` | platform | 编译期 | 扩展侧 SDK（`exts/*` 用它编） | 否 | 契约层；扩展 crate 唯一的外部依赖 |
| `extend-trash` | Nous | — | app | 数据目录 | 卸载源码的垃圾站 + `deleted-extensions.json` 删除名单（#84） | 否 | 位于 `data/extend-trash/`，install 的回退来源；名单随私库发布，防 git pull 复活 |
| `ext-host` | Nous | (待建) `nous-ext-host` | app | 编译期 | 帧协议 + 进程监管 + 熔断 + 热加载 | 是 | slice 1（`#20`） |
| `server` | Nous | `nous-server` | app | 编译期 | `/v1/*` 端点 + SSE | 是 | 唯一监听处；bin 叫 `nous-app` |
| `fixture` | Nous | `nous-fixture` | dev | 编译期 | fixture 库 → `/v1/sessions` 同形 JSON | 否 | 测试夹具 binary（`dev`：不进 systemd、不进发布包） |
| `audit` | Nous | (待建) `nous-audit` | app | 编译期 | 事件追加与投影 | 否 | 今天 `activity.js`（55 行） |
| `dispatcher` | Nous | `nous-dispatcher` | app | 编译期 | 注入 Clock + durable `sleeping` 任务的 60s 节拍、纯判定 `evaluate`、忙则跳过、失败隔离（trigger → 会话） | 否 | #73 落地；内核算核心三件之一，见下注 |
| `exts/` | Nous | — | app | 运行时外挂 | 帧协议（stdio） | 否 | 目录 `exts/<名>/`，可热加载 |
| `Trigger_Core` | Trigger | `trigger-core` | app | 编译期 | **定义面** 11 条自带 handler（`crates/Trigger/Core/src/catalog.rs`）：`GET /v1/triggers`（观测 + 调度健康，CLI `--triggers` 同一读口）、`POST /v1/triggers`、`PATCH / PUT /v1/triggers/{code}`、`POST /v1/triggers/{code}/{archive,restore,purge,fire}`、`GET /v1/triggers/{code}/fires`、`GET /v1/triggers/categories`、`POST /v1/triggers/category`；**工程面** 13 条（`src/projects.rs`）：`/v1/trigger-projects*` 的清单 / 文件面 / 新建 / 发布（cargo check → build → index.json → 提交）/ 归档三代 / 类别词表 / sync | 否 | #57/#58/#73/#80 落地；插件出生即全名（规则 12）；旧的 `triggers` / `trigger-catalog` 两行并入本行 |
| `elements` | Nous | `nous-elements` | app | 编译期 | `GET /v1/elements`（两段全量 + 引用解析）+ `GET/PUT/DELETE /v1/elements/{kind}/{id}`（单条读写 / 删）+ `POST /v1/elements/{kind}`（新建）+ `POST /v1/elements/{kind}/{id}/lock`（上锁 / 解锁）+ `POST /v1/elements/push`（手动补推）；写一次提交一次，同一个 `requestId` 重放不二次提交 | 否 | 元素库（组 / 包）：数据根 `NOUS_ELEMENTS=/opt/Nous-Elements`，一份 JSON 一个元素；8 条端点自带 handler（`crates/Nous/Elements/src/routes.rs`） |
| `Models` | Element | `element-models` | app | 编译期 | 元素库「模型入口组」→ 编目投影：组（`element-models`）+ 供应商包（`kind:"provider"`，`models[]`）+ 覆盖包（`kind:"model"`，按 `(provider, modelId)` 逐键合并）；`GET /v1/models/elements`（投影 + 诊断，缺包 / 悬空覆盖 / 明文密钥都如实报）；密钥只解析成形状（`env` / `cmd` / `secret` / `plaintext` / `missing`），值不进响应 | 否 | 盘上真相在 `nous-elements`；解析 / 合并 / 投影是纯函数（不读盘、不联网、不看时钟） |
| `routes` | Nous | `nous-routes` | platform | 编译期 | 插件自带端点的契约：`Route`（一条声明同时产出「装进 Router」与「注册表登记」）/ `Declaration`（`:id` 形式的注册表视图） | 否 | 端点归属的落点：`server` 只做「收集 → 装配 → 分发」，插件交出自己的 `routes()`；`plugin.toml` 不再有 `endpoints` 字段（规则 11：端点只由 `routes()` 声明，三处读口都从它生成） |
| `Data_Archive` | Data | `data-archive` | app | 编译期 | 会话归档：`POST /v1/sessions/:id/archive` · `GET /v1/archive` · `POST /v1/archive/:id/restore`（#67：闭包复印进 `archives/gen-YYYYMMDD.db`，墓碑住 `qrd3wh.sessions`；purge 走 sessions） | 否 | 三条端点自带 handler（`crates/Data/Archive/src/routes.rs`）；供应商 `data`，包名 `data-archive` |
| `Data_DataManager` | Data | `data-data-manager` | app | 编译期 | `GET /v1/stats/usage` + `GET /v1/stats/tasks` + `GET /v1/stats/db`（#62：只读聚合，不建表、不写库）+ 六屏明细 `overview` / `conversations` / `conversation/:id` / `cost` / `health`（数据监控室复原：dbstat 真页数 / prompt 重建 / usage 分项 + byHour / 健康明细） | 否 | 归口监控六屏；与 `--check` 共读口，tasks 现算 |
| `build-worker` | Nous | — | app | 运行时外挂 | 待定（编译派发） | 否 | 将来 |
| `presets` | Nous | — | app | 编译期 | 模板 / 终极模板名册 + `/v1/presets`（#60：端点面 + 修订表；判定归 `presets-admin`） | 否 | 数据根 `NOUS_PRESETS=/opt/Nous-Template`（#63；段清单读 `<root>/sections.json`，路径解析归 `env`） |
| `combos` | Nous | `nous-combos` | app | 编译期 | `GET /v1/combos/:section` + `PATCH /v1/combos/:section/:name`（#61：名册扫 `NOUS_PRESETS` 目录，enabled 由引擎算，写穿 `spec.enabled`，If-Match + requestId 幂等） + `POST /v1/combos/:section/:name/fire` + `POST /v1/presets/Ultimate-Template/launch`（#82：试跑复用 #73 的 evaluate，写 #74 血缘）；**四条端点自带 handler**（`routes.rs`） | 否 | 数据根 `NOUS_PRESETS`（与 `presets` 同根）；触发器存在性从 `NOUS_TRIGGER` 目录读。宿主能力四件事：两个盘面根 / 触发器判定视图 / 注入时钟 / 起会话 + 写血缘 |
| `Skill` | Library | `library-skill` | app | 编译期 | 6 条端点自带 handler（`crates/Library/Skill/src/routes.rs`）：`GET /v1/skills`（只读投影，`?usage=1` 反查谁在用）+ `/v1/skills/{files,file,create,publish}`（编辑页） | 否 | 默认根 `NOUS_SKILLS=/opt/Nous-skills`（#63，路径解析归 `env`）；`SKILL.md` frontmatter / 附件 / 类别词表解析仍在 `lib.rs`；盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，与 `prompts` / `Extend` 共用同一份；不 spawn git（发布走 `nous-repo`） |
| `prompt-text` | Nous | — | app | 声明式 | `*.md` 正文 | 是 | 数据 |
| `trigger-spec` | Nous | — | app | 声明式 | `nous.json` 参数声明 + `index.json` | 否 | 数据 |
| `skills-admin` | Nous | (待建) `nous-skills-admin` | app | 声明式 + 管理 API | skill 仓库管理（装/卸/variant） | 否 | 今天 `skills-api.js` 1,604 + `skills-variants.js` 371 |
| `presets-admin` | Nous | `nous-presets-admin` | app | 编译期 | 模板管理：形状闸 / 引用解析（`resolved`）/ 名册投影 + **自己的 32 条 `/v1/presets*` 端点**（`routes.rs`；13 条归 `presets`、19 条归 `presets-admin`，照搬家前静态表原样） | 否 | 从 TS 内核 `presets.js` 750 + `presets-api.js` 947 归位；段清单读 `<root>/sections.json` |

## 待判（还没归位，归位前不许进仓）

| 东西 | 疑点 | 倾向 |
|---|---|---|
| `locks.ts` | 文件锁——坑里记过"不许移植进 Rust" | 删掉，用 durable 的提交序列代替 |
| `tree.ts` / `docname.ts` | 内容树：数据访问还是工具？ | 并入 `server` 的投影层，不单独成 crate |
| `archive.js`（443） | 归档是维护动作还是插件？ | 有独立入口（命令/端点）= 插件；否则工具函数 |
| `upstream.js`（672） | 上游追踪 | #77 归入 `repo` 插件（镜像在 data/，订阅清单在技能私库） |
| `unread.js`（111） | 未读投影 | 并入 `server` 投影层 |
| `catalog.js`（225） | 编目 | 机器编目 → `dispatcher` 或 `server` |
| `maintenance.js`（75） | 维护动作 | 工具函数 |

## 不是插件

契约层（`nous-wire` 内的类型）· 装载器（`nous-ext` 里的装载部分）· `nous-fixture` ·
`scripts/verify.sh` · `scripts/judge.sh` · **客户端**（NousDesk、将来的手机端——它们是消费者，
只依赖协议版本）

## 注：`dispatcher` 的层归属

「调度」是内核三件之一（契约 / 装载器 / 调度）。但 `dispatcher` 作为 crate 标 `app`，
因为它的**实现**要调用会话引擎（`sessions`）。**内核三件里的"调度"指的是契约与节拍原语
（`dispatcher` 的 trait），不是它的实现**——那个 trait 落在 `wire` 或独立 `kernel-sched`，
`dispatcher` 只是它的默认实现。

## 注：与 `ls -d crates/*/*/` 的一一对应

今天 `crates/*/*` 下 29 个目录（`crates/Nous/` 24 个 + `crates/Library/` 2 个 + `crates/Data/` 2 个 + `crates/Element/` 1 个），
本表里 `crate` 列不是 `(待建)`、也不是 `—` 的正好 29 行：
`wire` `chord` `durable` `env` `ai` `ai-types` `tools` `runtime` `telemetry`
`store` `Extend` `ext-probe` `ext-sdk` `sessions` `server` `fixture` `elements` `env-ssh` `repo`
`routes` `prompts` `presets-admin` `dispatcher` `Device` `Skill` `documents` `Models`
`Data_DataManager` `Data_Archive`。
新增模块 → 先加一行；删模块 → 先删一行。

（`ai` 今天住着 `nous-ai` 一个包：目录不叫 `ai-providers`，是因为 spec 判据 6b 用
`cargo tree -p nous-durable --depth 2 | grep providers`，路径里带 `providers` 会让它永远假红。
T2 拆完再改成 `ai-providers`，那时包里只有 IO 那半，判据按包名判。）
