//! 静态元数据的**数据面**：清单（`MANIFEST`）与静态表（`ROUTES`）在这里。
//!
//! 视图不在这里了 —— `/v1/capabilities`、`/v1/_routes`、`/v1/openapi.json` 的组装
//! 搬进了 `crates/Nous/Device/src/routes.rs`（`Device` 插件，规则 11：端点的家就是
//! 插件的 `routes()`）。本文件只剩两份原料：清单行（[`manifest_lines`]）和注册表
//! （[`registered`]），由 `plugins.rs` 的 `impl nous_device::routes::Host` 交出去。
//!
//! `Skill` / `Nous_Prompts` 已搬进各自的插件（`routes()` 声明 + 自己的 handler）。
//!
//! 这里只做**数据装配**，没有第二份事实源：进程配置来自环境，能力声明是这张表本身。
//! 形状以 `wire.md` + `NousDesk/crates/nous-engine/src/types.rs` 为准；未知字段透传给客户端。
//!
//! `ROUTES` 只装**还没搬进插件的**端点；搬一个就从这里删一行（搬运进度见
//! `docs/magento2-migration.md`）。`/v1/*` 的完整真相 = `ROUTES` ∪ 各插件 `routes()`，
//! 两者由 [`registered`] 合流后交给插件判 enabled。

use std::path::PathBuf;

use nous_env::{ContentPath, PRESETS};
use serde_json::{json, Value};

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

/// 清单的一行交给 `Device` 插件（`capabilities` / `_routes` / `readme` 的原料之一）。
///
/// 宿主只做「读清单 + 查版本」，**视图组装在插件里**：`enabled` 要同时看清单开关和
/// 注册表里有没有它的端点，那个判定归端点自己。
pub fn manifest_lines() -> Vec<nous_device::routes::ManifestLine> {
    MANIFEST
        .iter()
        .map(|entry| {
            let (version, fallback) = crate::plugin_versions::version_of(entry.name);
            nous_device::routes::ManifestLine {
                name: entry.name,
                reason: entry.reason.map(|code| nous_device::routes::Refusal {
                    code: code.as_str(),
                    message: code.message(entry.name),
                }),
                layer: entry.layer.as_str(),
                kind: entry.kind.as_str(),
                required: entry.required,
                crate_name: entry.crate_name,
                description: entry.description,
                version,
                vendor: crate::plugin_versions::vendor_of(entry.name).map(str::to_string),
                version_from_release: fallback,
            }
        })
        .collect()
}

/// 注册表 = 静态表（还没搬走的端点）∪ 各插件的 `routes()` 声明。**未过滤**：
/// 按清单开关过滤是插件的事（它才知道 `enabled` 怎么判）。
pub fn registered() -> Vec<nous_routes::Declaration> {
    let mut all: Vec<nous_routes::Declaration> = ROUTES
        .iter()
        .map(|route| nous_routes::Declaration {
            method: route.method,
            path: route.path.to_string(),
            module: route.module,
        })
        .collect();
    all.extend(crate::plugins::declarations());
    all
}

/// 注册表的一行（宿主侧判据用）。**过滤逻辑不在这里** —— 它跟着视图住在
/// `nous_device::routes`，这里只是把结果摊成宿主自己的结构体。
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: String,
    pub module: &'static str,
}

/// 注册表 = 静态表 ∪ 插件声明，已按清单开关过滤。
pub fn endpoints() -> Vec<Endpoint> {
    nous_device::routes::enabled_endpoints(&manifest_lines(), &registered())
        .into_iter()
        .map(|(method, path, module)| Endpoint {
            method,
            path,
            module,
        })
        .collect()
}

/// 某模块在清单里开着没有。转发到插件，免得两份实现漂。
pub fn route_enabled(module: &str) -> bool {
    nous_device::routes::line_enabled(&manifest_lines(), module)
}

/// 全部由本进程注册的 `/v1/*` 路由。**新增端点先在这里落一行**。
pub const ROUTES: &[RegisteredRoute] = &[
    // sessions 的 17 条里 16 条已归插件；**这一条留在 server**（要宿主组装根与提示词注入）。
    // 其余 8 条 `Device` 端点（`/v1/models` / `/v1/default` / `/v1/catalog/refresh` /
    // `/v1/device` / `/v1/capabilities` / `/v1/_routes` / `/v1/openapi.json` /
    // `/v1/plugins/{name}/readme`）已搬进 `Device` 插件的 `routes()` 声明。
    //
    // **静态表只剩这一行** —— 再往里加行之前先问一句：它为什么不能住在它自己的插件里？
    RegisteredRoute { method: "POST", path: "/v1/sessions/:id/messages", module: "sessions" },
];

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
    ManifestEntry { name: "sessions", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-sessions`", description: "**pi 本体**（#44 落地）：最小轮次与取消收敛 + 会话读投影（`source`）/ 写命令（`commands`）/ 请求形状（`records`）/ 事件流（`hub` / `coalesce` / `project` / `observer`）；`/v1/sessions*` 十七条里 十六条自带 handler，`messages` 留在宿主侧（要组装根的 driver / completions 与提示词注入）" },
    // #68：env 的本地目录浏览已落地，SSH 远端另票。
    ManifestEntry { name: "env", reason: None, layer: Layer::Platform, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-env`", description: "契约；实现见 `env-local` / `env-ssh`。`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` / `NOUS_EXTEND` 一处解析，unit 不写；写回私库归 `skills-admin` / `presets-admin`。自带 `GET /v1/fs/dirs`（远端分支是宿主能力 trait：`nous-env-ssh` 的 requires 里有本 crate，反向依赖会成环）" },
    // #75：四套私库共用的 git 发布 / 归档 / pending / diff / history。
    ManifestEntry { name: "repo", reason: None, layer: Layer::Platform, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-repo`", description: "唯一的 git 进程调用面；六条私库路径解析沿用 `env`" },
    // #60：13 条 presets 端点 + 修订表已落地；形态取 docs/plugins.md 的第一条 presets 行。
    ManifestEntry { name: "presets", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "—", description: "数据根 `NOUS_PRESETS=/opt/Nous-Template`（#63；段清单读 `<root>/sections.json`，路径解析归 `env`）" },
    // #60：模板管理的判定层（TS 内核 `presets.js` / `presets-api.js` 的归位）。
    ManifestEntry { name: "presets-admin", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-presets-admin`", description: "从 TS 内核 `presets.js` 750 + `presets-api.js` 947 归位；段清单读 `<root>/sections.json`" },
    ManifestEntry { name: "Skill", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`library-skill`", description: "默认根 `NOUS_SKILLS=/opt/Nous-skills`（#63，路径解析归 `env`）；`SKILL.md` frontmatter / 附件 / 类别词表解析仍在 `lib.rs`；盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，与 `prompts` / `Extend` 共用同一份；不 spawn git（发布走 `nous-repo`）" },
    ManifestEntry { name: "archive", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`data-archive`", description: "三条端点自带 handler（`crates/Data/Archive/src/routes.rs`）；供应商 `data`，包名 `data-archive`" },
    // #61：两条 combos 端点已落地；六列逐格取 docs/plugins.md 的 combos 行。
    ManifestEntry { name: "combos", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`nous-combos`", description: "数据根 `NOUS_PRESETS`（与 `presets` 同根）；触发器存在性从 `NOUS_TRIGGER` 目录读。宿主能力四件事：两个盘面根 / 触发器判定视图 / 注入时钟 / 起会话 + 写血缘" },
    ManifestEntry { name: "Trigger_Core", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`trigger-core`", description: "#57/#58/#73/#80 落地；插件出生即全名（规则 12）；旧的 `triggers` / `trigger-catalog` 两行并入本行" },
    ManifestEntry { name: "data-manager", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`data-data-manager`", description: "归口监控六屏；与 `--check` 共读口，tasks 现算" },
    ManifestEntry { name: "Device", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-device`", description: "#65：从 server 提成独立插件；自带 9 条端点（机器 / 模型三件套 / `device` / `_routes` / `capabilities` / `openapi.json` / 插件 README）；自省三件套的**原料归宿主、视图归插件**" },
    ManifestEntry { name: "Nous_Prompts", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: true, crate_name: "`nous-prompts`", description: "默认根 `NOUS_PROMPTS=/opt/Nous-prompts`（#72，路径解析归 `env`）；`PROMPT.md` + `nous.json` + `settings.json` 的只读扫描与纯 `render()` 仍在 `lib.rs`；盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，与 `Skill` / `Extend` 共用同一份；`RepoError → 状态码` 的翻译在 `nous_repo::Refusal`（HTTP 回包壳在 `nous_routes::HttpRefusal`）；自带 6 条 `/v1/prompts*` 端点" },
    ManifestEntry { name: "Extend", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`library-extend`", description: "已落地（#17–#37，#81 解冻：真清单 / 熔断 / 热重载，#84 补管理面）；自带 15 条 `/v1/extensions*` 端点（扩展管理面 + 编辑页 4 条走 `nous_repo::entries`）；`ext-host` 行是它后续的重划" },
    // 元素库：旧面板 `k3skws` 块的 Rust 落点；盘上真相 = 第五套私库。
    ManifestEntry { name: "Models", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`element-models`", description: "盘上真相在 `element-core`；解析 / 合并 / 投影是纯函数（不读盘、不联网、不看时钟）" },
    ManifestEntry { name: "Element_Core", reason: None, layer: Layer::App, kind: ModuleKind::Compiled, required: false, crate_name: "`element-core`", description: "元素库（组 / 包）：数据根 `NOUS_ELEMENTS=/opt/Nous-Elements`，一份 JSON 一个元素；8 条端点自带 handler（`crates/Element/Core/src/routes.rs`）" },
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

