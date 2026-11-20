//! 模型编目：形状、来源、远端刷新与默认模型的原子写。
//!
//! 来源优先级：`CatalogEnvironment::inline`（`NOUS_MODELS`）> `file`（`NOUS_MODELS_FILE`）
//! > 诚实空表（`state: "missing"`）。`POST /v1/catalog/refresh` 会拉远端编目写进本地缓存，
//! 之后 file 来源就指到那份缓存。密钥只在内存里的请求头中过一手，不落盘、不进日志。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
#[serde(rename_all = "camelCase")]
pub struct CatalogDiagnostics {
    pub state: &'static str,
    pub source: Option<&'static str>,
    pub path: PathBuf,
    /// 最近一次成功刷新的时刻（epoch ms / RFC3339 字符串由调用方定，这里存原样）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    /// 最近一次刷新失败的原因；成功时为 `None`，失败时保留旧编目。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
                fetched_at: None,
                error: None,
            },
        }
    }
}

/// 编目来源的环境快照。调用方（`server`）负责读 `NOUS_*`，这里不做隐式全局读取。
/// `remote_*` 是 `POST /v1/catalog/refresh` 的入口；`cache_path` 是刷新成功后的本地落点，
/// 未给时沿用 `file_path`。密钥只在内存里（本结构体 + 请求头），任何序列化都不带它。
#[derive(Debug, Clone)]
pub struct CatalogEnvironment {
    pub inline: Option<String>,
    pub file: Option<PathBuf>,
    pub file_path: PathBuf,
    pub remote_base: Option<String>,
    pub remote_token: Option<String>,
    pub cache_path: Option<PathBuf>,
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
            remote_base: std::env::var("NOUS_CATALOG_BASE")
                .ok()
                .filter(|text| !text.trim().is_empty()),
            remote_token: std::env::var("NOUS_CATALOG_TOKEN")
                .ok()
                .filter(|text| !text.trim().is_empty()),
            cache_path: std::env::var("NOUS_MODELS_FILE")
                .ok()
                .filter(|text| !text.trim().is_empty())
                .map(PathBuf::from),
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
                fetched_at: None,
                error: None,
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
                fetched_at: None,
                error: None,
            },
        });
    }
    Ok(ModelsResponse::default())
}

/// 一次远端刷新：`GET {base}/models.json`（Bearer token 只在内存里）→ 原子写本地缓存。
///
/// 失败时不碰旧文件、不清空 items —— 调用方拿旧编目 + 本函数返回的错误继续服务。
/// token 不进返回值、不进错误文本、不写盘。
pub async fn refresh_remote_catalog(env: &CatalogEnvironment) -> Result<ModelsResponse, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| format!("构造 HTTP 客户端失败: {error}"))?;
    refresh_with_client(env, &client).await
}

/// 可注入客户端的底层入口（测试可用本地假服务/超时策略）。
pub async fn refresh_with_client(
    env: &CatalogEnvironment,
    client: &reqwest::Client,
) -> Result<ModelsResponse, String> {
    let base = env
        .remote_base
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "没有远端编目地址（NOUS_CATALOG_BASE）".to_string())?;
    let url = format!("{}/models.json", base.trim_end_matches('/'));
    let mut request = client.get(&url);
    if let Some(token) = env.remote_token.as_deref().filter(|t| !t.trim().is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| format!("拉远端编目 {url} 失败: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("远端编目 {url} 返回 HTTP {}", status.as_u16()));
    }
    let text = response.text().await.map_err(|error| format!("读远端编目 {url} 失败: {error}"))?;
    let file: CatalogFile =
        serde_json::from_str(&text).map_err(|error| format!("远端编目 {url} 不是合法 JSON: {error}"))?;
    let path = env.cache_path.as_ref().unwrap_or(&env.file_path);
    let body = serde_json::to_string_pretty(&file)
        .map_err(|error| format!("序列化远端编目: {error}"))?
        + "\n";
    let staged = path.with_extension("tmp-catalog-refresh");
    {
        let mut handle = std::fs::File::create(&staged)
            .map_err(|error| format!("创建临时文件 {}: {error}", staged.display()))?;
        handle
            .write_all(body.as_bytes())
            .and_then(|()| handle.sync_all())
            .map_err(|error| format!("写临时文件 {}: {error}", staged.display()))?;
    }
    std::fs::rename(&staged, path)
        .map_err(|error| format!("rename {} → {}: {error}", staged.display(), path.display()))?;
    let fetched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().to_string())
        .unwrap_or_default();
    Ok(ModelsResponse {
        default: file.default,
        items: file.items,
        catalog: CatalogDiagnostics {
            state: "ok",
            source: Some("remote"),
            path: path.clone(),
            fetched_at: Some(fetched_at),
            error: None,
        },
    })
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
    let items = serde_json::json!([
        {
            "name": "runtime-1",
            "at": "j1900",
            "user": null,
            "port": null,
            "ips": [],
            "addr": "",
            "note": "线上宿主；默认不收构建任务",
            "tags": ["runtime"],
            // 旧字段保留一版（同一字段不换含义）。
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
            "at": "asus",
            "user": null,
            "port": null,
            "ips": [],
            "addr": "",
            "note": "开发机，跨 target 编译",
            "tags": ["build", "runtime"],
            "hostname": "asus",
            "os": "linux",
            "arch": "x86_64",
            "triples": ["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"],
            "rustc": "1.99.1",
            "roles": ["build", "runtime"],
            "annotations": { "site": "home", "note": "开发机，跨 target 编译" }
        }
    ]);
    serde_json::json!({
        "at": "j1900",
        "machines": items,
        "items": items,
    })
}

/// `GET /v1/device` 的 device 自己那一半（版本 / 构建身份由 server 填）。
pub fn device(models: &ModelsResponse) -> Value {
    let default = models.default.as_ref().map(|model| {
        json!({ "provider": model.provider, "modelId": model.model_id })
    });
    json!({
        "self": std::env::var("NOUS_SELF").ok(),
        "origin": std::env::var("NOUS_ORIGIN").ok(),
        "node": env!("CARGO_PKG_VERSION"),
        "version": env!("CARGO_PKG_VERSION"),
        "startedAt": started_at(),
        "model": default,
        "faux": false,
        "data": std::env::var("NOUS_DATA").ok(),
        "catalog": serde_json::to_value(&models.catalog).unwrap_or(Value::Null),
        "available": models.items.iter().map(|item| {
            format!("{}/{}", item.provider, item.model_id)
        }).collect::<Vec<_>>(),
    })
}

fn started_at() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
