//! 静态元数据端点：`device` / `selection` / `extensions` /
//! `capabilities` / `_routes`（#47 + #48 合并后的唯一注册表）。
//!
//! `skills` / `prompts` 已搬进各自的插件（`routes()` 声明 + 自己的 handler）。
//!
//! 这里只做**数据装配**，没有第二份事实源：进程配置来自环境，能力声明是这张表本身。
//! 形状以 `wire.md` + `NousDesk/crates/nous-engine/src/types.rs` 为准；未知字段透传给客户端。
//!
//! `ROUTES` 是 `/v1/*` 端点的唯一真相：`router()` 从这里装配真路由、`capabilities()` 从这里生成
//! `endpoints`、`_routes` 从这里吐调试视图。三处同源，才撑得住「声明不许说谎」。
//!
//! #65：device 段（机器 / 模型 / 默认模型）已经搬到 `nous-device` 插件，这里只留转发。

use std::path::PathBuf;

use nous_env::{ContentPath, PRESETS};
use serde_json::{json, Value};

/// `GET /v1/device`。`node` 是引擎版本标识（About 面板显示），`startedAt` 是 epoch 毫秒。
/// 数据装配归 `nous-device`；build / target 来自 server 的产物身份（它才知道自己是什么二进制）。
pub fn device(models: &nous_device::ModelsResponse) -> Value {
    let body = nous_device::device(models);
    let default = body["model"].clone();
    // 字段顺序按 wire.md 的既有快照钉住（serde_json 开了 preserve_order）。
    json!({
        "self": body["self"],
        "origin": body["origin"],
        "node": env!("CARGO_PKG_VERSION"),
        "version": env!("CARGO_PKG_VERSION"),
        "build": crate::version::build_id(),
        "target": crate::version::target_triple(),
        "startedAt": body["startedAt"],
        "model": default,
        "faux": body["faux"],
        "data": body["data"],
        "catalog": body["catalog"],
        "available": body["available"],
    })
}

/// `GET /v1/selection`：当前没有选中的会话，诚实回 `null`（客户端 0 处依赖）。
pub fn selection() -> Value {
    json!({ "id": std::env::var("NOUS_SELECTION").ok().and_then(|s| s.parse::<i64>().ok()) })
}

/// 一条私库路径的只读状态：`root` 永远给解析出的路径，`present` 才说它在不在盘上。
/// 目录不存在时不建、不写、不 spawn —— "没装" 和 "不知道在哪" 从此可区分（#63）。
fn content_status(path: ContentPath) -> (PathBuf, Value, bool) {
    let root = path.resolve();
    let present = root.is_dir();
    let shown = Value::String(root.to_string_lossy().into_owned());
    let status = if present {
        json!({ "root": shown, "present": true, "missing": Value::Null })
    } else {
        json!({
            "root": shown,
            "present": false,
            "missing": format!("私库路径不存在：{}（设 {} 指到你的副本）", path.default, path.var),
        })
    };
    (root, status, present)
}

/// 模板数据根（#60 用）：只解析、只登记，不在这里读写私库。
pub fn presets_root() -> std::path::PathBuf {
    PRESETS.resolve()
}

/// `GET /v1/extensions`：#81 真清单投影。没装配运行时就诚实回空数组。

// ── 端点注册表：唯一真相（#47 拷问结果 §2/§4/§8 + #48 的 _routes）──────
//
// `router()` 从这里装配真路由；`capabilities()` 从这里生成 endpoints；
// `routes_view()` 是 `GET /v1/_routes` 的同一份视图。三处共用一张表：
// 手抄数组会让「声明不许说谎」的判据失去意义。

/// 一条已注册路由：`(method, 路由模板原文)`。模板保留 `:id` / `{kind}` 的注册形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredRoute {
    pub method: &'static str,
    pub path: &'static str,
    /// `docs/plugins.md` 的 module 列（用户功能面，不是 crate 名）。
    pub module: &'static str,
}

/// 全部由本进程注册的 `/v1/*` 路由。**新增端点先在这里落一行**。
pub const ROUTES: &[RegisteredRoute] = &[
    // #61：combos 两条端点（开关是引擎算出来的，Desk 只显示 + 写穿）；#82 补试跑。
    RegisteredRoute { method: "GET", path: "/v1/sessions", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/unread", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/origins", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/read/:id", module: "sessions" },
    // #68：只读目录浏览（env 插件）。
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/status", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/pending", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/pending/diff", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/pending/commit", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/history", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/history/diff", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/commit", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/archive", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/archive", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/restore", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/purge", module: "repo" },
    // #76：变体（skills / extend）。
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/variants", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/variants/bind", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/variants/unbind", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/variants/branch", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/variants/publish", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/variants/prune", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/variants/usage", module: "repo" },
    // #77：上游追踪（sources + 10 条 upstream 动作）挂在 skills 私库上。
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/sources", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/sources", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/upstream", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/upstream/refresh", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/upstream/pin", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/upstream/check", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/upstream/sync", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/upstream/tree", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/upstream/file", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/repos/:repo/upstream/files", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/upstream/copy", module: "repo" },
    RegisteredRoute { method: "POST", path: "/v1/repos/:repo/upstream/adopt", module: "repo" },
    RegisteredRoute { method: "GET", path: "/v1/models", module: "Nous_Device" },
    RegisteredRoute { method: "PUT", path: "/v1/default", module: "Nous_Device" },
    RegisteredRoute { method: "POST", path: "/v1/catalog/refresh", module: "Nous_Device" },
    RegisteredRoute { method: "POST", path: "/v1/sessions", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/sessions/:id/entries", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/messages", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/sessions/:id/events", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/abort", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/fork", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/rename", module: "sessions" },
    // #49：置顶写穿走 sessions 模块；与 rename/read 一样经同一个 `Session` 写者。
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/pin", module: "sessions" },
    // #64：写入面与 selection 两行状态都属于 sessions 模块。
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/config", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/purge", module: "sessions" },
    // #67 的世代归档三端点已搬进 `data/archive` 插件（`routes()` 声明），静态表只减不增。
    RegisteredRoute { method: "GET", path: "/v1/selection", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/selection", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/device", module: "Nous_Device" },
    // `extensions` 的 15 条已搬进 `Nous_Extend` 插件（`nous-extend` 的 `routes()` 声明），
    // 静态表只减不增。
    // #85：统一搜索投影（sessions 正文 + documents），服务端一次扫描。
    RegisteredRoute { method: "GET", path: "/v1/search", module: "sessions" },
    // 触发器的 24 条（定义面 11 + 工程面 13）已搬进 `Nous_Triggers` 插件
    // （`nous-triggers` 的 `routes()` 声明），静态表只减不增。
    RegisteredRoute { method: "GET", path: "/v1/capabilities", module: "Nous_Device" },
    // 插件人话 README：正文随产物发布到 docs/plugins/，端点只读盘。
    // 归 `device`（不是"server"）：`_routes` / `capabilities` 只吐**清单里开着的模块**，
    // "server" 不在 MANIFEST 的 14 条里 —— 归它等于这条端点永远不出现在 /v1/_routes，
    // 客户端会一直报「路由漂移」。device 是 0.1.25 里唯一「宿主级只读」的模块。
    RegisteredRoute { method: "GET", path: "/v1/plugins/{name}/readme", module: "Nous_Device" },
    // 机读总表：从注册表（本表 ∪ 插件声明）生成，自身也归 device。
    RegisteredRoute { method: "GET", path: "/v1/openapi.json", module: "Nous_Device" },
    // #60：13 条 presets 资源化端点（清单即详情 + 修订表）。
    // 元素库（组 / 包）：盘上真相在私库，写一次提交一次。
    RegisteredRoute { method: "GET", path: "/v1/_routes", module: "Nous_Device" },
];

/// `GET /v1/_routes`：debug 视图与 capabilities 从同一张表吐出。
///
/// `modules` 覆盖全部清单行（含未实现的，便于 desk 调试），`routes` 只列清单开着且
/// 代码注册了的端点——和 enabled 模块的 `endpoints` 多重集相等（不声明不存在的端点，
/// 未装配运行时的 `ext` 端点仍回 200 空清单，但真路径永远来自同一张表）。
pub fn routes_view() -> Value {
    json!({
        "routes": endpoints()
            .into_iter()
            .map(|route| json!({ "module": route.module, "method": route.method, "path": route.path }))
            .collect::<Vec<_>>(),
        "modules": MANIFEST
            .iter()
            .map(|entry| json!({ "name": entry.name, "enabled": route_enabled(entry.name) }))
            .collect::<Vec<_>>(),
    })
}

/// `GET /v1/openapi.json`：从 `ROUTES` 生成 paths，与 capabilities/_routes 三处同源。
pub fn openapi_paths() -> Value {
    let mut paths = serde_json::Map::new();
    for route in endpoints() {
        let entry = paths.entry(route.path.clone()).or_insert_with(|| json!({}));
        if let Some(object) = entry.as_object_mut() {
            object.insert(
                route.method.to_ascii_lowercase(),
                json!({ "operationId": format!("{} {}", route.method, route.path) }),
            );
        }
    }
    Value::Object(paths)
}

/// 某模块是否在发行清单里开着且可用（给判据与 _routes 用）。
pub fn route_enabled(module: &str) -> bool {
    MANIFEST
        .iter()
        .find(|entry| entry.name == module)
        .is_some_and(|entry| entry.reason.is_none())
}

fn enabled_routes() -> impl Iterator<Item = &'static RegisteredRoute> {
    ROUTES.iter().filter(|route| route_enabled(route.module))
}

/// 合流后的一条端点：静态表（还没搬进插件的）∪ 插件声明（`nous-routes`）。
///
/// 路径统一是 `:id` 形式（`wire.md` 的写法）；插件交上来的是 axum 的 `{id}`，
/// 由 `Route::registry_path` 折成同一种，注册表里不会出现两种写法。
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: String,
    pub module: &'static str,
}

/// 注册表 = 静态表 ∪ 插件声明。搬一个端点就从 `ROUTES` 删一行。
pub fn endpoints() -> Vec<Endpoint> {
    let mut all: Vec<Endpoint> = enabled_routes()
        .map(|route| Endpoint {
            method: route.method,
            path: route.path.to_string(),
            module: route.module,
        })
        .collect();
    for declaration in crate::plugins::declarations() {
        if !route_enabled(declaration.module) {
            continue;
        }
        all.push(Endpoint {
            method: declaration.method,
            path: declaration.path,
            module: declaration.module,
        });
    }
    all
}

/// `#88`：清单的一行。五个展示列逐格抄自 `docs/plugins.md` 主表，
/// `capabilities_manifest.rs` 逐格比对，不许第二份手抄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestEntry {
    pub name: &'static str,
    pub reason: Option<ReasonCode>,
    pub layer: Layer,
    pub kind: ModuleKind,
    pub required: bool,
    pub crate_name: &'static str,
    pub description: &'static str,
}

impl ManifestEntry {
    pub fn required_text(self) -> &'static str {
        if self.required { "是" } else { "否" }
    }
}

/// 四值封闭，与 `docs/plugins.md` 的 layer 列同源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Kernel,
    Platform,
    App,
    Dev,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Platform => "platform",
            Self::App => "app",
            Self::Dev => "dev",
        }
    }
}

/// 三值封闭，与 `docs/plugins.md` 的「形态」列同源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Compiled,
    Declarative,
    External,
}

impl ModuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compiled => "编译期",
            Self::Declarative => "声明式",
            Self::External => "运行时外挂",
        }
    }
}

/// 清单：模块名 == `docs/plugins.md` 的 module 列（评论第二条的拍板）。
/// `enabled` = 清单 ∩ 代码：没有注册路由的模块一律 disabled + reason。
pub const MANIFEST: &[ManifestEntry] = &[
    ManifestEntry { name: "sessions", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-sessions`", description: "**pi 本体**（#44 落地），承载最小轮次与取消收敛" },
    // #68：env 的本地目录浏览已落地，SSH 远端另票。
    ManifestEntry { name: "env", reason: None, layer: Layer::Platform, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-env`", description: "契约；实现见 `env-local` / `env-ssh`。`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` / `NOUS_EXTEND` 一处解析，unit 不写；写回私库归 `skills-admin` / `presets-admin`。自带 `GET /v1/fs/dirs`（远端分支是宿主能力 trait：`nous-env-ssh` 的 requires 里有本 crate，反向依赖会成环）" },
    // #75：四套私库共用的 git 发布 / 归档 / pending / diff / history。
    ManifestEntry { name: "repo", reason: None, layer: Layer::Platform, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-repo`", description: "唯一的 git 进程调用面；四条私库路径解析沿用 `env`" },
    // #60：13 条 presets 端点 + 修订表已落地；形态取 docs/plugins.md 的第一条 presets 行。
    ManifestEntry { name: "presets", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "—", description: "数据根 `NOUS_PRESETS=/opt/Nous-Template`（#63；段清单读 `<root>/sections.json`，路径解析归 `env`）" },
    // #60：模板管理的判定层（TS 内核 `presets.js` / `presets-api.js` 的归位）。
    ManifestEntry { name: "presets-admin", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-presets-admin`", description: "从 TS 内核 `presets.js` 750 + `presets-api.js` 947 归位；段清单读 `<root>/sections.json`" },
    ManifestEntry { name: "skills", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-skills`", description: "默认根 `NOUS_SKILLS=/opt/Nous-skills`（#63，路径解析归 `env`）；`SKILL.md` frontmatter / 附件 / 类别词表解析仍在 `lib.rs`；盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，与 `prompts` / `ext` 共用同一份；不 spawn git（发布走 `nous-repo`）" },
    ManifestEntry { name: "archive", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`data-archive`", description: "三条端点自带 handler（`crates/Data/Archive/src/routes.rs`）；供应商 `data`，包名 `data-archive`" },
    // #61：两条 combos 端点已落地；六列逐格取 docs/plugins.md 的 combos 行。
    ManifestEntry { name: "combos", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-combos`", description: "数据根 `NOUS_PRESETS`（与 `presets` 同根）；触发器存在性从 `NOUS_TRIGGER` 目录读。宿主能力四件事：两个盘面根 / 触发器判定视图 / 注入时钟 / 起会话 + 写血缘" },
    ManifestEntry { name: "Nous_Triggers", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-triggers`", description: "#57/#58/#73/#80 落地；插件出生即全名（规则 12）；旧的 `triggers` / `trigger-catalog` 两行并入本行" },
    ManifestEntry { name: "data-manager", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`data-data-manager`", description: "归口监控六屏；与 `--check` 共读口，tasks 现算" },
    ManifestEntry { name: "Nous_Device", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-device`", description: "#65：从 server 提成独立插件；`/v1/models` 文件+内联来源，`PUT /v1/default` 原子写" },
    ManifestEntry { name: "Nous_Prompts", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-prompts`", description: "默认根 `NOUS_PROMPTS=/opt/Nous-prompts`（#72，路径解析归 `env`）；`PROMPT.md` + `nous.json` + `settings.json` 的只读扫描与纯 `render()` 仍在 `lib.rs`；盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，与 `skills` / `ext` 共用同一份；`RepoError → 状态码` 的翻译在 `nous_repo::Refusal`（HTTP 回包壳在 `nous_routes::HttpRefusal`）；自带 6 条 `/v1/prompts*` 端点" },
    ManifestEntry { name: "Nous_Extend", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-extend`", description: "已落地（#17–#37，#81 解冻：真清单 / 熔断 / 热重载，#84 补管理面）；自带 15 条 `/v1/extensions*` 端点（扩展管理面 + 编辑页 4 条走 `nous_repo::entries`）；`ext-host` 行是它后续的重划" },
    // 元素库：旧面板 `k3skws` 块的 Rust 落点；盘上真相 = 第五套私库。
    ManifestEntry { name: "Element_Models", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`element-models`", description: "盘上真相在 `nous-elements`；解析 / 合并 / 投影是纯函数（不读盘、不联网、不看时钟）" },
    ManifestEntry { name: "elements", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-elements`", description: "元素库（组 / 包）：数据根 `NOUS_ELEMENTS=/opt/Nous-Elements`，一份 JSON 一个元素；8 条端点自带 handler（`crates/Nous/Elements/src/routes.rs`）" },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonCode {
    NotImplemented,
    #[allow(dead_code)] // 四值封闭：desk 按这个枚举分支，不许少一个
    DisabledByManifest,
    Frozen,
    #[allow(dead_code)] // 同上；#51 接上凭证后才真正构造
    NeedsCredentials,
}

impl ReasonCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotImplemented => "not_implemented",
            Self::DisabledByManifest => "disabled_by_manifest",
            Self::Frozen => "frozen",
            Self::NeedsCredentials => "needs_credentials",
        }
    }
    fn message(self, name: &str) -> String {
        match self {
            Self::NotImplemented => format!("{name} 模块尚未落地"),
            Self::DisabledByManifest => format!("{name} 在发行清单里关闭"),
            Self::Frozen => "扩展运行时已冻结，这里只返回空清单".to_string(),
            Self::NeedsCredentials => format!("{name} 需要凭证才能使用"),
        }
    }
}

/// `GET /v1/capabilities`：一个发行版一张清单，endpoints 全从注册表生成。
///
/// 形状是前向兼容的：未来（#51）可能给 module 加可选的 `kind` 字段，那是**加法**，
/// 不替换 `name` / `enabled` / `endpoints` / `reason` 的既有语义。
pub fn capabilities(origin: &str) -> Value {
    let modules: Vec<Value> = MANIFEST
        .iter()
        .map(|entry| {
            let name = entry.name;
            let registered: Vec<Endpoint> = endpoints()
                .into_iter()
                .filter(|route| route.module == name)
                .collect();
            // 清单关掉的模块即便有代码也不声明；清单开着但没代码就 not_implemented。
            let reason = entry.reason.or_else(|| {
                registered
                    .is_empty()
                    .then_some(ReasonCode::NotImplemented)
            });
            // #88：六列是新加的，老桌面端只读 name/enabled/endpoints/reason，照旧能解析。
            // #93：`version` 是这个**插件自己的**版本（从它自己的 plugin.toml 生成，见
            // `plugin_versions`），不是发行版号 —— 否则 14 个模块永远显示同一个数，
            // 「关于 → 插件」列表上看不出谁动过。取不到才回退发行版号，并加
            // `versionSource: "release"` 如实说明（回退不静默）。
            let (version, fallback) = crate::plugin_versions::version_of(name);
            // 规则 10：`vendor` 是出品方（`nous` / `data` …），桌面端拿它当副标题。
            // 没有清单的模块（外挂 / 待建）这里是 null —— 不猜、不编一个 nous 顶上。
            let vendor = crate::plugin_versions::vendor_of(name);
            let mut module = json!({
                "name": name,
                "version": version,
                "kind": entry.kind.as_str(),
                "layer": entry.layer.as_str(),
                "required": entry.required,
                "crate": entry.crate_name,
                "vendor": vendor,
                "description": entry.description,
            });
            if fallback {
                module["versionSource"] = json!("release");
            }
            match reason {
                Some(code) => {
                    module["enabled"] = json!(false);
                    module["reason"] = json!({ "code": code.as_str(), "message": code.message(name) });
                }
                None => {
                    module["enabled"] = json!(true);
                    module["endpoints"] = json!(registered
                        .iter()
                        .map(|route| format!("{} {}", route.method, route.path))
                        .collect::<Vec<_>>());
                }
            }
            module
        })
        .collect();
    json!({
        "contract": 1,
        "server": { "name": "nous", "version": env!("CARGO_PKG_VERSION") },
        "modules": modules,
        "runtime": {
            "provider": {
                "configured": false,
                "message": "没有配置真实 provider 凭证，发消息会 503",
            }
        },
        // 一个地址 + 服务表：engine 是它自己，triggers 尚未实现。
        "services": [
            { "name": "engine", "base": origin },
            { "name": "triggers", "base": origin, "enabled": true },
        ],
    })
}
