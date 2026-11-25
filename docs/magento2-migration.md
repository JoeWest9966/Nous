# 端点归插件（Magento 2 式）迁移记

> 这份文档是**搬家记事本**：目标、接缝、规矩、进度都记在这里。搬一个端点改一次进度表，
> 不另开第二份清单。判据见文末，命令照抄即可跑。

## 1 · 一句话目标

**端点归插件，`server` 只做「监听 + 中间件 + 分发」。** 插件自己声明路由、自己写 handler；
`server` 不认识任何一条业务路径。

## 2 · 为什么要搬

| 现状（搬家前） | 症状 |
|---|---|
| 所有 handler 都在 `crates/nous/server/src/`，`routes.rs` 一千多行 | 加一条端点就得改 `server`，插件再小也得排队 |
| 归属靠 `meta::ROUTES` 一张**手写静态表**声明 | 声明与实现是两份东西，只能靠判据对齐；搬/删容易漂 |
| `plugin.toml` 的 `endpoints` 全是空数组 | 清单里读不出某个插件到底提供了什么端点 |

Magento 2 的做法是：模块自带 controller，`etc/*/routes.xml` 声明路由，框架启动时把各模块的
路由聚成一张表，唯一入口只做 dispatch + 中间件。**我们现在搬的就是这个：模块自带 controller。**

## 3 · 接缝（已经建好的地基）

### 3.1 `nous-routes`：一条声明同时是「注册」和「登记」

```rust
// 插件侧（以 device 为例）
pub fn routes<S>() -> Vec<nous_routes::Route<S>>
where S: Clone + Send + Sync + 'static {
    vec![nous_routes::Route::new("GET", "/v1/machines", "device", || get(machines))]
}
```

- `Route::mount` 把 handler 装进 `Router`；`Route::declaration` 供 `capabilities` /
  `/v1/_routes` / `/v1/openapi.json` 读。**两者出自同一处，不可能分叉。**
- 路径写法只有一份：插件写 axum 的 `/v1/x/{id}`，注册表自动折成 `wire.md` 的 `/v1/x/:id`。
- `server` 侧只有两个函数：`plugins::routes()`（收集）与 `plugins::declarations()`（交给注册表）。

### 3.2 宿主能力 trait：插件不依赖 `server`

> 现在有两个能力的样板：`element-models` 的 `Host`（元素库仓库）与 `Secrets`（密钥文件路径）。
> 新端点需要什么，就在插件里加一个窄 trait，别去依赖 `AppState`。

插件不认识 `AppState`。它**声明自己需要什么**，由 `server` 实现并注入：

```rust
// 插件侧（element-models）
pub trait Host {
    fn elements_repo(&self) -> Option<std::sync::Arc<nous_repo::Repo>>;
}
impl<T: Host + ?Sized> Host for std::sync::Arc<T> { /* 转发 */ }

// 宿主侧（server/src/plugins.rs）
impl nous_element_models::routes::Host for AppState {
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

## 4 · 进度

### 4.1 已搬（10 条）

| 模块 | 端点 | 提交 | 备注 |
|---|---|---|---|
| `device` | `GET /v1/machines` | `feat(routes)` | 第一条试点，证明接缝通；handler 无需宿主状态 |
| `element-models` | `GET /v1/models/elements` | `feat(element-models)` | 第一条**需要宿主注入**的端点；`Host::elements_repo` |
| `element-models` | `PUT /v1/providers/{id}/secret` | `feat(element-models)` | **新端点**（不是搬家）：插件自带 handler，宿主给 `Secrets::secrets_file` |
| `data-manager` | `GET /v1/stats/{usage,tasks,db,overview,conversations,conversation/:id,cost,health}`（8 条） | `refactor(data-manager)` | 第一次「一个插件一次搬 8 条」：`Host::stats_paths` 只给两个库路径；`server/src/stats.rs` 整份删掉，`openapi` 归 `server/routes.rs`（它读的是注册表，不是统计） |

### 4.2 待搬（153 条 / 14 个模块）

按「搬起来最省事 → 最费事」排的，数字是该模块在静态表里还剩几条。

| 模块 | 存量 | 需要哪些宿主能力（预估） |
|---|---|---|
| `env` | 1 | 目录解析（`NOUS_START_DIRS`），无 IO 以外的状态 |
| `archive` | 3 | 归档/墓碑（`nous-durable` sessions） |
| `skills` | 6 | 私库读写（`nous-repo`）+ 变体表 |
| `prompts` | 6 | 私库读写（`nous-repo`） |
| `elements` | 8 | 元素库私库（已有 `element-models` 的先例） |
| `combos` | 4 | 组合段读写 + 试跑（依赖 `sessions` 起轮） |
| `presets` | 13 | 模板私库 + 修订表 |
| `presets-admin` | 19 | 模板私库 + 形状闸 |
| `ext` | 15 | 扩展私库 + 运行时（`nous-ext`）+ 管理面 |
| `triggers` | 24 | 触发器库 + 调度（`nous-dispatcher`） |
| `sessions` | 17 | 最厚：读投影 + 写命令 + SSE 出口（`events` 尤其要小心） |
| `repo` | 30 | 最杂：git 面全在这一家（`nous-repo` 已是 platform，宿主只需给根路径） |
| `device` | 7 | 剩下的编目/默认模型/`_routes`/`openapi` 等自省端点，建议**最后搬**（它们读注册表本身） |

建议顺序：`env` → `archive` → `skills` / `prompts` / `elements` → `combos` →
`presets(-admin)` → `ext` → `triggers` → `repo` → `sessions` → `device`。

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
3. `server/src` 里再也搜不到那条路径字面量：`grep -rn '"/v1/xxx"' crates/nous/server/src`。
4. 插件 `plugin.toml` 的 `[requires]` 补齐了新增的内部依赖（`check-plugins.sh` 会点名）。

## 6 · 已知遗留

- `nous-server` 仍是**唯一监听处**（`app` 层定义），这点不变；搬的是 handler 与声明。
- `plugin.toml` 的 `endpoints` 仍是空数组：端点由**代码声明**，不从清单生成。
  要让它也自动生成，得等 `/v1/_routes` 变成"从注册表导出清单"的生成物 —— 另开票。
- 元素库扫描失败的行为微调：搬家前走 `ServerError`（`repo_error` 映射），搬家后统一
  `500 {"error": "扫元素库失败：…"}`。形态没变（还是 `{error}` 一句话），状态码在
  极端分支上可能从 4xx 变 500。
- 进程创建点：`server` 里原本新的 `!cmd` 密钥解析自己 `Command::new("sh")`，
  已改成调 `nous_tools::os_env::run_capture`（S1 的唯一执行面，判据 6 / 12 的口子）。
  `crates/nous/server/src/trigger_projects.rs` 里的 `run_cargo` 是同一类欠账（更早就有），
  本条未动 —— 它要 async 语义，换法另议。
- 注册表顺序：静态表在前、插件声明在后；`capabilities_declares_modules_and_reasons.json`
  快照会随搬家调整端点顺序（内容不变）。

## 7 · 操作日志

| 日期 | 动作 |
|---|---|
| 2026-11-24 | 建 `nous-routes`（契约层）+ `server::plugins`（收集/装配/登记）；`device` 交出 `GET /v1/machines`（静态表删 1 行） |
| 2026-11-24 | 第二条宿主能力落在 `element-models`：`PUT /v1/providers/{id}/secret`（密钥值写宿主文件，元素库仍只存引用）|
| 2026-11-24 | `element-models` 交出 `GET /v1/models/elements`，`Host::elements_repo` 由 `server` 实现；`server/src/models.rs` 只剩 `ModelsResponse` 适配 |
| 2026-11-24 | `verify.sh` 判据 5/6/12 转绿（`os_env::run_capture_async`、`chord::sqlite::inspect`）；本轮不搬端点 |
| 2026-11-24 | 客户端去掉「模块名 → 中文名」表（`docs/plugins.md` 规则 9）：模块名只有英文一处真相，新插件客户端自动出现 |
| 2026-11-25 | 改名票：`stats` → `data/data-manager`（vendor=data，包名 `data-data-manager`）；端点路径 `/v1/stats/*` 一个字不变 |
| 2026-11-25 | **改名票**：`stats` → 出品方 `data` 的 `data-manager`（目录 `crates/data/data-manager`、包名 `data-data-manager`，端点路径 `/v1/stats/*` 一个字没动）；规则 10 由此立起来 |
| 2026-11-25 | `data-manager` 交出 8 条 `GET /v1/stats/*`：`Host::stats_paths` 注入两个库路径；`server/src/stats.rs` 删除（`openapi` 搬回 `server/routes.rs`），静态表删 8 行 |
