# 端点归插件（Magento 2 式）迁移记

> 这份文档是**搬家记事本**：目标、接缝、规矩、进度都记在这里。搬一个端点改一次进度表，
> 不另开第二份清单。判据见文末，命令照抄即可跑。

## 1 · 一句话目标

**端点归插件，`server` 只做「监听 + 中间件 + 分发」。** 插件自己声明路由、自己写 handler；
`server` 不认识任何一条业务路径。

## 2 · 为什么要搬

| 现状（搬家前） | 症状 |
|---|---|
| 所有 handler 都在 `crates/Nous/Server/src/`，`routes.rs` 一千多行 | 加一条端点就得改 `server`，插件再小也得排队 |
| 归属靠 `meta::ROUTES` 一张**手写静态表**声明 | 声明与实现是两份东西，只能靠判据对齐；搬/删容易漂 |
| `plugin.toml` 的 `endpoints` 全是空数组 | 清单里读不出某个插件到底提供了什么端点（2026-11-25 已把这个字段整个删掉，见 `docs/plugins.md` 规则 11） |

Magento 2 的做法是：模块自带 controller，`etc/*/routes.xml` 声明路由，框架启动时把各模块的
路由聚成一张表，唯一入口只做 dispatch + 中间件。**我们现在搬的就是这个：模块自带 controller。**

## 3 · 接缝（已经建好的地基）

### 3.1 `nous-routes`：一条声明同时是「注册」和「登记」

```rust
// 插件侧（以 device 为例）
pub fn routes<S>() -> Vec<nous_routes::Route<S>>
where S: Clone + Send + Sync + 'static {
    vec![nous_routes::Route::new("GET", "/v1/machines", "Nous_Device", || get(machines))]
}
```

- `Route::mount` 把 handler 装进 `Router`；`Route::declaration` 供 `capabilities` /
  `/v1/_routes` / `/v1/openapi.json` 读。**两者出自同一处，不可能分叉。**
- 路径写法只有一份：插件写 axum 的 `/v1/x/{id}`，注册表自动折成 `wire.md` 的 `/v1/x/:id`。
- `server` 侧只有两个函数：`plugins::routes()`（收集）与 `plugins::declarations()`（交给注册表）。

### 3.2 宿主能力 trait：插件不依赖 `server`

> 现在有三个能力的样板：`element-models` 的 `Host`（元素库仓库）与 `Secrets`（密钥文件路径），
> 以及 `data-manager` 的 `Host::stats_paths`（两个只读库路径）。
> 新端点需要什么，就在插件里加一个窄 trait，别去依赖 `AppState`。

插件不认识 `AppState`。它**声明自己需要什么**，由 `server` 实现并注入：

```rust
// 插件侧（element-models）
pub trait Host {
    fn elements_repo(&self) -> Option<std::sync::Arc<nous_repo::Repo>>;
}
impl<T: Host + ?Sized> Host for std::sync::Arc<T> { /* 转发 */ }

// 宿主侧（server/src/plugins.rs）
impl element_models::routes::Host for AppState {
    fn elements_repo(&self) -> Option<Arc<nous_repo::Repo>> { self.repo(crate::elements::REPO_KEY) }
}
```

规矩：**trait 定义在插件里，实现写在 `server/src/plugins.rs`。** 能力宁可窄（一个方法一个能力），
不要造 `trait Everything`。

### 3.3 三条规矩

1. **搬一个删一行**：端点搬进插件后，把 `meta::ROUTES` 里那一行删掉（静态表只减不增）。
2. **不许第二份声明**：路径只写在插件的 `routes()` 里，`server` 里不许再出现同一条路径字面量。
3. **插件不许 `use nous_server::…`**：需要宿主能力就加 trait 方法，由 `server` 实现。
   内核层（`wire` / `chord` / `durable` / `ai-types`）仍然不许出现 `axum::` / `.route(`（`verify.sh` 判据 4）。

### 3.4 标杆：一个「搬完」的插件长什么样（照 `data-manager` 抄）

`data-manager`（原 `stats`）是第一个把**整套**走完的插件：8 条端点、一份清单、一个窄能力、
一组端点级判据。后面 153 条按这张清单抄，不再逐个议接缝。**缺一条就不算搬完。**

#### A. 插件侧（`crates/<vendor>/<module>/src/routes.rs`）

- [ ] `pub const MODULE: &str = "<module>";` —— 注册表里的模块名只写在这里（清单里不再有 `endpoints`，规则 11）。
- [ ] `pub trait Host { fn … }`：**一个方法一个能力**，返回 `Option<…>` 表示「宿主没装配」。
- [ ] `impl<T: Host + ?Sized> Host for std::sync::Arc<T>`：路由装进 `Router<Arc<AppState>>` 时不用再写转发。
- [ ] `pub enum Refusal { NotConfigured, BadRequest(String), NotFound(String), Internal(String) }`
      + `impl axum::response::IntoResponse for Refusal`（`{status, {"error": …}}`；
      **必须实现 `IntoResponse`**，否则 `Handler<_>` 不成立，编译器只会说一句听不懂的 `trait bound not satisfied`）。
- [ ] handler 是泛型 `async fn x<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>, …)`
      —— 泛型参数上那串 bound 是 axum 的要求，不是啰嗦。
- [ ] `pub fn routes<H>() -> Vec<Route<H>>`：**路径只写在这里**（axum 的 `{id}` 形式），
      `nous-routes` 自动折成注册表要的 `:id`。
- [ ] 时钟只在端点层取（聚合层零时钟）；只读插件一次写都不做。

#### B. 宿主侧（`crates/Nous/Server/`）

- [ ] `plugins.rs`：`all.extend(<crate>::routes::routes::<Arc<AppState>>());`
- [ ] `plugins.rs`：`impl <crate>::routes::Host for AppState`（**能力实现只写这一处**）。
- [ ] `meta.rs`：把该模块的端点行从静态表**删掉**（只减不增）。
- [ ] 旧 handler 文件整份删除；**不是**插件的东西（例如 `openapi` 读的是注册表，不是统计）搬回 `routes.rs`，别顺手塞进插件。
- [ ] 需要路径/仓库这类死数据的，`AppState` 的字段类型直接换成插件的类型（`data-manager` 就把
      `StatsRuntime` 换成 `routes::Paths`，`with_stats` 的签名跟着变）—— 别留两套同形结构。

#### C. 判据（插件自己 4 条 + 仓库级 3 条）

- [ ] 声明逐字：`routes()` 的 `declared()` 与模块名逐条比对（路径写错、漏一条立刻红）。
- [ ] 未装配 = `409` 且文案是那句熟话（不是空 200，也不是 404）。
- [ ] 参数错 = `400` 且把收到的值回显在 `{error}` 里（`group_by` 那种闭集合尤其要）。
- [ ] 库/资源没建 = **200 + 按 0 报告**（「没建」不许伪装成「坏了」）。
- [ ] `scripts/check-plugins.sh`（清单四处同源 + 版本只增不减）
- [ ] `scripts/verify.sh`（路由归属、数据面、进程创建点、依赖边）
- [ ] `cargo test --workspace`
- [ ] 原端点的服务器级测试改成走插件类型（`stats_endpoints` 的 `StatsRuntime` → `routes::Paths`），
      读静态表的判据改成读 `meta::endpoints()`（静态表 ∪ 插件声明）—— **静态表变薄不该让判据失效**。

#### D. 记账（三处，缺一处下一个人就不知道发生了什么）

- [ ] `docs/plugins.md` 那一行（module / vendor / crate / layer / API 面 / 备注）
- [ ] 本文 §4 进度表：搬走的从「待搬」删除、加到「已搬」；静态表行数同步
- [ ] 本文 §7 操作日志一行：搬了什么、宿主能力叫什么、有什么坑

#### E. 版本与身份

- [ ] `plugin.toml` 只记身份：`name` / `vendor` / `version` / `layer` / `crate` / `requires`
      （**没有 `endpoints`**，规则 11）；`requires` 由 `check-plugins.sh` 对着 `cargo metadata` 核。
- [ ] 版本按规则 8 涨：端点契约变化 → minor；纯内部改动 → patch。宿主能力的增删也算契约。


## 4 · 进度

### 4.1 已搬（60 条）

| 模块 | 端点 | 提交 | 备注 |
|---|---|---|---|
| `device` | `GET /v1/machines` | `feat(routes)` | 第一条试点，证明接缝通；handler 无需宿主状态 |
| `element-models` | `GET /v1/models/elements` | `feat(element-models)` | 第一条**需要宿主注入**的端点；`Host::elements_repo` |
| `element-models` | `PUT /v1/providers/{id}/secret` | `feat(element-models)` | **新端点**（不是搬家）：插件自带 handler，宿主给 `Secrets::secrets_file` |
| `data-manager` | `GET /v1/stats/{usage,tasks,db,overview,conversations,conversation/:id,cost,health}`（8 条） | `refactor(data-manager)` | 第一次「一个插件一次搬 8 条」：`Host::stats_paths` 只给两个库路径；`server/src/stats.rs` 整份删掉，`openapi` 归 `server/routes.rs`（它读的是注册表，不是统计）。**这就是 §3.4 的标杆** |
| `archive` | `POST /v1/sessions/{id}/archive` · `GET /v1/archive` · `POST /v1/archive/{id}/restore`（3 条） | `feat(archive)` | 第二类接缝：**命令面**（`Host::archive_session` / `archive_generations` / `restore_session`），不是库路径 —— 归档要在活库写锁下写 gen 文件再提交墓碑，失败按方向补偿，那套语义在 `nous_durable::session::commands`。顺带把供应商换成 `data`（`crates/Data/Archive`、包名 `data-archive`）。`server/src/routes.rs` 的三条 handler 与 `RestoreBody` 整份删除 |
| `elements` | `GET /v1/elements` · `GET/PUT/DELETE /v1/elements/{kind}/{id}` · `GET/POST /v1/elements/{kind}` · `POST /v1/elements/{kind}/{id}/lock` · `POST /v1/elements/push`（8 条）
| `presets` + `presets-admin` | `/v1/presets*` 全 32 条（13 + 19） | `refactor(presets-admin)` | **第一次「两个模块共用一个插件的 handler」**：`crates/Nous/Server/src/presets.rs`（1902 行）整份搬成 `crates/Nous/PresetsAdmin/src/routes.rs`。宿主能力只有一个方法 `Host::preset_sources() -> Option<Sources>`（模板私库根 + 插件库 + 三个外部名册根 + 编目来源），`Sources` 由 `main` 装配一次、插件只接 —— 路径解析仍然只有 `env` 插件一处。`ServerError` 换成插件自己的 `Refusal`（六个分支一对一，状态码与文案一字没改）；`server` 侧 `presets::SharedStore` / `PresetStore` / `presets::routes()` 全删，`AppState.presets` 的类型直接换成 `routes::Sources`（不留两套同形结构）。**端点归属照搬家前静态表原样**（`GET order` 在 admin、`PUT order` 在 presets 这种手写漂移不夹带修） | | `feat(elements)` | 第一次搬**已经是独立文件**的模块（`server/src/elements.rs` 377 行）：盘上真相与「写一次提交一次」本来就在 `nous-elements`，所以宿主能力只要 `Host::elements_repo()`（一个仓库句柄），比 `data-manager` 还窄。`RepoError → 状态码` 的翻译搬进插件的 `Refusal::from_repo`（一处判，含 `Validation` 的 `{field, problem}` 与 423）。`server/src/elements.rs` 整份删除；`models.rs` 的 `REPO_KEY` 改读 `nous_elements::routes::REPO_KEY` |
| `env` | `GET /v1/fs/dirs`（1 条） | `refactor(env)` | **契约 crate 第一次自带端点**：`crates/Nous/Env` 是 `ExecutionEnv` 契约层，加 `axum` / `nous-routes` 是方向性决定（§3.4 的「模块自带 controller」与「契约层不绑传输」正面撞上）。定的口径是 **Magento 2 怎么切就怎么切**：一个模块 = 一个目录，`Api/` 与 `Controller/` 在同一个模块里，没有「只装接口那半」。宿主能力只有 `Host::remote_subdirs()` 一个方法 —— 远端实现 `nous-env-ssh` 的 `requires` 里有 `nous-env`，直接调会成环，所以只能反过来注入（这也本来就是契约层该有的方向） |
| `skills` | `GET /v1/skills` · `GET /v1/skills/files` · `GET/PUT /v1/skills/file` · `POST /v1/skills/{create,publish}`（6 条） | `feat(skills)` | 第一次「端点住在共用骨架里」的搬家：6 条里 5 条原在 `server/src/resource_admin.rs`（skills / prompts / extensions 三仓共用一副骨架），不能把骨架整体塞给一个插件。**盘面规矩先下沉**到 `nous_repo::entries`（名字 / 路径校验、递归清单、骨架落盘、发布），三个仓共用同一份；然后 `skills` 插件拿走自己那 6 条，`resource_admin.rs` 只剩 prompts / extensions（522 → 309 行）。宿主能力三个方法：`skills_repo` / `skills_root` / `skills_usage`（前两个是同一份私库的两种取法：写面要仓库句柄、只读投影要路径）。`meta::skills()` 与静态表 6 行一并删（143 → 137） |

### 4.2 待搬（104 条 / 9 个模块）

按「搬起来最省事 → 最费事」排的，数字是该模块在静态表里还剩几条
（静态表总数：搬家起点 162 行 → 现在 104 行；插件自带的端点 `Route::new` 共 61 条）。

| 模块 | 存量 | 需要哪些宿主能力（预估） |
|---|---|---|
| `prompts` | 6 | 私库读写（`nous-repo`） |
| `combos` | 4 | 组合段读写 + 试跑（依赖 `sessions` 起轮） |
| `ext` | 15 | 扩展私库 + 运行时（`nous-ext`）+ 管理面 |
| `triggers` | 24 | 触发器库 + 调度（`nous-dispatcher`） |
| `sessions` | 17 | 最厚：读投影 + 写命令 + SSE 出口（`events` 尤其要小心） |
| `repo` | 30 | 最杂：git 面全在这一家（`nous-repo` 已是 platform，宿主只需给根路径） |
| `device` | 7 | 剩下的编目/默认模型/`_routes`/`openapi` 等自省端点，建议**最后搬**（它们读注册表本身） |

建议顺序：`prompts` → `combos` → `ext` → `triggers` → `repo` → `sessions` → `device`。

`_routes` / `capabilities` / `openapi` 三条**不要急着搬**：它们要读整张注册表，
搬进 `device` 之前得先让「注册表」成为宿主能力（`Host::declarations()`），否则会成环。

## 5 · 判据（每搬一个都跑这几条）

```sh
./scripts/check-plugins.sh                                   # 模块/层/版本与 docs/plugins.md 对齐
cargo test -p nous-routes                                    # 契约：折写路径 + 挂载后真能打到 handler
cargo test -p nous-server --test capabilities_shapes \
  --test capabilities_manifest --test data_manager_manifest         # 注册表三处同源 + 多重集一致
cargo test --workspace                                       # 全量
```

额外的人工检查（搬家时逐条确认）：

1. `meta::ROUTES` 里那条**没了**（静态表只减不增）。
2. `plugins::declarations()` 里有那条（`/v1/_routes` 能看到，`module` 字段对）。
3. `server/src` 里再也搜不到那条路径字面量：`grep -rn '"/v1/xxx"' crates/Nous/Server/src`。
4. 插件 `plugin.toml` 的 `[requires]` 补齐了新增的内部依赖（`check-plugins.sh` 会点名）。

## 6 · 已知遗留

- `nous-server` 仍是**唯一监听处**（`app` 层定义），这点不变；搬的是 handler 与声明。
- ~~`plugin.toml` 的 `endpoints` 仍是空数组~~：**已修**——这个字段已删除（规则 11），端点只由
  插件的 `routes()` 声明，`capabilities` / `_routes` / `openapi` 三处都从注册表生成。
  要让它也自动生成，得等 `/v1/_routes` 变成"从注册表导出清单"的生成物 —— 另开票。
- 元素库扫描失败的行为微调：搬家前走 `ServerError`（`repo_error` 映射），搬家后统一
  `500 {"error": "扫元素库失败：…"}`。形态没变（还是 `{error}` 一句话），状态码在
  极端分支上可能从 4xx 变 500。
- 进程创建点：`server` 里原本新的 `!cmd` 密钥解析自己 `Command::new("sh")`，
  已改成调 `nous_tools::os_env::run_capture`（S1 的唯一执行面，判据 6 / 12 的口子）。
  `crates/Nous/Server/src/trigger_projects.rs` 里的 `run_cargo` 是同一类欠账（更早就有），
  本条未动 —— 它要 async 语义，换法另议。
- 注册表顺序：静态表在前、插件声明在后；`capabilities_declares_modules_and_reasons.json`
  快照会随搬家调整端点顺序（内容不变）。

## 7 · 操作日志

| 日期 | 动作 |
|---|---|
| 2026-11-24 | 建 `nous-routes`（契约层）+ `server::plugins`（收集/装配/登记）；`device` 交出 `GET /v1/machines`（静态表删 1 行） |
| 2026-11-26 | **`/v1/presets*` 32 条搬进 `Nous_PresetsAdmin`**（一个插件扛两个模块名：13 条归 `presets`、19 条归 `presets-admin`，照搬家前静态表原样）。宿主能力一个方法 `Host::preset_sources() -> Option<Sources>`；`Sources` 从 `server::presets::Sources::from_env()`（插件自己读环境）改成宿主装配一次（`main`），插件零环境读取。`ServerError` → 插件自己的 `Refusal`（六分支一对一）。三个坑：①注册表口径是 `:param`（`nous_routes::to_registry_path` 折的），静态表那半还写着 `{param}` —— 搬过来的声明是 `:param`，快照与 `presets_openapi` 两边都得归一才比得上；②`check-plugins.sh` 的 `requires` 是「对着 `cargo metadata` 算的」，加依赖必须同步清单；③纯改名 / 纯搬家之后 `plugin-versions.py` 的考古结论会跳（`server` 一路 0.2.9 → 0.3.21），别手改文档表，按提示涨 |
| 2026-11-25 | **目录与 vendor 也改成 PascalCase**（29 个目录 `crates/Nous/PresetsAdmin` / `crates/Data/DataManager` / `crates/Element/Models`，`vendor = "Nous"` / `"Data"` / `"Element"`）。包名、crate 标识、capability、URL 一个字没动。三处推导判据同算法改成 `_pascal`（按 camel hump 断词），`plugin-versions.py` 补「目录改名 = minor」（纯改名提交里 `plugin.toml` 一行没变、`git show` 的 +/- 是空的，靠行判据会落成 patch，16 个模块全落错档）。坑 2：快照里 `modules[].version` 也要跟着插件版本走，`presets-admin` 红了一次 |
| 2026-11-25 | **规则 12 第二个：`device` → `Nous_Device`**（vendor/目录/包名一字不动，只改全名与版本 0.7.0 → 0.8.0）。同时补齐尾段 **CamelCase → kebab** 推导（`Nous_PresetsAdmin` → 目录 `presets-admin`）。三个判据跟着改：`plugin-versions.rs` 的测试原本按小写名硬凑目录，`Nous_Device` 直接被跳过 → 判据空转（`只比对了 9 个模块`）；修成由全名推目录，并补 `rule12_name_and_dir_derivation` 自检。Desk 侧新增 `module_matches`（`Nous_Device` ↔ `device` 都认），否则改名当天「设备」页会被判成未登记而灰掉 |
| 2026-11-25 | **规则 12 落地**：模块全名照 Magento 2 的 `Vendor_Module`（`Element_Models`），
目录与包名按当时口径是小写（后来 2026-11-25 改成 PascalCase 目录 + `vendor = "Element"`）。第一个迁的是 `element-models`：
vendor `Nous` → `Element`、补一条 `MANIFEST`（它自带两条端点却不在清单里，capabilities 看不见）。
判据跟着改：`check-plugins.sh` 由全名**推出**目录 / 包名 / vendor；`layer-paths.sh` 与
`plugin-versions.py` 用同一算法；`deploy.sh` 的插件 README 按全名落盘（`Element_Models.md`）。
坑：`plugin-versions.py` 的考古 key 用全名后，`build.rs` 也必须读 `plugin.toml` 的 `name` 当 key，
否则 capabilities 回退发行版号 |
| 2026-11-24 | 第二条宿主能力落在 `element-models`：`PUT /v1/providers/{id}/secret`（密钥值写宿主文件，元素库仍只存引用）|
| 2026-11-24 | `element-models` 交出 `GET /v1/models/elements`，`Host::elements_repo` 由 `server` 实现；`server/src/models.rs` 只剩 `ModelsResponse` 适配 |
| 2026-11-24 | `verify.sh` 判据 5/6/12 转绿（`os_env::run_capture_async`、`chord::sqlite::inspect`）；本轮不搬端点 |
| 2026-11-24 | 客户端去掉「模块名 → 中文名」表（`docs/plugins.md` 规则 9）：模块名只有英文一处真相，新插件客户端自动出现 |
| 2026-11-25 | 改名票：`stats` → `data/data-manager`（vendor=data，包名 `data-data-manager`）；端点路径 `/v1/stats/*` 一个字不变 |
| 2026-11-25 | **改名票**：`stats` → 出品方 `data` 的 `data-manager`（目录 `crates/Data/DataManager`、包名 `data-data-manager`，端点路径 `/v1/stats/*` 一个字没动）；规则 10 由此立起来 |
| 2026-11-25 | `data-manager` 交出 8 条 `GET /v1/stats/*`：`Host::stats_paths` 注入两个库路径；`server/src/stats.rs` 删除（`openapi` 搬回 `server/routes.rs`），静态表删 8 行 |
| 2026-11-25 | `plugin.toml` 删掉 `endpoints` 字段（28 个清单）：端点唯一真相是 `routes()`，清单只记身份（规则 11） |
| 2026-11-25 | `archive` 交出 3 条世代归档端点，落 `crates/Data/Archive`（vendor=`data`、包名 `data-archive`、模块全名 `archive`）：宿主能力是**命令面** `Host::{archive_session, archive_generations, restore_session}`（实现只在 `server/src/plugins.rs`，全走同一个 `CommandSource`）；`server/src/routes.rs` 的 `archive` / `archive_index` / `restore` / `RestoreBody` 整份删；静态表删 3 行 |
| 2026-11-25 | `elements` 交出 8 条 `/v1/elements*`（组 / 包读写与锁）：宿主能力只有一个 `Host::elements_repo()` —— 盘上真相与「写一次提交一次」本来就在 `nous-elements`（这个模块早就是独立 crate，只缺端点）。`RepoError → 状态码` 的翻译搬进插件的 `Refusal::from_repo`（一处判：400 / 404 / 409 / 423 / 500，`Validation` 多带 `{field, problem}`）；`server/src/elements.rs` 整份删；静态表删 8 行（151 → 143）。坑：搬家后注册表写法从 `{kind}` 变成 `:kind`（`nous-routes` 折的），`capabilities` 的 `endpoints[]` 快照与断言要跟着改 |
| 2026-11-25 | `skills` 交出 6 条 `/v1/skills*`：第一次搬「端点住在三仓共用骨架里」的模块 —— 先把盘面规矩下沉到 `nous_repo::entries`（名字 / 路径校验、递归清单、骨架落盘、发布），`resource_admin.rs` 只剩 prompts / extensions 两仓（522 → 309 行）；宿主能力 `skills_repo` / `skills_root` / `skills_usage`；`meta::skills()` 与静态表 6 行删（143 → 137）。坑：`publish` 的回包形状必须与 `CommitResult` 逐字一致（`commit` 是 hash 字符串不是对象），改形状会把 `resource_admin_endpoints` 打红 |
