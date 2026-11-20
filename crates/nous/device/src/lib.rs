//! 模型编目：形状、来源与默认模型的原子写。
//!
//! 来源优先级：`CatalogEnvironment::inline`（`NOUS_MODELS`）> `file`（`NOUS_MODELS_FILE`）
//! > 诚实空表（`state: "missing"`）。这里只认文件系统与传入的字符串，不读进程环境。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 默认编目路径（`NOUS_MODELS_FILE` 未给时），也是诊断块里那条可诊断的路径。
pub const DEFAULT_MODELS_FILE: &str = "/var/lib/nous/models.json";

/// `GET /v1/models` 的 `items[]` 一项。字段名就是 wire 形状，不许改成 snake_case。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelItem {
    pub provider: String,
    pub model_id: String,
    pub name: String,
    pub thinking_levels: Vec<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
}

/// `default` / `PUT /v1/default` 请求里的模型引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CatalogFile {
    default: Option<ModelRef>,
    items: Vec<ModelItem>,
}

/// `GET /v1/models` 的诊断块：编目到底从哪来，没来时该看哪个文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogDiagnostics {
    pub state: &'static str,
    pub source: Option<&'static str>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelsResponse {
    pub default: Option<ModelRef>,
    pub items: Vec<ModelItem>,
    pub catalog: CatalogDiagnostics,
}

impl Default for ModelsResponse {
    fn default() -> Self {
        Self {
            default: None,
            items: Vec::new(),
            catalog: CatalogDiagnostics {
                state: "missing",
                source: None,
                path: PathBuf::from(DEFAULT_MODELS_FILE),
            },
        }
    }
}

/// 编目来源的环境快照。调用方（`server`）负责读 `NOUS_*`，这里不做隐式全局读取。
#[derive(Debug, Clone)]
pub struct CatalogEnvironment {
    pub inline: Option<String>,
    pub file: Option<PathBuf>,
    pub file_path: PathBuf,
}

impl CatalogEnvironment {
    /// 从进程环境取一次快照（应用入口用一次，测试用结构体字面量）。
    pub fn from_env() -> Self {
        Self {
            inline: std::env::var("NOUS_MODELS")
                .ok()
                .filter(|text| !text.trim().is_empty()),
            file: std::env::var("NOUS_MODELS_FILE")
                .ok()
                .filter(|text| !text.trim().is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_file()),
            file_path: std::env::var("NOUS_MODELS_FILE")
                .ok()
                .filter(|text| !text.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_MODELS_FILE)),
        }
    }
}

fn parse_inline(text: &str) -> Result<CatalogFile, String> {
    serde_json::from_str(text).map_err(|error| format!("NOUS_MODELS 不是合法编目 JSON: {error}"))
}

/// 读一份编目。inline 优先于文件；两者都没有就是诚实空表。
pub fn load_models(env: &CatalogEnvironment) -> Result<ModelsResponse, String> {
    if let Some(text) = &env.inline {
        let file = parse_inline(text)?;
        return Ok(ModelsResponse {
            default: file.default,
            items: file.items,
            catalog: CatalogDiagnostics {
                state: "ok",
                source: Some("env"),
                path: env.file_path.clone(),
            },
        });
    }
    if let Some(path) = &env.file {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("读模型编目 {}: {error}", path.display()))?;
        let file: CatalogFile = serde_json::from_str(&text)
            .map_err(|error| format!("模型编目 {} 不是合法 JSON: {error}", path.display()))?;
        return Ok(ModelsResponse {
            default: file.default,
            items: file.items,
            catalog: CatalogDiagnostics {
                state: "ok",
                source: Some("file"),
                path: path.clone(),
            },
        });
    }
    Ok(ModelsResponse::default())
}

/// 写一个新 default：先校验模型在 items 里，再在目标旁写临时文件，最后 rename。
pub fn save_default(path: &Path, model: &ModelRef) -> Result<ModelRef, String> {
    let staged = stage_default(path, model)?;
    std::fs::rename(&staged, path)
        .map_err(|error| format!("rename {} → {}: {error}", staged.display(), path.display()))?;
    Ok(model.clone())
}

/// 原子写的第一步：只写临时文件，不碰目标。测试用它模拟「rename 前被杀」。
pub fn stage_default(path: &Path, model: &ModelRef) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("读模型编目 {}: {error}", path.display()))?;
    let mut file: CatalogFile = serde_json::from_str(&text)
        .map_err(|error| format!("模型编目 {} 不是合法 JSON: {error}", path.display()))?;
    let exists = file.items.iter().any(|item| {
        item.provider == model.provider && item.model_id == model.model_id
    });
    if !exists {
        return Err(format!(
            "模型 {}/{} 不在编目 items 里",
            model.provider, model.model_id
        ));
    }
    file.default = Some(model.clone());
    let body = serde_json::to_string_pretty(&file)
        .map_err(|error| format!("序列化模型编目: {error}"))?
        + "\n";
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let staged = path.with_extension(format!("tmp-{nonce}"));
    let mut handle = std::fs::File::create(&staged)
        .map_err(|error| format!("创建临时文件 {}: {error}", staged.display()))?;
    handle
        .write_all(body.as_bytes())
        .and_then(|()| handle.sync_all())
        .map_err(|error| format!("写临时文件 {}: {error}", staged.display()))?;
    Ok(staged)
}

/// 机器编目：从 `NOUS_MACHINES_FILE` 给的 JSON 读；没有文件就用内置两台。
pub fn machines(file: Option<&Path>) -> serde_json::Value {
    if let Some(path) = file {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                return value;
            }
        }
    }
    builtin_machines()
}

fn builtin_machines() -> serde_json::Value {
    serde_json::json!({
        "items": [
            {
                "name": "runtime-1",
                "hostname": "j1900",
                "os": "linux",
                "arch": "x86_64",
                "triples": ["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"],
                "rustc": "1.99.1",
                "roles": ["runtime"],
                "annotations": { "site": "home", "note": "线上宿主；默认不收构建任务" }
            },
            {
                "name": "asus",
                "hostname": "asus",
                "os": "linux",
                "arch": "x86_64",
                "triples": ["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"],
                "rustc": "1.99.1",
                "roles": ["build", "runtime"],
                "annotations": { "site": "home", "note": "开发机，跨 target 编译" }
            }
        ]
    })
}
