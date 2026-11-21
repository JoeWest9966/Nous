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

## 表

| module | vendor | crate | layer | 形态 | API 面 | 必需 | 备注 |
|---|---|---|---|---|---|---|---|
| `wire` | nous | `nous-wire` | kernel | 编译期 | `/v1/*` 事件与实体类型 | 是 | 契约层，零依赖优先 |
| `chord` | nous | `nous-chord` | kernel | 编译期 | context / json / delta / sqlite / watch / output | 是 | 语义基元 |
| `durable` | nous | `nous-durable` | kernel | 编译期 | 提交 / 文档 / 会话与 entry 查询 / scans | 是 | 存储与事务边界 |
| `env` | nous | `nous-env` | platform | 编译期 | `FileSystem` + `Shell` 两张 trait + 四条私库路径解析（`content.rs`：#63）+ `GET /v1/fs/dirs` 的目录解析（`NOUS_START_DIRS` 只在这里读） | 是 | 契约；实现见 `env-local` / `env-ssh`。`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` / `NOUS_EXTEND` 一处解析，unit 不写；写回私库归 `skills-admin` / `presets-admin` |
| `repo` | nous | `nous-repo` | platform | 编译期 | 四套私库的文件 CRUD + git 发布 / 归档三代 / pending / diff / history（#75）+ 技能/扩展变体 worktree + 绑定表 + prune/publish/usage（#76）+ 上游追踪（sources / mirror / check / pin / sync / tree / file / copy / adopt；#77） | 是 | 唯一的 git 进程调用面；四条私库路径解析沿用 `env` |
| `env-local` | nous | (待建) `nous-env-local` | platform | 编译期 | `ExecutionEnv` 本地实现 | 是 | 今天的 `os_env.rs` 落点 |
| `env-ssh` | nous | `nous-env-ssh` | platform | 编译期 | `ExecutionEnv` 远端实现（ssh/scp + ControlMaster + 临时密钥） | 否 | 今天 `ssh-env.js`（631 行）；#78 落地 |
| `ai-types` | nous | `nous-ai-types` | kernel | 编译期 | TSchema / Message / Usage（纯） | 是 | 从 `nous-ai` 拆出，零 IO 零时钟（T2 已落地） |
| `ai` | nous | `nous-ai` | platform | 编译期 | provider trait + HTTP/SSE 实现 | 是 | 今天一个包兼纯/IO 两半；纯的那半已拆成 `ai-types` |
| `ai-providers` | nous | (待建) `nous-ai-providers` | platform | 编译期 | provider 实现（拆完只留 IO 那半） | 是 | T2 拆完由 `ai` 改出：目录改名 `ai` → `ai-providers`、包改名 `nous-ai` → `nous-ai-providers` |
| `tools` | nous | `nous-tools` | platform | 编译期 | read / write / edit / bash / grep / find / ls | 否 | 第一方工具，唯一进程创建面 |
| `runtime` | nous | `nous-runtime` | platform | 编译期 | Host 配置 / 认证 / 执行绑定 | 是 | |
| `telemetry` | nous | `nous-telemetry` | platform | 编译期 | span / event 契约，默认 noop | 否 | |
| `store` | nous | `nous-store` | platform | 编译期 | `open` / `backup_to` / `schema_version`，`triggers` / `fires` / `reclaim` | 否 | Nous 自己的库 `nous.db`（第二个数据面入口）：独立迁移链，**不依赖 `nous-durable`**（#56） |
| `device` | nous | `nous-device` | app | 编译期 | 机器编目 / 模型编目 / 默认模型 | 是 | #65：从 server 提成独立插件；`/v1/models` 文件+内联来源，`PUT /v1/default` 原子写 |
| `sessions` | nous | `nous-sessions` | app | 编译期 | 起轮次 / 取消 / 终态五值 | 是 | **pi 本体**（#44 落地），承载最小轮次与取消收敛 |
| `documents` | nous | `nous-documents` | app | 编译期 | `DocumentKind` + 泛型文档端点 + 路由注册表 | 是 | #48：一个实现生成 presets / prompts 两套端点 |
| `prompts` | nous | `nous-prompts` | app | 编译期 | 提示词目录只读扫描 + `render()` 纯函数（不碰盘不碰网不看时钟） | 是 | 今天 `prompts/index.js` |
| `ext` | nous | `nous-ext` | app | 编译期 | 扩展装载 / 帧协议 / 进程监管 / 熔断 / 热加载 / 热重载 / 真清单 | 否 | 已落地（#17–#37，#81 解冻：真清单 / 熔断 / 热重载）；`ext-host` 行是它后续的重划 |
| `archive` | nous | — | app | 编译期 | `archive` / `purge` / `archive_list` / `restore` | 否 | 面板时代归档端点；Rust 版只落地了真删 + 墓碑，完整四方法另票 |
| `ext-probe` | nous | `nous-ext-probe` | dev | 编译期 | 起一个扩展、逐帧验协议 | 否 | 探针 binary（`dev`：不进 systemd、不进发布包） |
| `ext-sdk` | nous | `nous-ext-sdk` | platform | 编译期 | 扩展侧 SDK（`exts/*` 用它编） | 否 | 契约层；扩展 crate 唯一的外部依赖 |
| `ext-host` | nous | (待建) `nous-ext-host` | app | 编译期 | 帧协议 + 进程监管 + 熔断 + 热加载 | 是 | slice 1（`#20`） |
| `server` | nous | `nous-server` | app | 编译期 | `/v1/*` 端点 + SSE | 是 | 唯一监听处；bin 叫 `nous-app` |
| `fixture` | nous | `nous-fixture` | dev | 编译期 | fixture 库 → `/v1/sessions` 同形 JSON | 否 | 测试夹具 binary（`dev`：不进 systemd、不进发布包） |
| `audit` | nous | (待建) `nous-audit` | app | 编译期 | 事件追加与投影 | 否 | 今天 `activity.js`（55 行） |
| `dispatcher` | nous | (待建) `nous-dispatcher` | app | 编译期 | 调度节拍（trigger → 会话） | 否 | 内核算核心三件之一，见下注 |
| `exts/` | nous | — | app | 运行时外挂 | 帧协议（stdio） | 否 | 目录 `exts/<名>/`，可热加载 |
| `triggers` | nous | — | app | 运行时外挂 | `GET /v1/triggers` + `POST /v1/triggers/{code}/fire`（#57：定义与账本在 `nous-store`，点火走同一个 Session）+ `GET /v1/triggers/{code}/fires`（#58：账本可读，counts 从库现算，CLI `--triggers` 同一读口） | 否 | #57 落地；调度循环不在本票 |
| `archive` | nous | — | app | 数据目录 + 端点族 | `POST /v1/sessions/:id/archive` · `GET /v1/archive` · `POST /v1/archive/:id/restore`（#67：闭包复印进 `archives/gen-YYYYMMDD.db`，墓碑住 `qrd3wh.sessions`；purge 走 sessions） | 是 | 独立 SQLite 文件（选项甲，见 docs/decisions.md）；不进 durable 迁移链；目录相对 `NOUS_DB` |
| `stats` | nous | `nous-stats` | app | 编译期 | `GET /v1/stats/usage` + `GET /v1/stats/tasks` + `GET /v1/stats/db`（#62：只读聚合，不建表、不写库） | 否 | 归口监控六屏；与 `--check` 共读口，tasks 现算 |
| `build-worker` | nous | — | app | 运行时外挂 | 待定（编译派发） | 否 | 将来 |
| `presets` | nous | — | app | 编译期 | 模板 / 终极模板名册 + `/v1/presets`（#60：13 条资源化端点 + 修订表） | 否 | 数据根 `NOUS_PRESETS=/opt/Nous-Template`（#63，#60 端点复用；路径解析归 `env`）；Rust 版名册是否保留待判 |
| `combos` | nous | — | app | 编译期 | `GET /v1/combos/:section` + `PATCH /v1/combos/:section/:name`（#61：名册扫 `NOUS_PRESETS` 目录，enabled 由引擎算，写穿 `spec.enabled`，If-Match + requestId 幂等） | 否 | 数据根 `NOUS_PRESETS`（与 `presets` 同根）；触发器存在性从 `NOUS_TRIGGER` 目录读 |
| `skills` | nous | — | app | 声明式 | skill 目录（提供） | 否 | 数据；默认根 `NOUS_SKILLS=/opt/Nous-skills`（#63，路径解析归 `env`）；管理面见下一行 |
| `presets` | nous | — | app | 声明式 | 模板 / 终极模板名册 | 否 | 数据根 `NOUS_PRESETS=/opt/Nous-Template`（#63，#60 端点复用；路径解析归 `env`）；Rust 版名册是否保留待判 |
| `skills` | nous | `nous-skills` | app | 声明式 | `SKILL.md` frontmatter / 附件 / 类别词表解析 + `/v1/skills` 只读投影（#71） | 否 | 数据；默认根 `NOUS_SKILLS=/opt/Nous-skills`（#63，路径解析归 `env`）；只列盘上真相，不碰 git；管理面见下一行 |
| `prompt-text` | nous | — | app | 声明式 | `*.md` 正文 | 是 | 数据 |
| `trigger-spec` | nous | — | app | 声明式 | `nous.json` 参数声明 + `index.json` | 否 | 数据 |
| `skills-admin` | nous | (待建) `nous-skills-admin` | app | 声明式 + 管理 API | skill 仓库管理（装/卸/variant） | 否 | 今天 `skills-api.js` 1,604 + `skills-variants.js` 371 |
| `presets-admin` | nous | (待建) `nous-presets-admin` | app | 声明式 + 管理 API | 模板管理 | 否 | 今天 `presets-api.js` 947 + `presets.js` 750 |
| `trigger-catalog` | nous | — | app | 运行时外挂 | 触发器清单（列/改/发布） | 否 | 今天 `n8qkkj` 块 |

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

今天 `crates/nous/` 下 18 个目录，本表里 `crate` 列不是 `(待建)`、也不是 `—` 的正好 18 行：
`wire` `chord` `durable` `env` `ai` `ai-types` `tools` `runtime` `telemetry` `documents`
`store` `ext` `ext-probe` `ext-sdk` `sessions` `server` `stats` `fixture`。新增模块 → 先加一行；删模块 → 先删一行。

（`ai` 今天住着 `nous-ai` 一个包：目录不叫 `ai-providers`，是因为 spec 判据 6b 用
`cargo tree -p nous-durable --depth 2 | grep providers`，路径里带 `providers` 会让它永远假红。
T2 拆完再改成 `ai-providers`，那时包里只有 IO 那半，判据按包名判。）
