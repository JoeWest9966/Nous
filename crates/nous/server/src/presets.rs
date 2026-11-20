//! #60 presets：模板 / 预设的资源化端点（13 条）+ 引擎内修订表。
//!
//! 盘是唯一真相：`<root>/<section>/<name>/preset.json`，进程内**没有**常驻 map。
//! 每次读都扫目录；每次写都整段串行锁，然后重扫。修订只落 `nous-plugins.db` 的
//! `presets_revisions`，不 spawn git（#75 的私库 git 历史是另一件事）。
//!
//! 这一层在 server 里，是因为 `docs/plugins.md` 的 `presets` 行是声明式（无独立 crate）；
//! 它依旧守层：IO 只走这里（app），kernel 不认识模板。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Result, ServerError};

pub const SECTIONS: &[&str] = &["Session-Template", "Ultimate-Template"];

/// 整段一把锁：写端点串行，写完立即重扫该段索引（这里没有索引，roster 每次现扫）。
pub struct PresetStore {
    root: PathBuf,
    db: PathBuf,
    lock: Mutex<()>,
}

#[derive(Debug, Clone)]
enum WriteKind {
    Put,
    Patch,
    Delete,
    Copy,
    Order,
    Rollback,
}

impl WriteKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
            Self::Copy => "copy",
            Self::Order => "order",
            Self::Rollback => "rollback",
        }
    }
}

/// 一次写操作的回执：同一 requestId 重放回第一次的结果，version 不涨。
#[derive(Debug, Clone)]
struct Receipt {
    version: i64,
    body: Value,
    status: StatusCode,
}

#[derive(Debug, Clone)]
struct PresetFile {
    name: String,
    body: Value,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Revision {
    n: i64,
    sha: String,
    body: String,
    actor: String,
    at: i64,
    request_id: String,
}

impl PresetStore {
    pub fn new(root: PathBuf, db: PathBuf) -> Self {
        Self { root, db, lock: Mutex::new(()) }
    }

    fn section_path(&self, section: &str) -> Result<PathBuf> {
        if !SECTIONS.contains(&section) {
            return Err(ServerError::BadRequest(format!(
                "不认识的 section: {section}（只有 {}）",
                SECTIONS.join(", ")
            )));
        }
        Ok(self.root.join(section))
    }

    fn preset_path(&self, section: &str, name: &str) -> Result<PathBuf> {
        let clean = Path::new(name).file_name().and_then(|n| n.to_str());
        if name.is_empty()
            || clean != Some(name)
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(ServerError::BadRequest(format!("非法的 name: {name:?}")));
        }
        Ok(self.section_path(section)?.join(name).join("preset.json"))
    }

    /// 扫目录，不读缓存：盘上有目录 = 这条存在。`problem` 非空 = 会话创建时不列它。
    fn scan(&self, section: &str) -> Result<Vec<PresetFile>> {
        let dir = self.section_path(section)?;
        let mut out = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(error) => return Err(ServerError::Internal(format!("扫 {section} 失败: {error}"))),
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = dir.join(&name).join("preset.json");
            let Ok(bytes) = fs::read(&path) else { continue };
            let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            out.push(PresetFile { name, body, bytes });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn one(&self, section: &str, name: &str) -> Result<PresetFile> {
        let path = self.preset_path(section, name)?;
        let bytes = fs::read(&path)
            .map_err(|_| ServerError::NotFound(format!("preset 不存在: {section}/{name}")))?;
        let body = serde_json::from_slice(&bytes)
            .map_err(|error| ServerError::BadRequest(format!("preset.json 坏了: {error}")))?;
        Ok(PresetFile { name: name.to_string(), body, bytes })
    }

    /// schema 在 `nous_store::Store::open` 时已迁移；这里只负责读写，不重开迁移链。
    fn connect(&self) -> Result<rusqlite::Connection> {
        rusqlite::Connection::open(&self.db)
            .map_err(|error| ServerError::Internal(format!("打开插件库失败: {error}")))
    }

    fn receipt(&self, section: &str, name: &str, kind: &WriteKind, request_id: &str) -> Result<Option<Receipt>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT body FROM preset_write_receipts WHERE section=?1 AND name=?2 AND kind=?3 AND request_id=?4",
        ).map_err(|error| ServerError::Internal(format!("读回执失败: {error}")))?;
        let rows: Vec<String> = stmt
            .query_map(params![section, name, kind.as_str(), request_id], |row| row.get(0))
            .map_err(|error| ServerError::Internal(format!("读回执失败: {error}")))?
            .filter_map(|row| row.ok())
            .collect();
        match rows.first() {
            None => Ok(None),
            Some(text) => {
                let value: Value = serde_json::from_str(text)
                    .map_err(|error| ServerError::Internal(format!("回执坏了: {error}")))?;
                let version = value["version"].as_i64().unwrap_or(0);
                let status = match value["status"].as_i64() {
                    Some(201) => StatusCode::CREATED,
                    Some(200) => StatusCode::OK,
                    _ => StatusCode::OK,
                };
                Ok(Some(Receipt { version, body: value, status }))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn remember(
        &self,
        section: &str,
        name: &str,
        kind: &WriteKind,
        request_id: &str,
        body: &Value,
    ) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR IGNORE INTO preset_write_receipts (section, name, kind, request_id, body) VALUES (?1,?2,?3,?4,?5)",
            params![section, name, kind.as_str(), request_id, body.to_string()],
        )
        .map_err(|error| ServerError::Internal(format!("记回执失败: {error}")))?;
        Ok(())
    }

    fn revisions(&self, section: &str, name: &str) -> Result<Vec<Revision>> {
        let conn = self.connect()?;
        let rows = {
            let mut stmt = conn.prepare(
                "SELECT n, sha, body, actor, at, request_id FROM presets_revisions WHERE section=?1 AND name=?2 ORDER BY n",
            ).map_err(|error| ServerError::Internal(format!("读修订失败: {error}")))?;
            let iter = stmt.query_map(params![section, name], |row| {
                Ok(Revision {
                    n: row.get(0)?,
                    sha: row.get(1)?,
                    body: row.get(2)?,
                    actor: row.get(3)?,
                    at: row.get(4)?,
                    request_id: row.get(5)?,
                })
            }).map_err(|error| ServerError::Internal(format!("读修订失败: {error}")))?;
            iter.collect::<std::result::Result<Vec<_>, _>>()
        };
        rows.map_err(|error| ServerError::Internal(format!("读修订失败: {error}")))
    }

    /// 写一次修订：n = 当前最大 n + 1，整条在一个事务里，原子。
    fn append_revision(&self, section: &str, name: &str, bytes: &[u8], request_id: &str) -> Result<i64> {
        let conn = self.connect()?;
        let tx = conn.unchecked_transaction()
            .map_err(|error| ServerError::Internal(format!("开修订事务失败: {error}")))?;
        let n: i64 = tx.query_row(
            "SELECT COALESCE(MAX(n), 0) + 1 FROM presets_revisions WHERE section=?1 AND name=?2",
            params![section, name],
            |row| row.get(0),
        ).map_err(|error| ServerError::Internal(format!("算修订号失败: {error}")))?;
        tx.execute(
            "INSERT INTO presets_revisions (section,name,n,sha,body,actor,at,request_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                section, name, n,
                sha256_hex(bytes),
                String::from_utf8_lossy(bytes).into_owned(),
                "local",
                now_ms(),
                request_id,
            ],
        ).map_err(|error| ServerError::Internal(format!("写修订失败: {error}")))?;
        tx.commit()
            .map_err(|error| ServerError::Internal(format!("提交修订失败: {error}")))?;
        Ok(n)
    }

    /// 原子写盘：临时文件 + rename，写完的字节就是之后 roster 的 sha 来源。
    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let dir = path.parent().ok_or_else(|| ServerError::Internal("没有父目录".into()))?;
        fs::create_dir_all(dir)
            .map_err(|error| ServerError::Internal(format!("建目录失败: {error}")))?;
        let temp = dir.join(".preset.json.tmp");
        {
            let mut file = fs::File::create(&temp)
                .map_err(|error| ServerError::Internal(format!("建临时文件失败: {error}")))?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| ServerError::Internal(format!("写临时文件失败: {error}")))?;
        }
        fs::rename(&temp, path)
            .map_err(|error| ServerError::Internal(format!("原子替换失败: {error}")))?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// 文件字节的 sha256：复用 `crate::version` 里那份标准实现，不另抄一份（抄错过一次）。
fn sha256_hex(input: &[u8]) -> String {
    let digest = crate::version::sha256(input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 校验：纯配置 JSON，不许有正文 / frontmatter。返回规范化后的字节（末尾换行）。
fn validate_body(value: &Value) -> Result<Vec<u8>> {
    let object = value.as_object().ok_or_else(|| {
        ServerError::BadRequest("preset.json 必须是 JSON 对象".to_string())
    })?;
    if object.contains_key("content") || object.contains_key("frontmatter") {
        return Err(ServerError::BadRequest(
            "preset.json 是纯配置：不许带 content / frontmatter".to_string(),
        ));
    }
    for key in ["name", "summary", "category", "spec"] {
        if !object.contains_key(key) {
            return Err(ServerError::BadRequest(format!("preset.json 缺字段 {key}")));
        }
    }
    if !object["name"].is_string() || object["name"].as_str().is_none_or(str::is_empty) {
        return Err(ServerError::BadRequest("preset.json 的 name 必须是非空字符串".to_string()));
    }
    if !object["category"].is_array()
        || !object["category"].as_array().unwrap().iter().all(Value::is_string)
    {
        return Err(ServerError::BadRequest("preset.json 的 category 必须是 string[]".to_string()));
    }
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| ServerError::Internal(format!("序列化失败: {error}")))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// 一条 roster 项：清单即详情。`problem` 非空 = 这条坏了，会话创建时不列它。
fn roster_item(file: &PresetFile, order: &BTreeMap<String, i64>, archived: bool, version: i64) -> Value {
    let problem = if file.body.is_null() {
        Some("preset.json 解析失败".to_string())
    } else {
        None
    };
    json!({
        "name": file.name,
        "sha": sha256_hex(&file.bytes),
        "version": version,
        "category": file.body.get("category").cloned().unwrap_or(json!([])),
        "summary": file.body.get("summary").cloned().unwrap_or(Value::Null),
        "description": file.body.get("description").cloned().unwrap_or(Value::Null),
        "spec": file.body.get("spec").cloned().unwrap_or(Value::Null),
        "order": order.get(&file.name).copied().unwrap_or(i64::MAX),
        "archived": archived,
        "problem": problem,
    })
}

fn load_order(dir: &Path) -> BTreeMap<String, i64> {
    let path = dir.join("order.json");
    let Ok(bytes) = fs::read(path) else { return BTreeMap::new() };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { return BTreeMap::new() };
    let Some(list) = value.get("order").and_then(Value::as_array) else { return BTreeMap::new() };
    list.iter()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(i, name)| (name.to_string(), i as i64))
        .collect()
}

fn load_archived(dir: &Path) -> Vec<String> {
    let path = dir.join("archived.json");
    let Ok(bytes) = fs::read(path) else { return vec![] };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get("archived").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn save_names(dir: &Path, file: &str, key: &str, names: &[String]) -> Result<()> {
    fs::create_dir_all(dir).map_err(|error| ServerError::Internal(format!("建目录失败: {error}")))?;
    let path = dir.join(file);
    let bytes = serde_json::to_vec(&json!({ key: names }))
        .map_err(|error| ServerError::Internal(format!("序列化失败: {error}")))?;
    let temp = dir.join(format!(".{file}.tmp"));
    {
        let mut handle = fs::File::create(&temp)
            .map_err(|error| ServerError::Internal(format!("建临时文件失败: {error}")))?;
        handle.write_all(&bytes)
            .and_then(|_| handle.sync_all())
            .map_err(|error| ServerError::Internal(format!("写临时文件失败: {error}")))?;
    }
    fs::rename(temp, path).map_err(|error| ServerError::Internal(format!("原子替换失败: {error}")))?;
    Ok(())
}

/// 把状态装进 AppState 的字段里（只有 server 这一层需要）。
pub type SharedStore = std::sync::Arc<PresetStore>;

// ── HTTP handlers ──────────────────────────────────────────────────────────

/// `GET /v1/presets`：段清单，每段带 count（不硬编码段名到客户端）。
pub async fn sections(State(store): State<SharedStore>) -> Result<Json<Value>> {
    let mut items = Vec::new();
    for section in SECTIONS {
        let count = store.scan(section)?.len();
        items.push(json!({ "section": section, "count": count }));
    }
    Ok(Json(json!({ "sections": items })))
}

/// `GET /v1/presets/model`：预设能引用的模型清单（只读，沿用原状）。
pub async fn model(State(store): State<SharedStore>) -> Result<Json<Value>> {
    let path = store.root.join("models.json");
    let value = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(json!({ "models": [] })),
        Err(_) => json!({ "models": [] }),
    };
    Ok(Json(value))
}

/// `GET /v1/presets/:section`：roster，清单即详情。**每次现扫盘**，不返回进程内常量。
pub async fn roster(
    State(store): State<SharedStore>,
    AxumPath(section): AxumPath<String>,
) -> Result<Json<Value>> {
    let _ = store.section_path(&section)?;
    let dir = store.root.join(&section);
    let order = load_order(&dir);
    let archived = load_archived(&dir);
    let mut items = Vec::new();
    for file in store.scan(&section)? {
        let version = store
            .revisions(&section, &file.name)?
            .last()
            .map(|revision| revision.n)
            .unwrap_or(0);
        items.push(roster_item(
            &file,
            &order,
            archived.contains(&file.name),
            version,
        ));
    }
    Ok(Json(json!({ "section": section, "items": items })))
}

/// `GET /v1/presets/:section/categories`：段内类别名册（含零计数类别）。
pub async fn categories(
    State(store): State<SharedStore>,
    AxumPath(section): AxumPath<String>,
) -> Result<Json<Value>> {
    let dir = store.section_path(&section)?;
    let mut names: Vec<String> = Vec::new();
    if let Ok(bytes) = fs::read(dir.join("categories.json")) {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(list) = value.as_array().or_else(|| value.get("categories").and_then(Value::as_array)) {
                names.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
            }
        }
    }
    for file in store.scan(&section)? {
        if let Some(list) = file.body.get("category").and_then(Value::as_array) {
            for category in list.iter().filter_map(Value::as_str) {
                if !names.iter().any(|name| name == category) {
                    names.push(category.to_string());
                }
            }
        }
    }
    names.sort();
    Ok(Json(json!({ "section": section, "categories": names })))
}

/// `GET /v1/presets/:section/:name`：原文 + ETag（= version）。
pub async fn get_preset(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
) -> Result<Response> {
    let file = store.one(&section, &name)?;
    let version = store
        .revisions(&section, &name)?
        .last()
        .map(|revision| revision.n)
        .unwrap_or(0);
    let body = json!({
        "section": section,
        "name": name,
        "version": version,
        "sha": sha256_hex(&file.bytes),
        "preset": file.body,
    });
    let mut response = Json(body).into_response();
    response.headers_mut().insert(
        header::ETAG,
        header::HeaderValue::from_str(&version.to_string())
            .map_err(|error| ServerError::Internal(format!("ETag: {error}")))?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
pub struct PutQuery {
    dry_run: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteBody {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(flatten)]
    preset: Value,
}

fn request_id_of(body: &WriteBody) -> Result<String> {
    let id = body.request_id.clone().filter(|value| !value.is_empty()).ok_or_else(|| {
        ServerError::BadRequest("requestId is required".to_string())
    })?;
    Ok(id)
}

/// `PUT /v1/presets/:section/:name`：建或覆盖；dry_run=1 只校验不落盘。
pub async fn put_preset(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    Query(query): Query<PutQuery>,
    headers: HeaderMap,
    Json(body): Json<WriteBody>,
) -> Result<Response> {
    let _ = store.preset_path(&section, &name)?;
    let request_id = request_id_of(&body)?;
    let bytes = validate_body(&body.preset)?;
    if query.dry_run.as_deref() == Some("1") {
        return Ok((StatusCode::OK, Json(json!({ "valid": true, "wrote": false }))).into_response());
    }
    let guard = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    drop(guard);
    // 真写走串行段：用 store 的自有 mutex 锁住整个决策 + 落盘 + 修订。
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section, &name, &WriteKind::Put, &request_id)? {
        return Ok((receipt.status, Json(receipt.body)).into_response());
    }
    let if_match = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    let existing = fs::read(store.preset_path(&section, &name)?).ok();
    let version = store.revisions(&section, &name)?.last().map(|r| r.n).unwrap_or(0);
    if let Some(expected) = if_match {
        if existing.is_none() || expected.trim_matches('"') != version.to_string() {
            return Err(ServerError::PreconditionFailed(format!(
                "If-Match 不匹配：要 {expected}，盘上是 {version}"
            )));
        }
    }
    let path = store.preset_path(&section, &name)?;
    store.write_file(&path, &bytes)?;
    let version = store.append_revision(&section, &name, &bytes, &request_id)?;
    let response = json!({
        "section": section,
        "name": name,
        "version": version,
        "sha": sha256_hex(&bytes),
        "wrote": true,
    });
    store.remember(&section, &name, &WriteKind::Put, &request_id, &response)?;
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchBody {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    category: Option<Vec<String>>,
    #[serde(default)]
    order: Option<i64>,
}

/// `PATCH /v1/presets/:section/:name`：只改元数据位。
pub async fn patch_preset(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>> {
    let request_id = request_id_of_patch(&body)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section, &name, &WriteKind::Patch, &request_id)? {
        return Ok(Json(receipt.body));
    }
    let file = store.one(&section, &name)?;
    let mut preset = file.body;
    if let Some(archived) = body.archived {
        set_archived(&store, &section, &name, archived)?;
    }
    if let Some(to) = body.name.as_deref() {
        rename_preset(&store, &section, &name, to)?;
    }
    if let Some(category) = body.category.as_deref() {
        preset["category"] = json!(category);
    }
    if let Some(order) = body.order {
        set_order(&store, &section, &name, order)?;
    }
    let bytes = validate_body(&preset)?;
    store.write_file(&store.preset_path(&section, &name)?, &bytes)?;
    let version = store.append_revision(&section, &name, &bytes, &request_id)?;
    let response = json!({ "section": section, "name": name, "version": version });
    store.remember(&section, &name, &WriteKind::Patch, &request_id, &response)?;
    Ok(Json(response))
}

fn request_id_of_patch(body: &PatchBody) -> Result<String> {
    body.request_id.clone().filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::BadRequest("requestId is required".to_string()))
}

fn set_archived(store: &PresetStore, section: &str, name: &str, value: bool) -> Result<()> {
    let dir = store.section_path(section)?;
    let mut names = load_archived(&dir);
    if value && !names.iter().any(|n| n == name) { names.push(name.to_string()); }
    if !value { names.retain(|n| n != name); }
    save_names(&dir, "archived.json", "archived", &names)
}

fn rename_preset(store: &PresetStore, section: &str, from: &str, to: &str) -> Result<()> {
    let old = store.preset_path(section, from)?;
    let new = store.preset_path(section, to)?;
    if !old.is_file() { return Err(ServerError::NotFound(format!("preset 不存在: {section}/{from}"))); }
    if new.is_file() { return Err(ServerError::Conflict(format!("目标已存在: {section}/{to}"))); }
    fs::create_dir_all(new.parent().unwrap()).map_err(|e| ServerError::Internal(e.to_string()))?;
    fs::rename(&old, &new).map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(())
}

fn set_order(store: &PresetStore, section: &str, name: &str, order: i64) -> Result<()> {
    let dir = store.section_path(section)?;
    let mut names = load_order(&dir).into_iter().collect::<Vec<_>>();
    names.sort_by_key(|(_, index)| *index);
    let mut order_names: Vec<String> = names.into_iter().map(|(n, _)| n).collect();
    order_names.retain(|n| n != name);
    let insert_at = (order as usize).min(order_names.len());
    order_names.insert(insert_at, name.to_string());
    save_names(&dir, "order.json", "order", &order_names)
}

/// `DELETE /v1/presets/:section/:name`：软删 = archived（盘保留，会话创建时不列）。
pub async fn delete_preset(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
) -> Result<StatusCode> {
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let _ = store.one(&section, &name)?;
    set_archived(&store, &section, &name, true)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyBody {
    name: String,
    request_id: String,
}

pub async fn copy_preset(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    Json(body): Json<CopyBody>,
) -> Result<Response> {
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section, &body.name, &WriteKind::Copy, &body.request_id)? {
        return Ok((receipt.status, Json(receipt.body)).into_response());
    }
    let bytes = store.one(&section, &name)?.bytes;
    let path = store.preset_path(&section, &body.name)?;
    if path.is_file() {
        return Err(ServerError::Conflict(format!("目标已存在: {section}/{}", body.name)));
    }
    store.write_file(&path, &bytes)?;
    let version = store.append_revision(&section, &body.name, &bytes, &body.request_id)?;
    let response = json!({ "section": section, "name": body.name, "version": version });
    store.remember(&section, &body.name, &WriteKind::Copy, &body.request_id, &response)?;
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

#[derive(Debug, Deserialize)]
pub struct OrderBody {
    order: Vec<String>,
    #[serde(default)]
    request_id: Option<String>,
}

pub async fn put_order(
    State(store): State<SharedStore>,
    AxumPath(section): AxumPath<String>,
    Json(body): Json<OrderBody>,
) -> Result<Json<Value>> {
    let dir = store.section_path(&section)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let names = store.scan(&section)?.into_iter().map(|file| file.name).collect::<Vec<_>>();
    let mut ordered = body.order.clone();
    ordered.retain(|name| names.contains(name));
    for name in &names {
        if !ordered.contains(name) { ordered.push(name.clone()); }
    }
    save_names(&dir, "order.json", "order", &ordered)?;
    Ok(Json(json!({ "section": section, "order": ordered })))
}

pub async fn list_revisions(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _ = store.one(&section, &name)?;
    let revisions = store.revisions(&section, &name)?;
    let items = revisions.iter().map(|revision| json!({
        "n": revision.n,
        "at": revision.at,
        "actor": revision.actor,
        "sha": revision.sha,
        "summary": revision.body.lines().next().unwrap_or(""),
        "requestId": revision.request_id,
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "section": section, "name": name, "items": items })))
}

pub async fn get_revision(
    State(store): State<SharedStore>,
    AxumPath((section, name, n)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let n = n.parse::<i64>().map_err(|_| ServerError::BadRequest(format!("非法修订号: {n}")))?;
    let revision = store.revisions(&section, &name)?
        .into_iter().find(|r| r.n == n)
        .ok_or_else(|| ServerError::NotFound(format!("修订不存在: {section}/{name}#{n}")))?;
    Ok(Json(json!({
        "section": section,
        "name": name,
        "n": revision.n,
        "at": revision.at,
        "actor": revision.actor,
        "sha": revision.sha,
        "body": revision.body,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackBody {
    n: i64,
    request_id: String,
}

pub async fn rollback(
    State(store): State<SharedStore>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    Json(body): Json<RollbackBody>,
) -> Result<Response> {
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section, &name, &WriteKind::Rollback, &body.request_id)? {
        return Ok((receipt.status, Json(receipt.body)).into_response());
    }
    let revision = store.revisions(&section, &name)?
        .into_iter().find(|r| r.n == body.n)
        .ok_or_else(|| ServerError::NotFound(format!("修订不存在: {section}/{name}#{}", body.n)))?;
    let bytes = revision.body.into_bytes();
    store.write_file(&store.preset_path(&section, &name)?, &bytes)?;
    let version = store.append_revision(&section, &name, &bytes, &body.request_id)?;
    let response = json!({ "section": section, "name": name, "version": version, "rolledBackTo": body.n });
    store.remember(&section, &name, &WriteKind::Rollback, &body.request_id, &response)?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// 把 presets 的 13 条路由挂到同一份 state 上。
pub fn attach(state: SharedStore, router: Router) -> Router {
    let presets = Router::new()
        .route("/v1/presets", axum::routing::get(sections))
        .route("/v1/presets/model", axum::routing::get(model))
        .route("/v1/presets/{section}", axum::routing::get(roster).put(put_order))
        .route("/v1/presets/{section}/categories", axum::routing::get(categories))
        .route(
            "/v1/presets/{section}/{name}",
            axum::routing::get(get_preset)
                .put(put_preset)
                .patch(patch_preset)
                .delete(delete_preset),
        )
        .route("/v1/presets/{section}/{name}/copy", axum::routing::post(copy_preset))
        .route("/v1/presets/{section}/{name}/revisions", axum::routing::get(list_revisions))
        .route("/v1/presets/{section}/{name}/revisions/{n}", axum::routing::get(get_revision))
        .route("/v1/presets/{section}/{name}/rollback", axum::routing::post(rollback))
        .with_state(state);
    router.merge(presets)
}
