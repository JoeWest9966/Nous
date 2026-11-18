//! documents 端点（#48）的 server 侧装配。
//!
//! 泛型 handler 只在 `documents` crate 的 `DocumentService` 上做 kind 分派（从注册表查表），
//! 没有 `if kind == "presets"`。读用只读 `Storage`，写经 `Session`（见 `DurableDocumentBackend`）。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use nous_documents::routes::DOCUMENT_ROUTES;
use nous_documents::service::{DocumentService, DocumentWriter};
use nous_documents::store::DocumentReader;
use nous_documents::DocumentKind;
use nous_durable::session::commands::{write_document, DocumentMutation};
use nous_durable::session::Session;
use nous_durable::{DocumentPoint, Storage};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};

/// 默认（只读）backend：测试装配用。写端点明确 409，不假装落库。
pub struct EmptyBackend;

#[async_trait]
impl DocumentWriter for EmptyBackend {
    async fn write(&self, _: &str, _: &str, _: &str, _: Value, _: Option<&str>) -> std::result::Result<String, String> {
        Err("this server was started read-only".into())
    }
    async fn rename(&self, _: &str, _: &str, _: &str, _: &str) -> std::result::Result<Vec<String>, String> {
        Err("this server was started read-only".into())
    }
    async fn action(&self, _: &str, _: &str, _: &str, _: &str, _: Value) -> std::result::Result<(), String> {
        Err("this server was started read-only".into())
    }
}

impl DocumentReader for EmptyBackend {
    fn names(&self, _: &'static str, _: &str) -> std::result::Result<BTreeSet<String>, String> {
        Ok(BTreeSet::new())
    }
    fn get(&self, _: &'static str, _: &str, _: &str) -> std::result::Result<Option<(Value, String)>, String> {
        Ok(None)
    }
    fn revisions(&self, _: &'static str, _: &str, _: &str) -> std::result::Result<Vec<(i64, String)>, String> {
        Ok(vec![])
    }
    fn at(&self, _: &'static str, _: &str, _: &str, _: i64) -> std::result::Result<Option<Value>, String> {
        Ok(None)
    }
}

/// 生产 backend：只读走到库，写走同一个 `Session`。
#[derive(Clone)]
pub struct DurableDocumentBackend {
    path: std::path::PathBuf,
    session: Arc<Session>,
}

impl DurableDocumentBackend {
    pub fn new(path: impl AsRef<Path>, session: Arc<Session>) -> Self {
        Self { path: path.as_ref().to_path_buf(), session }
    }

    fn last_seq(&self, kind: &str, section: &str, name: &str) -> Option<i64> {
        let (id, _) = self.find(kind, section, name).ok()??;
        Storage::open_read_only(&self.path).ok()?.revisions(id).ok()?.last().map(|(seq, _)| *seq)
    }

    fn reader(&self) -> Result<Storage> {
        Storage::open_read_only(&self.path)
            .map_err(|error| ServerError::Internal(format!("open {}: {error}", self.path.display())))
    }

    fn find(&self, kind: &str, section: &str, name: &str) -> Result<Option<(i64, nous_durable::StoredDocument)>> {
        let storage = self.reader()?;
        let address = json!({
            "kind": format!("{kind}.{section}"),
            "scope": { "kind": "session" },
            "key": name,
        });
        let Some(record) = storage.find_document(&address, DocumentPoint::Current)
            .map_err(|error| ServerError::Internal(error.to_string()))? else {
            return Ok(None);
        };
        let id = record.get("id").and_then(Value::as_i64)
            .ok_or_else(|| ServerError::Internal("document record has no id".into()))?;
        let Some(doc) = storage.materialize(id, DocumentPoint::Current)
            .map_err(|error| ServerError::Internal(error.to_string()))? else {
            return Ok(None);
        };
        Ok(Some((id, doc)))
    }
}

impl DocumentReader for DurableDocumentBackend {
    fn names(&self, kind: &'static str, section: &str) -> std::result::Result<BTreeSet<String>, String> {
        let storage = Storage::open_read_only(&self.path).map_err(|e| e.to_string())?;
        let page = storage.scan_documents(
            &nous_durable::DocumentQuery {
                scope: json!({ "kind": "session" }),
                kind: Some(format!("{kind}.{section}")),
                at: DocumentPoint::Current,
            },
            10_000,
            None,
        ).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        Ok(page.items.iter().filter_map(|record| record.get("key").and_then(Value::as_str).map(str::to_string)).collect())
    }

    fn get(&self, kind: &'static str, section: &str, name: &str) -> std::result::Result<Option<(Value, String)>, String> {
        let (_, doc) = self.find(kind, section, name).map_err(|e| e.to_string())?.ok_or("missing")?;
        Ok(Some((doc.value.clone(), format!("v{}:{}", doc.version, doc.deltas_since_base))))
    }

    fn revisions(&self, kind: &'static str, section: &str, name: &str) -> std::result::Result<Vec<(i64, String)>, String> {
        let (id, _) = match self.find(kind, section, name).map_err(|e| e.to_string())? {
            Some(found) => found,
            None => return Ok(vec![]),
        };
        let storage = Storage::open_read_only(&self.path).map_err(|e| e.to_string())?;
        storage.revisions(id).map_err(|e| e.to_string())
    }

    fn at(&self, kind: &'static str, section: &str, name: &str, seq: i64) -> std::result::Result<Option<Value>, String> {
        let (id, _) = match self.find(kind, section, name).map_err(|e| e.to_string())? {
            Some(found) => found,
            None => return Ok(None),
        };
        let storage = Storage::open_read_only(&self.path).map_err(|e| e.to_string())?;
        storage.revision_value(id, seq).map_err(|e| e.to_string())
            .map(|doc| doc.map(|d| d.value))
    }
}

#[async_trait]
impl DocumentWriter for DurableDocumentBackend {
    async fn write(&self, kind: &str, section: &str, name: &str, fields: Value, _etag: Option<&str>) -> std::result::Result<String, String> {
        let mutation = match self.find(kind, section, name).map_err(|e| e.to_string())? {
            Some((id, doc)) => DocumentMutation::Change {
                id,
                base_version: doc.version,
                value: fields,
            },
            None => DocumentMutation::Create {
                kind: format!("{kind}.{section}"),
                key: name.to_string(),
                value: fields,
            },
        };
        write_document(&self.session, vec![mutation]).await.map_err(|e| e.to_string())?;
        Ok(format!("r{}", self.last_seq(kind, section, name).unwrap_or(0)))
    }

    async fn rename(&self, kind: &str, section: &str, name: &str, to: &str) -> std::result::Result<Vec<String>, String> {
        let (id, doc) = self.find(kind, section, name).map_err(|e| e.to_string())?
            .ok_or_else(|| format!("document {name} not found"))?;
        let mut fields = doc.value.clone();
        fields["name"] = json!(to);
        write_document(&self.session, vec![
            DocumentMutation::Retire { id },
            DocumentMutation::Create {
                kind: format!("{kind}.{section}"),
                key: to.to_string(),
                value: fields,
            },
        ]).await.map_err(|e| e.to_string())?;
        Ok(vec![])
    }

    async fn action(&self, kind: &str, section: &str, name: &str, action: &str, payload: Value) -> std::result::Result<(), String> {
        let (id, doc) = self.find(kind, section, name).map_err(|e| e.to_string())?
            .ok_or_else(|| format!("document {name} not found"))?;
        let mut fields = doc.value.clone();
        fields["action"] = json!(action);
        fields["actionPayload"] = payload;
        write_document(&self.session, vec![DocumentMutation::Change {
            id, base_version: doc.version, value: fields,
        }]).await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// 从一张注册表构造 documents 的 axum 路由。测试打每条路由时用的也是这张表。
pub fn documents_routes() -> Router<Arc<AppState>> {
    let mut router: Router<Arc<AppState>> = Router::new();
    for route in DOCUMENT_ROUTES {
        router = router.route(route.path, handler_for(route.method, route.path));
    }
    router
}

/// **唯一的** `(method, path)` → handler 映射。它按路径模板选泛型 handler，不按 kind 分支；
/// handler 内部按 `:kind` 从编译期注册表找到 `KindAdapter`（见 `adapter`）。
fn handler_for(method: &'static str, path: &'static str) -> axum::routing::MethodRouter<Arc<AppState>> {
    let is_get = method == "GET";
    if path.ends_with("/history/diff") && is_get { return get(history_diff) }
    if path.ends_with("/pending/diff") && is_get { return get(pending_diff) }
    if path.ends_with("/history") && is_get { return get(history) }
    if path.ends_with("/pending") && is_get { return get(pending) }
    if path.ends_with("/validate") && is_get { return get(validate) }
    if path.ends_with("/rename") { return post(rename) }
    if path.ends_with("/publish") { return post(action_publish) }
    if path.ends_with("/reload") { return post(action_reload) }
    match (method, path) {
        ("GET", "/v1/documents/{kind}") => get(list),
        ("POST", "/v1/documents/{kind}/{section}") | ("POST", "/v1/documents/{kind}") => post(create),
        ("GET", "/v1/documents/{kind}/{section}/{name}") => get(get_one),
        ("PUT", "/v1/documents/{kind}/{section}/{name}") => put(put_one),
        _ => post(action_publish),
    }
}

/// kind 注册表：两个 kind 的编译期结构体都在这里被同一个适配器泛型函数包住。
struct KindAdapter {
    kind: &'static str,
    list: fn(&DocumentService, &str) -> std::result::Result<Value, String>,
    get: fn(&DocumentService, &str, &str) -> std::result::Result<Option<Value>, String>,
    validate: fn(&DocumentService, &str, &str) -> std::result::Result<Value, String>,
    history: fn(&DocumentService, &str, &str) -> std::result::Result<Value, String>,
    diff: fn(&DocumentService, &str, &str, Option<&str>, bool) -> std::result::Result<Option<Value>, String>,
    pending: fn(&DocumentService, &str, &str) -> std::result::Result<Value, String>,
}

const fn of<K: DocumentKind>() -> KindAdapter {
    KindAdapter {
        kind: K::KIND,
        list: DocumentService::list::<K>,
        get: DocumentService::get::<K>,
        validate: DocumentService::validate::<K>,
        history: DocumentService::history::<K>,
        diff: DocumentService::diff::<K>,
        pending: DocumentService::pending::<K>,
    }
}

/// 编译期 kind 表。加一个 kind 只加一行这里，handler 一行都不改 —— 没有 per-kind 分支。
static KIND_ADAPTERS: &[KindAdapter] = &[
    of::<nous_documents::kinds::presets::PresetFields>(),
    of::<nous_documents::kinds::prompts::PromptFields>(),
];

fn adapter(kind: &str) -> Option<&'static KindAdapter> {
    KIND_ADAPTERS.iter().find(|adapter| adapter.kind == kind)
}

fn service(state: &Arc<AppState>) -> Result<DocumentService> {
    state.document_service().ok_or_else(|| ServerError::Conflict("no document backend".into()))
}

fn bad(message: String) -> ServerError {
    if message.contains("read-only") { ServerError::Conflict(message) } else { ServerError::Internal(message) }
}

#[derive(Deserialize, Default)]
struct PutBody {
    #[serde(default)]
    fields: Value,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    category: Option<Vec<String>>,
}

impl PutBody {
    fn fields(self) -> Value {
        if !self.fields.is_null() { return self.fields }
        let mut map = serde_json::Map::new();
        if let Some(name) = self.name { map.insert("name".into(), json!(name)); }
        if let Some(summary) = self.summary { map.insert("summary".into(), json!(summary)); }
        if let Some(category) = self.category { map.insert("category".into(), json!(category)); }
        Value::Object(map)
    }
}

async fn list(State(state): State<Arc<AppState>>, AxumPath((kind,)): AxumPath<(String,)>) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    let section = "Session-Template".to_string();
    Ok(Json((a.list)(&service(&state)?, &section).map_err(bad)?))
}

async fn get_one(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    (a.get)(&service(&state)?, &section, &name).map_err(bad)?
        .map(Json).ok_or_else(|| ServerError::NotFound(format!("document {name} not found")))
}

async fn put_one(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
    Json(body): Json<PutBody>,
) -> Result<Json<Value>> {
    let svc = service(&state)?;
    let writer = svc.writer().ok_or_else(|| ServerError::Conflict("no document writer".into()))?;
    let etag = writer.write(&kind, &section, &name, body.fields(), None).await.map_err(bad)?;
    Ok(Json(json!({ "kind": kind, "section": section, "name": name, "etag": etag })))
}

async fn create(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section)): AxumPath<(String, String)>,
    Json(body): Json<PutBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let svc = service(&state)?;
    let writer = svc.writer().ok_or_else(|| ServerError::Conflict("no document writer".into()))?;
    let mut fields = body.fields();
    let name = fields.get("name").and_then(Value::as_str).unwrap_or("untitled").to_string();
    fields["name"] = json!(name);
    let etag = writer.write(&kind, &section, &name, fields, None).await.map_err(bad)?;
    Ok((StatusCode::CREATED, Json(json!({ "kind": kind, "section": section, "name": name, "etag": etag }))))
}

async fn validate(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    Ok(Json((a.validate)(&service(&state)?, &section, &name).map_err(bad)?))
}

async fn history(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    Ok(Json((a.history)(&service(&state)?, &section, &name).map_err(bad)?))
}

#[derive(Deserialize, Default)]
struct DiffQuery {
    hash: Option<String>,
    /// desk 的 `history/diff` 带 `path`；documents 的文档就是最小单元，路径即 name，
    /// 这里显式收下但不解释，避免客户端拿到 400。
    #[allow(dead_code)]
    path: Option<String>,
}

async fn history_diff(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
    axum::extract::Query(query): axum::extract::Query<DiffQuery>,
) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    let diff = (a.diff)(&service(&state)?, &section, &name, query.hash.as_deref(), false).map_err(bad)?;
    diff.map(Json).ok_or_else(|| ServerError::NotFound("revision not found".into()))
}

async fn pending(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    Ok(Json((a.pending)(&service(&state)?, &section, &name).map_err(bad)?))
}

async fn pending_diff(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let a = adapter(&kind).ok_or_else(|| ServerError::NotFound(format!("unknown kind {kind}")))?;
    let diff = (a.diff)(&service(&state)?, &section, &name, None, true).map_err(bad)?;
    diff.map(Json).ok_or_else(|| ServerError::NotFound("no pending diff".into()))
}

#[derive(Deserialize, Default)]
struct ActionBody {
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    message: Option<String>,
    #[serde(flatten)]
    rest: std::collections::HashMap<String, Value>,
}

async fn rename(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
    Json(body): Json<ActionBody>,
) -> Result<Json<Value>> {
    let svc = service(&state)?;
    let to = body.to.unwrap_or_default();
    let writer = svc.writer().ok_or_else(|| ServerError::Conflict("no document writer".into()))?;
    let refs = writer.rename(&kind, &section, &name, &to).await.map_err(bad)?;
    Ok(Json(json!({ "ok": true, "kind": kind, "section": section, "name": name, "to": to, "refs": refs, "pushed": null, "commit": {} })))
}

async fn action_publish(
    state: State<Arc<AppState>>,
    path: AxumPath<(String, String, String)>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    action_with_op(state, path, "publish", body).await
}

async fn action_reload(
    state: State<Arc<AppState>>,
    path: AxumPath<(String, String, String)>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    action_with_op(state, path, "reload", body).await
}

async fn action_with_op(
    State(state): State<Arc<AppState>>,
    AxumPath((kind, section, name)): AxumPath<(String, String, String)>,
    op: &str,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let svc = service(&state)?;
    let writer = svc.writer().ok_or_else(|| ServerError::Conflict("no document writer".into()))?;
    let payload = if body.is_empty() { json!({}) } else {
        let parsed: ActionBody = serde_json::from_slice(&body)
            .map_err(|e| ServerError::BadRequest(format!("action body: {e}")))?;
        serde_json::to_value(parsed.rest).unwrap()
    };
    writer.action(&kind, &section, &name, op, payload).await.map_err(bad)?;
    Ok(Json(json!({ "ok": true, "kind": kind, "section": section, "name": name, "action": op })))
}

/// `GET /v1/_routes`：注册表在 `meta.rs`，与 capabilities 同源（唯一真相）。
async fn routes() -> Json<Value> {
    Json(crate::meta::routes_view())
}

pub fn attach(state: Arc<AppState>, router: Router<Arc<AppState>>) -> Router {
    router
        .route("/v1/_routes", get(routes))
        .merge(documents_routes())
        .with_state(state)
}
