//! #61 combos：组合（终极模板里「触发器 + 会话模板」那一条）的只读投影 + 开关写穿。
//!
//! 这一层是 app 层的文件系统逻辑：名册由扫 `<root>/<section>/<name>/preset.json` 得出，
//! 与 presets 共用数据布局，但**没有**进程内常驻名单。每次请求都重扫盘面 —— 人手改文件
//! 也算数。`enabled` 由引擎算：`spec.enabled !== false` 且引用的 trigger / session 都在；
//! 客户端只显示 + 写穿，不自己拼规则。
//!
//! 写穿只动 `spec.enabled` 一位。幂等回执存在 `<root>/.combos-receipts/`（点开头，扫名册
//! 时会跳过），不另开一份「开关名单」。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};

pub const SECTIONS: &[&str] = &["Session-Template", "Ultimate-Template"];

/// 触发器名册根：只有这一个读取点（默认 `/opt/Nous-Trigger`，与旧 TS 版一致）。
pub const TRIGGER_VAR: &str = "NOUS_TRIGGER";
pub const TRIGGER_DEFAULT: &str = "/opt/Nous-Trigger";

pub fn triggers_root() -> std::path::PathBuf {
    std::env::var_os(TRIGGER_VAR)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(TRIGGER_DEFAULT))
}

/// 目录名（不是 section 名）：扫名册时跳过，不是一条组合。
const RECEIPTS_DIR: &str = ".combos-receipts";

/// 一份组合的盘面原文（不缓存）。
#[derive(Debug, Clone)]
pub(crate) struct ComboFile {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) body: Value,
}

impl ComboFile {
    fn version(&self) -> String {
        format!("v{}", &crate::version::bytes_to_hex(&crate::version::sha256(&self.bytes))[..12])
    }
}

/// 一条组合的对外形状。`unknown` 里收着引擎以后加的字段，原样透传。
pub(crate) fn view(file: &ComboFile, triggers: &BTreeSet<String>, sessions: &BTreeSet<String>) -> Value {
    let spec = file.body.get("spec").cloned().unwrap_or(Value::Null);
    let mut problems = Vec::new();
    let trigger = spec.get("trigger").and_then(Value::as_str).unwrap_or("").to_string();
    let session = spec.get("session").and_then(Value::as_str).unwrap_or("").to_string();
    let params = spec.get("params").cloned().unwrap_or_else(|| json!({}));
    if trigger.is_empty() {
        problems.push("这条没钉触发器".to_string());
    } else if !triggers.contains(&trigger) {
        problems.push(format!("引用的 trigger `{trigger}` 不在了"));
    }
    if session.is_empty() {
        problems.push("这条没钉会话模板".to_string());
    } else if !sessions.contains(&session) {
        problems.push(format!("引用的会话模板 `{session}` 不在了"));
    }
    let explicit = spec.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let mut out = json!({
        "name": file.name,
        "trigger": if trigger.is_empty() { Value::Null } else { Value::String(trigger.clone()) },
        "params": params,
        "session": if session.is_empty() { Value::Null } else { Value::String(session.clone()) },
        "enabled": explicit && problems.is_empty(),
        "problem": problems.join("；"),
        "version": file.version(),
    });
    // 未知字段原样透传：引擎加字段不该让我们编译失败，也不该在这里被吞掉。
    if let Some(object) = out.as_object_mut() {
        if let Some(spec_object) = spec.as_object() {
            for (key, value) in spec_object {
                if !object.contains_key(key) {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
    }
    out
}

/// 扫描连接器：根来自装配（生产从 `NOUS_PRESETS` / `NOUS_TRIGGER` 读，测试直接注）。
#[derive(Debug, Clone)]
pub struct ComboStore {
    root: PathBuf,
    triggers: PathBuf,
}

impl ComboStore {
    pub fn new(root: impl Into<PathBuf>, triggers: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), triggers: triggers.into() }
    }

    fn section_path(&self, section: &str) -> Result<PathBuf> {
        if !SECTIONS.contains(&section) {
            return Err(ServerError::BadRequest(format!(
                "不认识的 section: {section}（只有 {}）", SECTIONS.join(", ")
            )));
        }
        Ok(self.root.join(section))
    }

    fn file_path(&self, section: &str, name: &str) -> Result<PathBuf> {
        if name.is_empty()
            || Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name)
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(ServerError::BadRequest(format!("非法的 name: {name:?}")));
        }
        Ok(self.section_path(section)?.join(name).join("preset.json"))
    }

    fn scan(&self, section: &str) -> Result<Vec<ComboFile>> {
        let dir = self.section_path(section)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(ServerError::Internal(format!("扫 {section} 失败: {error}"))),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let bytes = match fs::read(entry.path().join("preset.json")) {
                Ok(bytes) => bytes,
                Err(_) => continue, // 没有 preset.json 的目录不是一条组合
            };
            let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            out.push(ComboFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes,
                body,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub(crate) fn one(&self, section: &str, name: &str) -> Result<ComboFile> {
        let path = self.file_path(section, name)?;
        let bytes = fs::read(&path)
            .map_err(|_| ServerError::NotFound(format!("组合不存在: {section}/{name}")))?;
        let body = serde_json::from_slice(&bytes)
            .map_err(|error| ServerError::BadRequest(format!("preset.json 坏了: {error}")))?;
        Ok(ComboFile { name: name.to_string(), bytes, body })
    }

    /// 触发器名册：`<NOUS_TRIGGER>/<code>/` 下有目录就算存在。
    pub(crate) fn trigger_names(&self) -> BTreeSet<String> {
        fs::read_dir(&self.triggers)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 会话模板名册：与 presets 共用数据根，从盘现扫（不读 durable 库）。
    pub(crate) fn session_names(&self) -> Result<BTreeSet<String>> {
        Ok(self.scan("Session-Template")?.into_iter().map(|file| file.name).collect())
    }

    fn roster(&self, section: &str) -> Result<Value> {
        let triggers = self.trigger_names();
        let sessions = self.session_names()?;
        let items = self.scan(section)?
            .iter()
            .map(|file| view(file, &triggers, &sessions))
            .collect::<Vec<_>>();
        Ok(json!({ "combos": items }))
    }

    /// 写穿 `spec.enabled` 一位；只动这一条组合的这一位。
    fn patch(&self, section: &str, name: &str, enabled: bool, if_match: &str, request_id: &str) -> Result<Value> {
        // 幂等优先于版本号：客户端拿第一次的 If-Match 重放同一 requestId 时，
        // 盘面已经变了也不能再回 412 —— 那等于让幂等锚失效（坑 99）。
        if let Some(previous) = self.receipt(section, name, request_id)? {
            return Ok(previous);
        }
        let file = self.one(section, name)?;
        if file.version() != if_match {
            return Err(ServerError::PreconditionFailed(format!(
                "版本不匹配：期望 {if_match}，盘上是 {}",
                file.version()
            )));
        }
        let mut body = file.body.clone();
        let spec = body
            .as_object_mut()
            .and_then(|object| object.get_mut("spec"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| ServerError::BadRequest("preset.json 没有 spec 对象".to_string()))?;
        spec.insert("enabled".to_string(), json!(enabled));
        let bytes = serde_json::to_vec_pretty(&body)
            .map_err(|error| ServerError::Internal(format!("序列化 preset.json: {error}")))?;
        let path = self.file_path(section, name)?;
        let dir = path.parent().ok_or_else(|| ServerError::Internal("没有父目录".into()))?;
        let temp = dir.join(".preset.json.tmp");
        fs::write(&temp, &bytes)
            .map_err(|error| ServerError::Internal(format!("写临时文件失败: {error}")))?;
        fs::rename(&temp, &path)
            .map_err(|error| ServerError::Internal(format!("原子替换失败: {error}")))?;
        let written = ComboFile { name: name.to_string(), bytes, body };
        let triggers = self.trigger_names();
        let sessions = self.session_names()?;
        let mut receipt = view(&written, &triggers, &sessions);
        receipt["requestId"] = json!(request_id);
        self.remember(section, name, request_id, &receipt)?;
        Ok(receipt)
    }

    /// #82：动作（fire / launch）的幂等回执，与 PATCH 共用同一份盘面目录。
    pub(crate) fn action_receipt(&self, name: &str, request_id: &str) -> Result<Option<Value>> {
        self.receipt("Ultimate-Template", name, request_id)
    }

    pub(crate) fn remember_action(&self, name: &str, request_id: &str, receipt: &Value) -> Result<()> {
        self.remember("Ultimate-Template", name, request_id, receipt)
    }

    fn receipt_dir(&self, section: &str, name: &str) -> PathBuf {
        self.root.join(RECEIPTS_DIR).join(section).join(name)
    }

    fn receipt(&self, section: &str, name: &str, request_id: &str) -> Result<Option<Value>> {
        let path = self.receipt_dir(section, name).join(format!("{request_id}.json"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ServerError::Internal(format!("读回执失败: {error}"))),
        };
        let value = serde_json::from_slice(&bytes)
            .map_err(|error| ServerError::Internal(format!("回执坏了: {error}")))?;
        Ok(Some(value))
    }

    fn remember(&self, section: &str, name: &str, request_id: &str, receipt: &Value) -> Result<()> {
        let dir = self.receipt_dir(section, name);
        fs::create_dir_all(&dir)
            .map_err(|error| ServerError::Internal(format!("建回执目录失败: {error}")))?;
        fs::write(dir.join(format!("{request_id}.json")), serde_json::to_vec(receipt).unwrap())
            .map_err(|error| ServerError::Internal(format!("记回执失败: {error}")))?;
        Ok(())
    }
}

pub(crate) fn combo_store(state: &AppState) -> Result<&ComboStore> {
    state.combos().ok_or_else(|| ServerError::Conflict("combos not configured".to_string()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchBody {
    enabled: bool,
    request_id: Option<String>,
}

/// `GET /v1/combos/:section`：每次请求都重扫盘面。
pub async fn list(State(state): State<std::sync::Arc<AppState>>, AxumPath((section,)): AxumPath<(String,)>) -> Result<Json<Value>> {
    Ok(Json(combo_store(&state)?.roster(&section)?))
}

/// `PATCH /v1/combos/:section/:name`：只改 `{enabled}`，带 If-Match + requestId。
pub async fn patch(
    State(state): State<std::sync::Arc<AppState>>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    if body.is_empty() {
        return Err(ServerError::BadRequest("enabled and requestId are required".to_string()));
    }
    let body: PatchBody = serde_json::from_slice(&body)
        .map_err(|error| ServerError::BadRequest(format!("patch body: {error}")))?;
    let request_id = body
        .request_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::BadRequest("requestId is required".to_string()))?;
    let if_match = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ServerError::PreconditionFailed("If-Match is required".to_string()))?;
    let receipt = combo_store(&state)?.patch(&section, &name, body.enabled, if_match, &request_id)?;
    Ok(Json(receipt))
}
