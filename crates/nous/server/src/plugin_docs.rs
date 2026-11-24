//! `GET /v1/plugins/{name}/readme`：插件「人话 README」的只读口。
//!
//! 文档**不进二进制**：源码里的 `crates/<vendor>/<module>/README.md` 由 `scripts/deploy.sh`
//! 随产物发布到 `<产物目录>/docs/plugins/<module>.md`，这里运行时读盘（mtime + 长度做缓存
//! 键）。于是改文档只要同步这一个目录，不用重编译、不用发版 —— 取向见 `docs/plugins.md`
//! 规则 7 与 `decisions.md` 2026-11-24 那条。
//!
//! 目录顺序：`NOUS_DOCS_DIR`（部署脚本/运维显式指定）→ `<exe 所在目录>/docs/plugins`
//! （跟着版本目录走，回退旧版本时文档也一起回退）。
//!
//! 名字只认 `meta::MANIFEST` 的 module 列：不是白名单里的名字一律 404，
//! 顺带把 `..` / 绝对路径这类路径穿越挡在门外（文件名由我们拼，不接用户输入）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::app::AppState;

/// 一页 README 的缓存：mtime + 长度没变就不重读。
struct Cached {
    stamp: (Option<SystemTime>, u64),
    markdown: String,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 文档根目录。`NOUS_DOCS_DIR` 优先，否则跟着产物走。
pub fn docs_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOUS_DOCS_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("docs/plugins")))
        .unwrap_or_else(|| PathBuf::from("docs/plugins"))
}

/// 模块名 → 文档文件。名字必须在 MANIFEST 里，否则 `None`（= 404）。
fn doc_path(root: &Path, module: &str) -> Option<PathBuf> {
    if !crate::meta::MANIFEST.iter().any(|entry| entry.name == module) {
        return None;
    }
    Some(root.join(format!("{module}.md")))
}

/// 读一页文档（带缓存）。返回 `None` = 文件不在或读不出来，由调用方决定 404 的措辞。
fn read_doc(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.modified().ok(), meta.len());
    if let Some(cached) = cache().lock().ok()?.get(path) {
        if cached.stamp == stamp {
            return Some(cached.markdown.clone());
        }
    }
    let markdown = std::fs::read_to_string(path).ok()?;
    cache().lock().ok()?.insert(
        path.to_path_buf(),
        Cached {
            stamp,
            markdown: markdown.clone(),
        },
    );
    Some(markdown)
}

/// `GET /v1/plugins/{name}/readme`
///
/// 200：`{ module, version, markdown, bytes }`。
/// 404 `unknown_module`：名字不在 MANIFEST（拼错、或不是插件）。
/// 404 `readme_not_installed`：名单里有，但产物里没带这份文档（部署漏同步 / 老产物）。
pub async fn readme(
    State(_state): State<std::sync::Arc<AppState>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    let root = docs_root();
    let Some(path) = doc_path(&root, &name) else {
        return not_found(
            "unknown_module",
            &name,
            format!("`{name}` 不在 capabilities 的模块清单里"),
        );
    };
    let Some(markdown) = read_doc(&path) else {
        return not_found(
            "readme_not_installed",
            &name,
            format!("产物里没有这份文档：{}（跑 scripts/deploy.sh --docs-only 同步）", path.display()),
        );
    };
    Json(serde_json::json!({
        "module": name,
        "version": env!("CARGO_PKG_VERSION"),
        "markdown": markdown,
        "bytes": markdown.len(),
    }))
    .into_response()
}

fn not_found(code: &str, module: &str, hint: String) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": code,
            "module": module,
            "hint": hint,
        })),
    )
        .into_response()
}
