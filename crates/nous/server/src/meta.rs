//! 静态元数据端点：`device` / `selection` / `skills` / `prompts` /
//! `extensions` / `capabilities` / `_routes`（#47 + #48 合并后的唯一注册表）。
//!
//! 这里只做**数据装配**，没有第二份事实源：进程配置来自环境，能力声明是这张表本身。
//! 形状以 `wire.md` + `NousDesk/crates/nous-engine/src/types.rs` 为准；未知字段透传给客户端。
//!
//! `ROUTES` 是 `/v1/*` 端点的唯一真相：`router()` 从这里装配真路由（documents 那一族由
//! `documents::DOCUMENT_ROUTES` 生成，逐条登记在这里）、`capabilities()` 从这里生成
//! `endpoints`、`_routes` 从这里吐调试视图。三处同源，才撑得住「声明不许说谎」。
//!
//! #65：device 段（机器 / 模型 / 默认模型）已经搬到 `nous-device` 插件，这里只留转发。

use std::path::PathBuf;

use nous_env::{ContentPath, PRESETS, PROMPTS, SKILLS};
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

/// `GET /v1/skills`：目录不存在就是空表，不伪造内容、不 404。
///
/// 盘上真相由 `nous_skills` 插件解析（`SKILL.md` frontmatter / 附件 / 类别词表）。
/// 这里只做 wire 投影：目录路径转字符串、`Option` 转 `null`，`tracked` 恒 null
/// （#71 不碰 git，判定留给别的票）。`repo` 块只给 root + present（#63 形状）。
pub fn skills() -> Value {
    let (root, mut status, present) = content_status(SKILLS);
    let scanned = nous_skills::scan(&root);
    let entries = scanned
        .skills
        .iter()
        .map(|skill| {
            let load = if let Some(detail) = &skill.load.detail {
                json!({ "state": skill.load.state, "detail": detail })
            } else {
                json!({ "state": skill.load.state })
            };
            json!({
                "name": skill.name,
                "description": skill.description,
                "category": skill.category,
                "hidden": skill.hidden,
                "tracked": skill.tracked,
                "dir": skill.dir.to_string_lossy().into_owned(),
                "file": skill.file.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "attachments": skill.attachments,
                "load": load,
            })
        })
        .collect::<Vec<Value>>();
    status["categories"] = json!(scanned.categories);
    status["skills"] = json!(entries);
    status["repo"] = json!({
        "root": root.to_string_lossy().into_owned(),
        "present": present,
    });
    status
}

/// `GET /v1/prompts`：#48 前先与 skills 同构的空表。
pub fn prompts() -> Value {
    let (_, mut body, _) = content_status(PROMPTS);
    body["default"] = Value::Null;
    body["categories"] = json!([]);
    body["prompts"] = json!([]);
    body
}

/// 模板数据根（#60 用）：只解析、只登记，不在这里读写私库。
pub fn presets_root() -> std::path::PathBuf {
    PRESETS.resolve()
}

/// `GET /v1/extensions`：扩展运行时冻结，清单为空。
pub fn extensions() -> Value {
    json!({ "items": [] })
}

/// `GET /v1/triggers` 的投影（真行由路由从 store 读；无装配时回空表）。
pub fn triggers(items: Vec<Value>) -> Value {
    json!({ "triggers": items, "root": Value::Null })
}

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
///
/// documents 一族既在 `documents::DOCUMENT_ROUTES`（泛型 handler 的注册源），也在这里
/// 逐条登记；`documents_routes.rs` 的判据守住两张表不漂移。
pub const ROUTES: &[RegisteredRoute] = &[
    RegisteredRoute { method: "GET", path: "/v1/sessions", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/unread", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/origins", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/read/:id", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/machines", module: "device" },
    // #68：只读目录浏览（env 插件）。
    RegisteredRoute { method: "GET", path: "/v1/fs/dirs", module: "env" },
    RegisteredRoute { method: "GET", path: "/v1/models", module: "device" },
    RegisteredRoute { method: "PUT", path: "/v1/default", module: "device" },
    RegisteredRoute { method: "POST", path: "/v1/catalog/refresh", module: "device" },
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
    RegisteredRoute { method: "GET", path: "/v1/selection", module: "sessions" },
    RegisteredRoute { method: "POST", path: "/v1/selection", module: "sessions" },
    RegisteredRoute { method: "GET", path: "/v1/device", module: "device" },
    RegisteredRoute { method: "GET", path: "/v1/skills", module: "skills" },
    RegisteredRoute { method: "GET", path: "/v1/prompts", module: "prompts" },
    RegisteredRoute { method: "GET", path: "/v1/extensions", module: "ext" },
    // #85：统一搜索投影（sessions 正文 + documents），服务端一次扫描。
    RegisteredRoute { method: "GET", path: "/v1/search", module: "sessions" },
    // #57：triggers 已落地，清单打开（GET 列表 + POST 点火）。
    RegisteredRoute { method: "GET", path: "/v1/triggers", module: "triggers" },
    RegisteredRoute { method: "POST", path: "/v1/triggers/{code}/fire", module: "triggers" },
    // #58：观测读口 —— 每个触发器自己的账，可读（时间倒序）。
    RegisteredRoute { method: "GET", path: "/v1/triggers/{code}/fires", module: "triggers" },
    RegisteredRoute { method: "GET", path: "/v1/capabilities", module: "device" },
    // #62：stats 三条只读观测端点。
    RegisteredRoute { method: "GET", path: "/v1/stats/usage", module: "stats" },
    RegisteredRoute { method: "GET", path: "/v1/stats/tasks", module: "stats" },
    RegisteredRoute { method: "GET", path: "/v1/stats/db", module: "stats" },
    RegisteredRoute { method: "GET", path: "/v1/openapi.json", module: "device" },
    // #48：documents 的 12 条端点（模板原文来自 nous_documents::DOCUMENT_ROUTES）。
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}/{section}/{name}", module: "documents" },
    RegisteredRoute { method: "PUT", path: "/v1/documents/{kind}/{section}/{name}", module: "documents" },
    RegisteredRoute { method: "POST", path: "/v1/documents/{kind}/{section}", module: "documents" },
    RegisteredRoute { method: "POST", path: "/v1/documents/{kind}/{section}/{name}/rename", module: "documents" },
    RegisteredRoute { method: "POST", path: "/v1/documents/{kind}/{section}/{name}/publish", module: "documents" },
    RegisteredRoute { method: "POST", path: "/v1/documents/{kind}/{section}/{name}/reload", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}/{section}/{name}/validate", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}/{section}/{name}/history", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}/{section}/{name}/history/diff", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}/{section}/{name}/pending", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/documents/{kind}/{section}/{name}/pending/diff", module: "documents" },
    RegisteredRoute { method: "GET", path: "/v1/_routes", module: "device" },
];

/// `GET /v1/_routes`：debug 视图与 capabilities 从同一张表吐出。
///
/// `modules` 覆盖全部清单行（含未实现的，便于 desk 调试），`routes` 只列清单开着且
/// 代码注册了的端点——和 enabled 模块的 `endpoints` 多重集相等（不声明不存在的端点，
/// 冻结的 `ext` 物理路由仍在但不声明；`GET /v1/extensions` 照旧 200 空清单）。
pub fn routes_view() -> Value {
    json!({
        "routes": enabled_routes()
            .map(|route| json!({ "module": route.module, "method": route.method, "path": route.path }))
            .collect::<Vec<_>>(),
        "modules": MANIFEST
            .iter()
            .map(|(name, _)| json!({ "name": name, "enabled": route_enabled(name) }))
            .collect::<Vec<_>>(),
    })
}

/// `GET /v1/openapi.json`：从 `ROUTES` 生成 paths，与 capabilities/_routes 三处同源。
pub fn openapi_paths() -> Value {
    let mut paths = serde_json::Map::new();
    for route in enabled_routes() {
        let entry = paths.entry(route.path.to_string()).or_insert_with(|| json!({}));
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
        .find(|(name, _)| *name == module)
        .is_some_and(|(_, reason)| reason.is_none())
}

fn enabled_routes() -> impl Iterator<Item = &'static RegisteredRoute> {
    ROUTES.iter().filter(|route| route_enabled(route.module))
}

/// 清单：模块名 == `docs/plugins.md` 的 module 列（评论第二条的拍板）。
/// `enabled` = 清单 ∩ 代码：没有注册路由的模块一律 disabled + reason。
const MANIFEST: &[(&str, Option<ReasonCode>)] = &[
    ("sessions", None),
    // #68：env 的本地目录浏览已落地，SSH 远端另票。
    ("env", None),
    // #48 已落地：12 条 documents 端点在 ROUTES 里，清单也打开。
    ("documents", None),
    ("presets", Some(ReasonCode::NotImplemented)),
    ("skills", None),
    ("archive", Some(ReasonCode::NotImplemented)),
    ("combos", Some(ReasonCode::NotImplemented)),
    ("triggers", None),
    ("stats", None),
    ("device", None),
    ("prompts", None),
    ("ext", Some(ReasonCode::Frozen)),
];

#[derive(Debug, Clone, Copy)]
enum ReasonCode {
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
        .map(|(name, manifest_reason)| {
            let registered: Vec<&RegisteredRoute> = enabled_routes()
                .filter(|route| route.module == *name)
                .collect();
            // 清单关掉的模块即便有代码也不声明；清单开着但没代码就 not_implemented。
            let reason = manifest_reason.or_else(|| {
                registered
                    .is_empty()
                    .then_some(ReasonCode::NotImplemented)
            });
            match reason {
                Some(code) => json!({
                    "name": name,
                    "enabled": false,
                    "reason": { "code": code.as_str(), "message": code.message(name) },
                }),
                None => json!({
                    "name": name,
                    "enabled": true,
                    "endpoints": registered
                        .iter()
                        .map(|route| format!("{} {}", route.method, route.path))
                        .collect::<Vec<_>>(),
                }),
            }
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
