//! 本插件自己的端点：`/v1/presets*` 全 19 条（#60 的 HTTP 面）。
//!
//! Magento 2 那种「模块自带 controller」：handler 住在插件里，插件不认识 `server` 的
//! `AppState`，只声明它需要什么（[`Host`]：模板私库根 + 插件库路径 + 几个外部名册根），
//! 由宿主实现并注入。判定（形状 / 引用 / 展开）是同一个 crate 的纯函数层 ——
//! 这一层只做 HTTP ↔ 盘面翻译。
//!
//! 盘是唯一真相：`<root>/<section>/<name>/<manifest>`，进程内**没有**常驻 map。
//! 每次读都扫目录；每次写都整段串行锁，然后重扫。修订只落 `nous-plugins.db` 的
//! `presets_revisions`，不 spawn git（#75 的私库 git 历史是另一件事）。
//! 段清单读 `<root>/sections.json`：段 ID 就是文件夹名，每段自带清单文件名与校验器
//! （`Session-Template` = `preset.json`，`Ultimate-Template` = `ultimate.json`）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use crate as admin;
use crate::{ArchivedItem, Ctx, Item, ModelLevels, ParamDecl, Section, TriggerDecl};
use nous_repo::Repo;
use nous_routes::Route;
use nous_store::Store;
use serde::Deserialize;
use serde_json::{json, Value};

use nous_store::PresetRevision as Revision;

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "Nous_PresetsAdmin";

/// 一条端点的拒绝理由：搬家前 `ServerError` 的六个分支，状态码与文案一字没改。
///
/// 为什么不用 `ServerError`：插件不认识 `server`（`docs/magento2-migration.md` §3.3 第 3 条），
/// 拒绝理由必须活在自己这一侧；宿主实现 [`Host`] 时把自己的错误翻成这个形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    BadRequest(String),
    NotFound(String),
    /// 状态冲突（已存在 / 已被改过）。409 是「现在不行」，400 是「参数这么写不行」。
    Conflict(String),
    /// If-Match 不匹配（412）。
    PreconditionFailed(String),
    /// 目标被锁定（423）。
    Locked(String),
    Internal(String),
}

impl Refusal {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            Self::Locked(_) => StatusCode::LOCKED,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::PreconditionFailed(message)
            | Self::Locked(message)
            | Self::Internal(message) => message,
        }
    }

    /// 宿主侧翻错误用：`ServerError` 的六个分支一对一搬过来。
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        let message = message.into();
        match status {
            StatusCode::BAD_REQUEST => Self::BadRequest(message),
            StatusCode::NOT_FOUND => Self::NotFound(message),
            StatusCode::CONFLICT => Self::Conflict(message),
            StatusCode::PRECONDITION_FAILED => Self::PreconditionFailed(message),
            StatusCode::LOCKED => Self::Locked(message),
            _ => Self::Internal(message),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Refusal {}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = self.status();
        (status, Json(json!({ "error": self.message() }))).into_response()
    }
}

pub type Result<T> = std::result::Result<T, Refusal>;

/// 归档目录名（**段目录里**的隐藏目录：`<root>/<段>/.archive/<名>`）。
pub const ARCHIVE_DIR: &str = ".archive";

/// 宿主能力：这个插件要的那几件事，一个方法一个能力。
///
/// 插件不认识 `AppState`（`docs/magento2-migration.md` §3.3 第 3 条）；宿主在
/// `crates/Nous/Server/src/plugins.rs` 里实现这个 trait 并注入。
///
/// `None` = 宿主没装配：所有端点回 `409 {"error":"presets not configured"}`
/// （搬家前未装配时这些路径是 404 —— 404 是「没有这条路径」，409 是「这条路径现在不行」，
/// 后者才说得清「模板面没接上」）。
pub trait Host {
    /// 模板私库根 + 插件库路径 + 外部名册根。`None` = 没装配。
    fn preset_sources(&self) -> Option<Sources>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn preset_sources(&self) -> Option<Sources> {
        (**self).preset_sources()
    }
}

/// 解析要问的那几个外部根。生产从 `env` 插件取；测试显式注入，不读进程环境。
#[derive(Debug, Clone)]
pub struct Sources {
    /// 模板私库根（`NOUS_PRESETS`）：`<root>/<section>/<name>/<manifest>`。
    pub root: PathBuf,
    /// 插件库（`nous-plugins.db`）：修订表与幂等回执都在这一个文件里。
    pub db: PathBuf,
    pub skills: PathBuf,
    pub prompts: PathBuf,
    pub extensions: PathBuf,
    pub triggers: PathBuf,
    /// 注入的模型编目（faux / 测试）。`None` = 按 `catalog_env` 的来源现读。
    pub models: Option<Vec<ModelLevels>>,
    pub device_model: Option<(String, String)>,
    /// 编目来源（文件路径等）。**由宿主算好**（`main` 的 `CatalogEnvironment::from_env()`）：
    /// 插件不读环境变量，路径解析仍然只有一处。
    pub catalog_env: nous_device::CatalogEnvironment,
}

impl Sources {
    pub fn from_env() -> Self {
        Self {
            root: nous_env::PRESETS.resolve(),
            db: PathBuf::new(),
            skills: nous_env::SKILLS.resolve(),
            prompts: nous_env::PROMPTS.resolve(),
            extensions: nous_env::EXTEND.resolve(),
            triggers: nous_env::TRIGGERS.resolve(),
            models: None,
            device_model: None,
            catalog_env: nous_device::CatalogEnvironment::from_env(),
        }
    }
}

/// 整段一把锁：写端点串行，写完立即重扫该段索引（这里没有索引，roster 每次现扫）。
pub struct PresetStore {
    root: PathBuf,
    db: PathBuf,
    lock: Mutex<()>,
    sources: Sources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    Put,
    Patch,
    Copy,
    Rollback,
}

impl WriteKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Copy => "copy",
            Self::Rollback => "rollback",
        }
    }
}

/// 一次写操作的回执：同一 requestId 重放回第一次的结果，version 不涨。
#[derive(Debug, Clone)]
struct Receipt {
    body: Value,
    status: StatusCode,
}

impl PresetStore {
    /// 从宿主给的那一份 [`Sources`] 建。**不读进程环境** —— 路径解析归 `env` 插件，
    /// 由宿主装配时算好（`main` 一次、测试一次），这里只接。
    pub fn new(sources: Sources) -> Self {
        Self { root: sources.root.clone(), db: sources.db.clone(), lock: Mutex::new(()), sources }
    }

    /// 段清单：`<root>/sections.json`；没有就按目录名兜底两段。
    fn sections(&self) -> Result<Vec<Section>> {
        let path = self.root.join("sections.json");
        let text = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(Refusal::Internal(format!("读 sections.json 失败: {error}")))
            }
        };
        admin::read_sections(text.as_deref()).map_err(Refusal::Internal)
    }

    fn section(&self, id: &str) -> Result<Section> {
        admin::find(&self.sections()?, id)
            .cloned()
            .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{id}")))
    }

    fn section_dir(&self, section: &Section) -> PathBuf {
        self.root.join(&section.id)
    }

    fn preset_dir(&self, section: &Section, name: &str) -> PathBuf {
        self.section_dir(section).join(name)
    }

    fn manifest_path(&self, section: &Section, name: &str) -> PathBuf {
        self.preset_dir(section, name).join(&section.manifest)
    }

    /// 扫目录，不读缓存：盘上有目录 + 有清单文件 = 这条存在。
    /// 隐藏目录（`.archive/` 等）天然不进。
    fn scan(&self, section: &Section) -> Result<Vec<Item>> {
        let dir = self.section_dir(section);
        let mut out = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(error) => {
                return Err(Refusal::Internal(format!("扫 {} 失败: {error}", section.id)))
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(bytes) = fs::read(path.join(&section.manifest)) else { continue };
            let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            out.push(Item {
                name,
                dir: path.to_string_lossy().into_owned(),
                body,
                bytes,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn one(&self, section: &Section, name: &str) -> Result<Item> {
        let dir = self.preset_dir(section, name);
        let path = dir.join(&section.manifest);
        let bytes = fs::read(&path).map_err(|_| {
            Refusal::NotFound(format!("preset 不存在: {}/{name}", section.id))
        })?;
        let body = serde_json::from_slice(&bytes).map_err(|error| {
            Refusal::BadRequest(format!("{} 坏了: {error}", section.manifest))
        })?;
        Ok(Item { name: name.to_string(), dir: dir.to_string_lossy().into_owned(), body, bytes })
    }

    /// 归档区：`<段>/.archive/<名>/<manifest>`。
    fn archived(&self, section: &Section) -> Vec<ArchivedItem> {
        let dir = self.section_dir(section).join(ARCHIVE_DIR);
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&dir) else { return out };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(bytes) = fs::read(path.join(&section.manifest)) else { continue };
            let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let archived_at = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|span| span.as_millis() as i64);
            out.push(ArchivedItem { name, body, archived_at });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn read_section_json(&self, section: &Section, file: &str) -> Option<Value> {
        fs::read(self.section_dir(section).join(file))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    fn vocabulary(&self, section: &Section) -> Vec<String> {
        admin::vocabulary(self.read_section_json(section, "categories.json").as_ref())
    }

    fn order(&self, section: &Section) -> Vec<String> {
        admin::order(self.read_section_json(section, "order.json").as_ref())
    }

    fn git(&self) -> Option<Repo> {
        if !self.root.join(".git").exists() {
            return None;
        }
        Repo::open(&self.root).ok()
    }

    fn tracked(&self, section: &Section) -> BTreeSet<String> {
        let Some(repo) = self.git() else { return BTreeSet::new() };
        repo.tracked_paths(&section.id)
            .map(|paths| paths.into_iter().collect())
            .unwrap_or_default()
    }

    /// 这一段的仓状态。读不了仓给一句话，不是 500 —— 白屏说不清「是仓没了还是预设没了」。
    fn repo_view(&self, section: &Section) -> Value {
        let empty = || {
            json!({ "head": null, "dirty": false, "ahead": 0, "behind": 0, "remote": null })
        };
        let Some(repo) = self.git() else { return empty() };
        let remote = repo.remote_url().ok().flatten();
        match repo.status() {
            Ok(status) => {
                // `dirty` 只认这一段：另一段没写完不该让这一段变脏。
                let dirty = repo
                    .pending()
                    .map(|units| {
                        let prefix = format!("{}/", section.id);
                        units.iter().any(|unit| unit.paths.iter().any(|path| path.starts_with(&prefix)))
                    })
                    .unwrap_or(status.dirty);
                json!({
                    "head": status.head,
                    "dirty": dirty,
                    "ahead": status.ahead,
                    "behind": status.behind,
                    "remote": remote,
                })
            }
            Err(error) => {
                let mut value = empty();
                value["error"] = json!(error.to_string());
                value["remote"] = json!(remote);
                value
            }
        }
    }

    /// 解析要问的那几件事。每次现扫 —— 引用校验要的是**此刻**的进程状态。
    fn ctx(&self, sections: &[Section]) -> Ctx {
        let mut ctx = Ctx::default();
        ctx.skills = dir_names(&self.sources.skills);
        ctx.extensions = dir_names(&self.sources.extensions);
        ctx.prompts = nous_prompts::read_catalog(&self.sources.prompts)
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        ctx.triggers = trigger_decls(&self.sources.triggers);
        let (models, device) = self.models();
        ctx.models = models;
        ctx.device_model = device;
        // 终极模板的 `session` 引用 Session-Template 段的名册。
        if let Some(sessions) = sections.iter().find(|section| section.id == "Session-Template") {
            ctx.sessions = self
                .scan(sessions)
                .map(|items| items.into_iter().map(|item| item.name).collect())
                .unwrap_or_default();
        }
        ctx
    }

    fn models(&self) -> (Vec<ModelLevels>, Option<(String, String)>) {
        if let Some(models) = &self.sources.models {
            return (models.clone(), self.sources.device_model.clone());
        }
        let catalog = nous_device::load_models(&self.sources.catalog_env).unwrap_or_default();
        let models = catalog
            .items
            .iter()
            .map(|item| ModelLevels {
                provider: item.provider.clone(),
                model_id: item.model_id.clone(),
                thinking_levels: item.thinking_levels.clone(),
            })
            .collect();
        let device = catalog
            .default
            .as_ref()
            .map(|model| (model.provider.clone(), model.model_id.clone()))
            .or(self.sources.device_model.clone());
        (models, device)
    }

    /// 仓里的路径提交（归档 / 改名 / 发布共用）。没有仓就什么都不做（本地单用户合法）。
    fn commit(&self, paths: &[String], message: &str, request_id: Option<&str>) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let Some(repo) = self.git() else { return Ok(()) };
        repo.commit(paths, message, request_id)
            .map(|_| ())
            .map_err(|error| Refusal::BadRequest(format!("git 提交失败：{error}")))
    }

    fn store(&self) -> Result<Store> {
        Store::open(&self.db).map_err(|error| Refusal::Internal(format!("打开插件库失败: {error}")))
    }

    fn receipt(&self, section: &str, name: &str, kind: &WriteKind, request_id: &str) -> Result<Option<Receipt>> {
        let text = self.store()?
            .preset_receipt(section, name, kind.as_str(), request_id)
            .map_err(|error| Refusal::Internal(format!("读回执失败: {error}")))?;
        match text {
            None => Ok(None),
            Some(text) => {
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| Refusal::Internal(format!("回执坏了: {error}")))?;
                let status = match value["status"].as_i64() {
                    Some(201) => StatusCode::CREATED,
                    Some(200) => StatusCode::OK,
                    _ => StatusCode::OK,
                };
                Ok(Some(Receipt { body: value, status }))
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
        self.store()?
            .remember_preset_write(section, name, kind.as_str(), request_id, &body.to_string())
            .map_err(|error| Refusal::Internal(format!("记回执失败: {error}")))?;
        Ok(())
    }

    fn revisions(&self, section: &str, name: &str) -> Result<Vec<Revision>> {
        self.store()?
            .preset_revisions(section, name)
            .map_err(|error| Refusal::Internal(format!("读修订失败: {error}")))
    }

    fn version(&self, section: &str, name: &str) -> Result<i64> {
        Ok(self.revisions(section, name)?.last().map(|revision| revision.n).unwrap_or(0))
    }

    /// 写一次修订：n = 当前最大 n + 1，整条在一个事务里，原子。
    fn append_revision(&self, section: &str, name: &str, bytes: &[u8], request_id: &str) -> Result<i64> {
        self.store()?
            .append_preset_revision(section, name, bytes, "local", now_ms(), &sha256_hex(bytes), request_id)
            .map_err(|error| Refusal::Internal(format!("写修订失败: {error}")))
    }

    /// 原子写盘：临时文件 + rename，写完的字节就是之后 roster 的 sha 来源。
    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let dir = path.parent().ok_or_else(|| Refusal::Internal("没有父目录".into()))?;
        fs::create_dir_all(dir)
            .map_err(|error| Refusal::Internal(format!("建目录失败: {error}")))?;
        let temp = dir.join(".preset.json.tmp");
        {
            let mut file = fs::File::create(&temp)
                .map_err(|error| Refusal::Internal(format!("建临时文件失败: {error}")))?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| Refusal::Internal(format!("写临时文件失败: {error}")))?;
        }
        fs::rename(&temp, path)
            .map_err(|error| Refusal::Internal(format!("原子替换失败: {error}")))?;
        Ok(())
    }

    fn write_section_json(&self, section: &Section, file: &str, value: &Value) -> Result<()> {
        let path = self.section_dir(section).join(file);
        let mut bytes = serde_json::to_vec_pretty(value)
            .map_err(|error| Refusal::Internal(format!("序列化失败: {error}")))?;
        bytes.push(b'\n');
        self.write_file(&path, &bytes)
    }

    /// 归档 / 还原 / 彻底删除。`git` 的 `git mv` 用不了（单元是 `<段>/<名>`），
    /// 所以搬目录用 `fs::rename`，提交走 `Repo::commit` 的显式 paths。
    fn move_into_archive(&self, section: &Section, name: &str) -> Result<Vec<String>> {
        let from = self.preset_dir(section, name);
        if !from.is_dir() {
            return Err(Refusal::NotFound(format!("工作区里没有 {name}/")));
        }
        let to = self.section_dir(section).join(ARCHIVE_DIR).join(name);
        if to.exists() {
            return Err(Refusal::Conflict(format!("已归档过：{name}")));
        }
        fs::create_dir_all(to.parent().unwrap())
            .map_err(|error| Refusal::Internal(format!("建归档目录失败: {error}")))?;
        fs::rename(&from, &to).map_err(|error| Refusal::Internal(format!("搬进归档失败: {error}")))?;
        Ok(vec![
            format!("{}/{}", section.id, name),
            format!("{}/{ARCHIVE_DIR}/{name}", section.id),
        ])
    }

    fn move_out_of_archive(&self, section: &Section, name: &str) -> Result<Vec<String>> {
        let from = self.section_dir(section).join(ARCHIVE_DIR).join(name);
        if !from.is_dir() {
            return Err(Refusal::NotFound(format!("归档区里没有 {name}/")));
        }
        let to = self.preset_dir(section, name);
        if to.exists() {
            return Err(Refusal::Conflict(format!("目标已存在：{name}")));
        }
        fs::rename(&from, &to).map_err(|error| Refusal::Internal(format!("搬出归档失败: {error}")))?;
        Ok(vec![
            format!("{}/{}", section.id, name),
            format!("{}/{ARCHIVE_DIR}/{name}", section.id),
        ])
    }

    /// 把段的形状闸跑一遍（发布前）。返回问题清单。
    fn gate(&self, section: &Section) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for item in self.scan(section)? {
            problems.extend(admin::problems_for(section, &item.body, Some(&item.name)));
        }
        Ok(problems)
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// 文件字节的 sha256：与 `nous_repo::files::etag` 同一份标准实现（不另抄一份，抄错过一次）。
fn sha256_hex(input: &[u8]) -> String {
    let digest = nous_repo::sha256(input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 一个目录下的子目录名（隐藏目录跳过）。技能 / 扩展的「装没装」判据。
fn dir_names(path: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(entries) = fs::read_dir(path) else { return out };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && name != "node_modules" {
            out.insert(name);
        }
    }
    out
}

/// 触发器声明：`NOUS_TRIGGER/<code>/nous.json` 的 `params[]`。
fn trigger_decls(root: &Path) -> Vec<TriggerDecl> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let code = entry.file_name().to_string_lossy().into_owned();
        if code.starts_with('.') {
            continue;
        }
        let Ok(bytes) = fs::read(path.join("nous.json")) else { continue };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { continue };
        let params = value
            .get("params")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|param| {
                        let name = param.get("name").and_then(Value::as_str)?.to_string();
                        Some(ParamDecl {
                            name,
                            kind: param
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("string")
                                .to_string(),
                            label: param.get("label").and_then(Value::as_str).map(str::to_string),
                            required: param.get("required").and_then(Value::as_bool).unwrap_or(false),
                            options: param
                                .get("options")
                                .and_then(Value::as_array)
                                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
                                .unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(TriggerDecl {
            code,
            summary: value.get("summary").and_then(Value::as_str).map(str::to_string),
            version: value.get("version").and_then(Value::as_i64),
            params,
        });
    }
    out.sort_by(|a, b| a.code.cmp(&b.code));
    out
}

fn check_name(raw: &str) -> Result<String> {
    admin::check_name(raw).map_err(Refusal::BadRequest)
}

fn gate_message(problems: &[String]) -> String {
    if problems.len() > 4 {
        format!("{}；等 {} 项", problems[..4].join("；"), problems.len())
    } else {
        problems.join("；")
    }
}

fn repo_error(error: nous_repo::RepoError) -> Refusal {
    use nous_repo::RepoError as E;
    match error {
        E::BadRequest(message) | E::InvalidPath(message) => Refusal::BadRequest(message),
        E::Validation { field, problem } => Refusal::BadRequest(format!("{field}: {problem}")),
        E::NotFound(message) => Refusal::NotFound(message),
        E::Conflict { message, .. } => Refusal::Conflict(message),
        E::Busy(message) => Refusal::Conflict(message),
        E::Locked(message) => Refusal::Locked(message),
        E::LockPoisoned | E::Io(_) => Refusal::Internal(error.to_string()),
    }
}

fn pretty(bytes: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(bytes).unwrap_or_else(|_| b"{}".to_vec());
    out.push(b'\n');
    out
}

/// 宿主能力 → 这一层的写者。`None` = 没装配，所有端点回 409（不是空 200、也不是 404）。
fn store_of<H: Host>(host: &H) -> Result<PresetStore> {
    host.preset_sources()
        .map(PresetStore::new)
        .ok_or_else(|| Refusal::Conflict("presets not configured".to_string()))
}

// ── HTTP handlers ──────────────────────────────────────────────────────────

/// `GET /v1/presets`：段清单，每段带 count（不硬编码段名到客户端）。
pub async fn sections<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let sections = store.sections()?;
    let mut items = Vec::new();
    for section in &sections {
        let count = store.scan(section)?.len();
        items.push(json!({
            "id": section.id,
            "title": section.title,
            "count": count,
            "manifest": section.manifest,
            "checker": section.checker.as_str(),
        }));
    }
    Ok(Json(json!({ "root": store.root, "sections": items })))
}

/// `GET /v1/presets/model`：预设能引用的模型清单（只读，沿用原状）。
pub async fn model<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let path = store.root.join("models.json");
    let value = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(json!({ "models": [] })),
        Err(_) => json!({ "models": [] }),
    };
    Ok(Json(value))
}

/// `GET /v1/presets/:section`：overview（名册 + 归档 + 词表 + 仓状态）。**每次现扫盘**。
pub async fn roster<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let sections = store.sections()?;
    let section = admin::find(&sections, &section_id)
        .cloned()
        .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{section_id}")))?;
    let ctx = store.ctx(&sections);
    let tracked = store.tracked(&section);
    let order = store.order(&section);

    let mut rows: Vec<(String, Value)> = Vec::new();
    for item in store.scan(&section)? {
        let version = store.version(&section.id, &item.name)?;
        let is_tracked = tracked.iter().any(|path| {
            path == &format!("{}/{}", section.id, item.name)
                || path.starts_with(&format!("{}/{}/", section.id, item.name))
        });
        let order_at = order
            .iter()
            .position(|name| name == &item.name)
            .map(|index| index as i64)
            .unwrap_or(i64::MAX);
        let row = admin::roster_row(
            &item,
            &section,
            &ctx,
            is_tracked,
            version,
            order_at,
            sha256_hex(&item.bytes),
        );
        rows.push((item.name.clone(), row));
    }
    let rows = admin::apply_order(rows, &order, |(name, _)| name.as_str());
    let presets: Vec<Value> = rows.into_iter().map(|(_, row)| row).collect();

    let archived: Vec<Value> = store.archived(&section).iter().map(admin::archived_row).collect();
    let mut categories: Vec<String> = store.vocabulary(&section);
    for item in &presets {
        if let Some(list) = item.get("category").and_then(Value::as_array) {
            categories.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    let categories = admin::sort_categories(categories);
    let repo = store.repo_view(&section);

    Ok(Json(admin::overview(
        &section,
        &store.root.to_string_lossy(),
        repo,
        presets,
        archived,
        categories,
    )))
}

/// `GET /v1/presets/:section/categories`：段内类别名册（含零计数类别）。
pub async fn categories<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let sections = store.sections()?;
    let section = admin::find(&sections, &section_id)
        .cloned()
        .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{section_id}")))?;
    let mut names = store.vocabulary(&section);
    for item in store.scan(&section)? {
        if let Some(list) = item.body.get("category").and_then(Value::as_array) {
            names.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    let categories = admin::sort_categories(names);
    let presets: Vec<Value> = store
        .scan(&section)?
        .iter()
        .map(|item| json!({ "name": item.name, "category": item.body.get("category") }))
        .collect();
    Ok(Json(json!({ "section": section.id, "categories": categories, "presets": presets })))
}

#[derive(Debug, Deserialize)]
pub struct CategoryOpBody {
    pub op: String,
    pub name: String,
    #[serde(default)]
    pub to: Option<String>,
}

/// `POST /v1/presets/:section/categories`：词表的增 / 删 / 改名。
///
/// 改名 / 删除要连带改每条预设挂着的类别（含子路径）—— 只改词表的话，
/// 下一次读会被「每条挂着的」补回来，等于没改。
pub async fn mutate_categories<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<CategoryOpBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let sections = store.sections()?;
    let section = admin::find(&sections, &section_id)
        .cloned()
        .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{section_id}")))?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(Refusal::BadRequest("类别名不能为空".into()));
    }
    let to = body.to.as_deref().map(str::trim).filter(|text| !text.is_empty());
    if matches!(body.op.as_str(), "rename") && to.is_none() {
        return Err(Refusal::BadRequest("改名要给 to".into()));
    }
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());

    let mut vocab = store.vocabulary(&section);
    match body.op.as_str() {
        "add" => {
            if !vocab.iter().any(|entry| entry == &name) {
                vocab.push(name.clone());
            }
        }
        "remove" => {
            vocab.retain(|entry| entry != &name && !entry.starts_with(&format!("{name}/")));
        }
        "rename" => {
            let to = to.unwrap();
            for entry in vocab.iter_mut() {
                if entry == &name {
                    *entry = to.to_string();
                } else if let Some(rest) = entry.strip_prefix(&format!("{name}/")) {
                    *entry = format!("{to}/{rest}");
                }
            }
        }
        other => {
            return Err(Refusal::BadRequest(format!("不认识的 op: {other}（只有 add / remove / rename）")))
        }
    }
    let vocab = admin::sort_categories(vocab);
    store.write_section_json(&section, "categories.json", &json!({ "v": 1, "categories": vocab }))?;

    // 连带改每条预设挂着的类别。
    let mut touched = 0usize;
    let mut mounted: Vec<String> = Vec::new();
    for item in store.scan(&section)? {
        let Some(list) = item.body.get("category").and_then(Value::as_array) else { continue };
        let mut next: Vec<String> = list.iter().filter_map(Value::as_str).map(str::to_string).collect();
        let before = next.clone();
        match body.op.as_str() {
            "remove" => next.retain(|entry| entry != &name && !entry.starts_with(&format!("{name}/"))),
            "rename" => {
                let to = to.unwrap();
                for entry in next.iter_mut() {
                    if entry == &name {
                        *entry = to.to_string();
                    } else if let Some(rest) = entry.strip_prefix(&format!("{name}/")) {
                        *entry = format!("{to}/{rest}");
                    }
                }
            }
            _ => {}
        }
        if next == before {
            continue;
        }
        let mut preset = item.body.clone();
        let sorted = admin::sort_categories(next.clone());
        mounted.extend(sorted.iter().cloned());
        preset["category"] = json!(sorted);
        let bytes = pretty(&preset);
        store.write_file(&store.manifest_path(&section, &item.name), &bytes)?;
        touched += 1;
    }
    // 词表 ∪ 现在真正挂着的：改名一个原本没进词表的类别也要在回包里看得见。
    let vocab = admin::sort_categories(vocab.into_iter().chain(mounted));
    store.write_section_json(&section, "categories.json", &json!({ "v": 1, "categories": vocab }))?;

    Ok(Json(json!({
        "section": section.id,
        "categories": vocab,
        "touched": touched,
        "presets": store.scan(&section)?.iter().map(|item| json!({
            "name": item.name,
            "category": item.body.get("category").cloned().unwrap_or(json!([])),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct SetCategoryBody {
    pub name: String,
    #[serde(default)]
    pub category: Vec<String>,
}

/// `POST /v1/presets/:section/category`：改**一条**预设挂着的类别。
pub async fn set_category<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<SetCategoryBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let sections = store.sections()?;
    let section = admin::find(&sections, &section_id)
        .cloned()
        .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{section_id}")))?;
    let name = check_name(&body.name)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let item = store.one(&section, &name)?;
    let mut preset = item.body;
    let category = admin::sort_categories(body.category);
    preset["category"] = json!(category);
    let bytes = pretty(&preset);
    store.write_file(&store.manifest_path(&section, &name), &bytes)?;
    let mut vocab = store.vocabulary(&section);
    vocab.extend(category.iter().cloned());
    let vocab = admin::sort_categories(vocab);
    store.write_section_json(&section, "categories.json", &json!({ "v": 1, "categories": vocab }))?;
    Ok(Json(json!({
        "section": section.id,
        "name": name,
        "category": category,
        "categories": vocab,
    })))
}

/// `GET /v1/presets/:section/:name`：原文 + ETag（= version）。
pub async fn get_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
) -> Result<Response> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let file = store.one(&section, &name)?;
    let version = store.version(&section.id, &name)?;
    let body = json!({
        "section": section.id,
        "name": name,
        "version": version,
        "sha": sha256_hex(&file.bytes),
        "preset": file.body,
    });
    let mut response = Json(body).into_response();
    response.headers_mut().insert(
        header::ETAG,
        header::HeaderValue::from_str(&version.to_string())
            .map_err(|error| Refusal::Internal(format!("ETag: {error}")))?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
pub struct ValidateQuery {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub spec: Option<Value>,
}

/// `GET /v1/presets/:section/validate|resolve?name=` 与 `POST`（体 `{name}` 或 `{spec}`）：
/// 只读，不写盘、不提交。面板的编辑框靠它做校验 / 画 chip —— 跟建会话走同一份判据。
pub async fn validate_get<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Query(query): Query<ValidateQuery>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    validate_inner(&store, &section_id, "validate", ValidateBody { name: query.name, spec: None }).await
}

pub async fn validate_post<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<ValidateBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    validate_inner(&store, &section_id, "validate", body).await
}

pub async fn resolve_get<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Query(query): Query<ValidateQuery>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    validate_inner(&store, &section_id, "resolve", ValidateBody { name: query.name, spec: None }).await
}

pub async fn resolve_post<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<ValidateBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    validate_inner(&store, &section_id, "resolve", body).await
}

async fn validate_inner(
    store: &PresetStore,
    section_id: &str,
    route: &str,
    body: ValidateBody,
) -> Result<Json<Value>> {
    let sections = store.sections()?;
    let section = admin::find(&sections, section_id)
        .cloned()
        .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{section_id}")))?;
    let by_name = body.name;
    let (spec, dir_name) = match by_name {
        Some(raw) => {
            let name = check_name(&raw)?;
            let item = store.one(&section, &name)?;
            let problems = admin::problems_for(&section, &item.body, Some(&name));
            if !problems.is_empty() {
                return Err(Refusal::BadRequest(gate_message(&problems)));
            }
            (item.body, Some(name))
        }
        None => {
            let spec = body
                .spec
                .ok_or_else(|| Refusal::BadRequest("要么给 name，要么给 spec".into()))?;
            (spec, None)
        }
    };
    let ctx = store.ctx(&sections);
    let shape = admin::problems_for(&section, &spec, dir_name.as_deref());
    if !shape.is_empty() {
        return Ok(Json(json!({
            "ok": false, "errors": shape, "warnings": [], "spec": null, "resolved": null
        })));
    }
    let (errors, warnings, resolved) = if section.is_ultimate() {
        let errors = admin::ultimate_reference_problems(&spec, &ctx);
        let resolved = admin::resolve_ultimate(&spec, &ctx);
        (errors, Vec::new(), resolved)
    } else {
        let (errors, warnings) = admin::reference_problems(&spec, &ctx);
        let resolved = admin::resolve_spec(&spec, &ctx);
        (errors, warnings, resolved)
    };
    let ok = errors.is_empty();
    if route == "resolve" {
        return Ok(Json(json!({
            "ok": ok, "errors": errors, "warnings": warnings, "resolved": resolved
        })));
    }
    Ok(Json(json!({
        "ok": ok, "errors": errors, "warnings": warnings, "spec": spec, "resolved": resolved
    })))
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
    body.request_id.clone().filter(|value| !value.is_empty())
        .ok_or_else(|| Refusal::BadRequest("requestId is required".to_string()))
}

/// `PUT /v1/presets/:section/:name`：建或覆盖；dry_run=1 只校验不落盘。
pub async fn put_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
    Query(query): Query<PutQuery>,
    headers: HeaderMap,
    Json(body): Json<WriteBody>,
) -> Result<Response> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let name = check_name(&name)?;
    let request_id = request_id_of(&body)?;
    let preset = body.preset;
    let problems = admin::problems_for(&section, &preset, Some(&name));
    if !problems.is_empty() {
        return Err(Refusal::BadRequest(gate_message(&problems)));
    }
    let bytes = pretty(&preset);
    if query.dry_run.as_deref() == Some("1") {
        return Ok((StatusCode::OK, Json(json!({ "valid": true, "wrote": false }))).into_response());
    }
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section.id, &name, &WriteKind::Put, &request_id)? {
        return Ok((receipt.status, Json(receipt.body)).into_response());
    }
    let path = store.manifest_path(&section, &name);
    let existing = fs::read(&path).ok();
    let version = store.version(&section.id, &name)?;
    let if_match = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    if let Some(expected) = if_match {
        if existing.is_none() || expected.trim_matches('"') != version.to_string() {
            return Err(Refusal::PreconditionFailed(format!(
                "If-Match 不匹配：要 {expected}，盘上是 {version}"
            )));
        }
    }
    store.write_file(&path, &bytes)?;
    let version = store.append_revision(&section.id, &name, &bytes, &request_id)?;
    let response = json!({
        "section": section.id,
        "name": name,
        "version": version,
        "sha": sha256_hex(&bytes),
        "wrote": true,
    });
    store.remember(&section.id, &name, &WriteKind::Put, &request_id, &response)?;
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
pub async fn patch_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let name = check_name(&name)?;
    let request_id = body.request_id.clone().filter(|value| !value.is_empty())
        .ok_or_else(|| Refusal::BadRequest("requestId is required".to_string()))?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section.id, &name, &WriteKind::Patch, &request_id)? {
        return Ok(Json(receipt.body));
    }

    let mut current = name.clone();
    if let Some(archived) = body.archived {
        if archived {
            let paths = store.move_into_archive(&section, &current)?;
            store.commit(&paths, &format!("preset: 归档 {current}"), Some(&request_id))?;
            // 搬进归档之后这条已经不在活库里：再读清单只会 404。
            let response = json!({ "section": section.id, "name": current, "archived": true });
            store.remember(&section.id, &name, &WriteKind::Patch, &request_id, &response)?;
            return Ok(Json(response));
        }
        let paths = store.move_out_of_archive(&section, &current)?;
        store.commit(&paths, &format!("preset: 从归档恢复 {current}"), Some(&request_id))?;
    }

    if let Some(to) = body.name.as_deref() {
        let to = check_name(to)?;
        if to != current {
            rename_dir(&store, &section, &current, &to, &request_id)?;
            current = to;
        }
    }

    let mut preset = store.one(&section, &current)?.body;
    if let Some(category) = body.category.as_deref() {
        let category = admin::sort_categories(category.iter().cloned());
        preset["category"] = json!(category);
        let mut vocab = store.vocabulary(&section);
        vocab.extend(category);
        let vocab = admin::sort_categories(vocab);
        store.write_section_json(&section, "categories.json", &json!({ "v": 1, "categories": vocab }))?;
    }
    if let Some(order) = body.order {
        set_order(&store, &section, &current, order)?;
    }
    let problems = admin::problems_for(&section, &preset, Some(&current));
    if !problems.is_empty() {
        return Err(Refusal::BadRequest(gate_message(&problems)));
    }
    let bytes = pretty(&preset);
    store.write_file(&store.manifest_path(&section, &current), &bytes)?;
    let version = store.append_revision(&section.id, &current, &bytes, &request_id)?;
    let response = json!({ "section": section.id, "name": current, "version": version });
    store.remember(&section.id, &name, &WriteKind::Patch, &request_id, &response)?;
    Ok(Json(response))
}

/// 改名：目录、清单里的 `name`、引用它的终极模板的 `session`，三件一起做。
fn rename_dir(
    store: &PresetStore,
    section: &Section,
    from: &str,
    to: &str,
    request_id: &str,
) -> Result<()> {
    let old = store.preset_dir(section, from);
    let new = store.preset_dir(section, to);
    if !old.is_dir() {
        return Err(Refusal::NotFound(format!("工作区里没有 {from}/")));
    }
    if new.exists() {
        return Err(Refusal::Conflict(format!("已经有 {to}/ 了 —— 改名前先处理它")));
    }
    let mut paths = Vec::new();
    // 引用它的终极模板（跨段）。
    let mut refs: Vec<(Section, String)> = Vec::new();
    for other in store.sections()? {
        if !other.is_ultimate() || other.id == section.id {
            continue;
        }
        for item in store.scan(&other)? {
            if item.body.get("session").and_then(Value::as_str) == Some(from) {
                refs.push((other.clone(), item.name));
            }
        }
    }

    fs::rename(&old, &new).map_err(|error| Refusal::Internal(format!("改名失败: {error}")))?;
    let manifest = new.join(&section.manifest);
    let rewrite = |path: &Path, key: &str, value: &str| -> Result<()> {
        let mut raw: Value = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| Refusal::Internal(format!("读清单失败：{}", path.display())))?;
        raw[key] = json!(value);
        store.write_file(path, &pretty(&raw))
    };
    if let Err(error) = rewrite(&manifest, "name", to) {
        let _ = fs::rename(&new, &old);
        return Err(error);
    }
    for (other, name) in &refs {
        let path = store.manifest_path(other, name);
        if let Err(error) = rewrite(&path, "session", to) {
            let _ = rewrite(&manifest, "name", from);
            let _ = fs::rename(&new, &old);
            return Err(error);
        }
        paths.push(format!("{}/{}/{}", other.id, name, other.manifest));
    }
    paths.push(format!("{}/{}/{}", section.id, to, section.manifest));
    paths.push(format!("{}/{}/{}", section.id, from, section.manifest));
    store.commit(&paths, &format!("preset: 改名 {from} → {to}"), Some(request_id))?;
    Ok(())
}

fn set_order(store: &PresetStore, section: &Section, name: &str, order: i64) -> Result<()> {
    let mut names = store.order(section);
    names.retain(|entry| entry != name);
    let insert_at = (order.max(0) as usize).min(names.len());
    names.insert(insert_at, name.to_string());
    store.write_section_json(section, "order.json", &json!({ "v": 1, "order": names }))
}

/// `DELETE /v1/presets/:section/:name`：软删 = 搬进 `.archive/`（盘保留，会话创建时不列）。
pub async fn delete_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
) -> Result<StatusCode> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let name = check_name(&name)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let paths = store.move_into_archive(&section, &name)?;
    store.commit(&paths, &format!("preset: 归档 {name}"), None)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyBody {
    name: String,
    request_id: String,
}

/// `POST /v1/presets/:section/:name/copy`：复制成另一条（清单里的 `name` 跟着改）。
pub async fn copy_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
    Json(body): Json<CopyBody>,
) -> Result<Response> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let from = check_name(&name)?;
    let to = check_name(&body.name)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section.id, &to, &WriteKind::Copy, &body.request_id)? {
        return Ok((receipt.status, Json(receipt.body)).into_response());
    }
    let source = store.preset_dir(&section, &from);
    if !source.is_dir() {
        return Err(Refusal::NotFound(format!("preset 不存在: {}/{from}", section.id)));
    }
    let dest = store.preset_dir(&section, &to);
    if dest.exists() {
        return Err(Refusal::Conflict(format!("目标已存在: {}/{}", section.id, to)));
    }
    let mut preset = store.one(&section, &from)?.body;
    preset["name"] = json!(to);
    let problems = admin::problems_for(&section, &preset, Some(&to));
    if !problems.is_empty() {
        return Err(Refusal::BadRequest(gate_message(&problems)));
    }
    // 整目录复制（PRESET.md 等说明文件跟着走），清单单独重写成新名字。
    copy_dir(&source, &dest).map_err(|error| Refusal::Internal(format!("复制失败: {error}")))?;
    let bytes = pretty(&preset);
    store.write_file(&dest.join(&section.manifest), &bytes)?;
    let version = store.append_revision(&section.id, &to, &bytes, &body.request_id)?;
    let response = json!({ "section": section.id, "name": to, "version": version });
    store.remember(&section.id, &to, &WriteKind::Copy, &body.request_id, &response)?;
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct OrderBody {
    order: Vec<String>,
    #[serde(default, rename = "requestId")]
    _request_id: Option<String>,
}

pub async fn put_order<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<OrderBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let names: Vec<String> = store.scan(&section)?.into_iter().map(|item| item.name).collect();
    let mut ordered: Vec<String> = Vec::new();
    for name in body.order.iter() {
        if names.contains(name) && !ordered.contains(name) {
            ordered.push(name.clone());
        }
    }
    for name in &names {
        if !ordered.contains(name) {
            ordered.push(name.clone());
        }
    }
    store.write_section_json(&section, "order.json", &json!({ "v": 1, "order": ordered }))?;
    Ok(Json(json!({ "section": section.id, "order": ordered })))
}

pub async fn get_order<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    Ok(Json(json!({ "section": section.id, "order": store.order(&section) })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub spec: Option<Value>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `POST /v1/presets/:section/create`：起手式三种（`spec` / `from: device` / 空白）。
pub async fn create_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let sections = store.sections()?;
    let section = admin::find(&sections, &section_id)
        .cloned()
        .ok_or_else(|| Refusal::NotFound(format!("没有这一段：{section_id}")))?;
    let name = check_name(&body.name)?;
    let summary = body.summary.trim().to_string();
    if summary.is_empty() {
        return Err(Refusal::BadRequest(format!("{name}: summary 不能空 —— 人靠这一行决定开不开它")));
    }
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if store.preset_dir(&section, &name).exists() {
        return Err(Refusal::BadRequest(format!("{name}/ 已经在了 —— 换个名字，或者直接去改它")));
    }
    if store.section_dir(&section).join(ARCHIVE_DIR).join(&name).exists() {
        return Err(Refusal::BadRequest(format!(
            "{name}/ 在 .archive/ 里 —— 点「恢复」把它捡回来，别建第二个"
        )));
    }

    let mut spec = body
        .spec
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    spec["v"] = json!(1);
    spec["name"] = json!(name);
    spec["summary"] = json!(summary);
    spec["category"] = json!(admin::sort_categories(body.category.clone()));
    if let Some(description) = &body.description {
        spec["description"] = json!(description);
    }
    if section.is_ultimate() {
        let trigger = body.trigger.as_deref().map(str::trim).unwrap_or("");
        let session = body.session.as_deref().map(str::trim).unwrap_or("");
        if trigger.is_empty() {
            return Err(Refusal::BadRequest(format!("{name}: 得选一个触发器 —— 没有触发器就没有「什么时候」")));
        }
        if session.is_empty() {
            return Err(Refusal::BadRequest(format!("{name}: 得选一条会话模板 —— 触发器只管「什么时候」")));
        }
        spec["trigger"] = json!(trigger);
        spec["session"] = json!(session);
        spec["params"] = body.params.clone().unwrap_or_else(|| json!({}));
        spec["enabled"] = json!(body.enabled.unwrap_or(true));
    }
    if body.from.as_deref() == Some("device") {
        let (_, device) = store.models();
        if let Some((provider, model_id)) = device {
            spec["model"] = json!({ "provider": provider, "modelId": model_id });
        }
    }

    let problems = admin::problems_for(&section, &spec, Some(&name));
    if !problems.is_empty() {
        return Err(Refusal::BadRequest(gate_message(&problems)));
    }
    if section.is_ultimate() {
        let ctx = store.ctx(&sections);
        let reference = admin::ultimate_reference_problems(&spec, &ctx);
        if !reference.is_empty() {
            return Err(Refusal::BadRequest(gate_message(&reference)));
        }
    }

    store.write_file(&store.manifest_path(&section, &name), &pretty(&spec))?;
    if let Some(text) = body.body.as_deref().filter(|text| !text.trim().is_empty()) {
        store.write_file(&store.preset_dir(&section, &name).join("PRESET.md"), text.as_bytes())?;
    }
    let mut vocab = store.vocabulary(&section);
    vocab.extend(body.category.iter().cloned());
    let vocab = admin::sort_categories(vocab);
    store.write_section_json(&section, "categories.json", &json!({ "v": 1, "categories": vocab }))?;
    Ok(Json(json!({
        "ok": true,
        "section": section.id,
        "name": name,
        "categories": vocab,
    })))
}

#[derive(Debug, Deserialize)]
pub struct NameBody {
    pub name: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameBody {
    pub name: String,
    pub to: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

/// `POST /v1/presets/:section/{archive,restore,purge}`（体 `{name}`）。
///
/// 三个静态段各挂一个薄 handler：路径里只有一个参数，`AxumPath` 只能收一个。
pub async fn archive_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    lifecycle(&store, &section_id, "archive", body).await
}

pub async fn restore_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    lifecycle(&store, &section_id, "restore", body).await
}

pub async fn purge_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    lifecycle(&store, &section_id, "purge", body).await
}

async fn lifecycle(
    store: &PresetStore,
    section_id: &str,
    op: &str,
    body: NameBody,
) -> Result<Json<Value>> {
    let section = store.section(section_id)?;
    let name = check_name(&body.name)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let (paths, subject) = match op {
        "archive" => (
            store.move_into_archive(&section, &name)?,
            format!("preset: 归档 {name}"),
        ),
        "restore" => (
            store.move_out_of_archive(&section, &name)?,
            format!("preset: 从归档恢复 {name}"),
        ),
        "purge" => {
            let path = store.section_dir(&section).join(ARCHIVE_DIR).join(&name);
            if !path.is_dir() {
                return Err(Refusal::NotFound(format!("归档区里没有 {name}")));
            }
            fs::remove_dir_all(&path)
                .map_err(|error| Refusal::Internal(format!("彻底删除失败: {error}")))?;
            (
                vec![format!("{}/{ARCHIVE_DIR}/{name}", section.id)],
                format!("preset: 彻底删除 {name}"),
            )
        }
        _ => {
            return Err(Refusal::BadRequest(format!(
                "不认识的 op: {op}（只有 archive / restore / purge）"
            )))
        }
    };
    store.commit(&paths, &subject, body.request_id.as_deref())?;
    Ok(Json(json!({
        "ok": true,
        "section": section.id,
        "name": name,
        "op": op,
    })))
}

/// `POST /v1/presets/:section/rename`（体 `{name, to}`）。
pub async fn rename_preset<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<RenameBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let from = check_name(&body.name)?;
    let to = check_name(&body.to)?;
    if from == to {
        return Err(Refusal::BadRequest("新名字和旧名字一样".into()));
    }
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    rename_dir(&store, &section, &from, &to, body.request_id.as_deref().unwrap_or(""))?;
    Ok(Json(json!({ "ok": true, "section": section.id, "name": from, "to": to })))
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileSaveBody {
    pub name: String,
    pub path: String,
    pub text: String,
    #[serde(default)]
    pub base: Option<String>,
}

/// `GET /v1/presets/:section/files?name=`：这条预设目录里有哪些文件。
pub async fn files<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let name = check_name(&query.name)?;
    let dir = store.preset_dir(&section, &name);
    if !dir.is_dir() {
        return Err(Refusal::NotFound(format!("preset 不存在: {}/{name}", section.id)));
    }
    let mut out = Vec::new();
    collect_files(&dir, "", &mut out)
        .map_err(|error| Refusal::Internal(format!("列文件失败: {error}")))?;
    out.sort_by(|a: &Value, b: &Value| {
        a["path"].as_str().unwrap_or("").cmp(b["path"].as_str().unwrap_or(""))
    });
    Ok(Json(json!({ "section": section.id, "name": name, "files": out })))
}

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<Value>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, &rel, out)?;
        } else {
            out.push(json!({ "path": rel, "size": entry.metadata().map(|m| m.len()).unwrap_or(0) }));
        }
    }
    Ok(())
}

/// `GET|PUT /v1/presets/:section/file`：单个文件的原文 / 写回（ETag = 字节的 sha256）。
pub async fn file_get<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let name = check_name(&query.name)?;
    let rel = check_rel_path(query.path.as_deref().unwrap_or(""))?;
    let path = store.preset_dir(&section, &name).join(&rel);
    let bytes = fs::read(&path)
        .map_err(|_| Refusal::NotFound(format!("文件不存在: {name}/{rel}")))?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|_| Refusal::BadRequest(format!("{rel} 不是 UTF-8 文本")))?;
    Ok(Json(json!({
        "section": section.id, "name": name, "path": rel, "text": text, "etag": sha256_hex(&bytes)
    })))
}

pub async fn file_put<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<FileSaveBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let name = check_name(&body.name)?;
    let rel = check_rel_path(&body.path)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if store.one(&section, &name).is_err() {
        return Err(Refusal::NotFound(format!("preset 不存在: {}/{name}", section.id)));
    }
    let path = store.preset_dir(&section, &name).join(&rel);
    if let Some(base) = body.base.as_deref() {
        let current = fs::read(&path).ok();
        let current_etag = current.as_deref().map(sha256_hex);
        if current_etag.as_deref() != Some(base) {
            return Err(Refusal::Conflict(format!(
                "文件已被改过（盘上是 {}，你手里是 {base}）",
                current_etag.unwrap_or_else(|| "不存在".into())
            )));
        }
    }
    // 写**这一段的清单文件**时先过形状闸：写坏了不是「未保存」，而是这条当场失效。
    if rel == section.manifest {
        let value: Value = serde_json::from_str(&body.text)
            .map_err(|error| Refusal::BadRequest(format!("{} 不是合法 JSON: {error}", section.manifest)))?;
        let problems = admin::problems_for(&section, &value, Some(&name));
        if !problems.is_empty() {
            return Err(Refusal::BadRequest(gate_message(&problems)));
        }
        if section.is_ultimate() {
            let sections = store.sections()?;
            let ctx = store.ctx(&sections);
            let reference = admin::ultimate_reference_problems(&value, &ctx);
            if !reference.is_empty() {
                return Err(Refusal::BadRequest(gate_message(&reference)));
            }
        }
        let bytes = body.text.clone().into_bytes();
        store.write_file(&path, &bytes)?;
        store.append_revision(&section.id, &name, &bytes, "file")?;
    } else {
        store.write_file(&path, body.text.as_bytes())?;
    }
    let bytes = body.text.into_bytes();
    Ok(Json(json!({
        "section": section.id, "name": name, "path": rel, "etag": sha256_hex(&bytes)
    })))
}

fn check_rel_path(raw: &str) -> Result<String> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(Refusal::BadRequest("path 不能为空".into()));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(Refusal::BadRequest(format!("路径必须是预设目录里的相对路径：{path}")));
    }
    if path.split('/').any(|part| part == ".." || part == ".") {
        return Err(Refusal::BadRequest(format!("路径里有 .. 或 .：{path}")));
    }
    Ok(path.to_string())
}

/// `GET /v1/presets/:section/pending`：仓状态 + 待提交单元（一条预设一个 unit）。
pub async fn pending<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let Some(repo) = store.git() else {
        return Ok(Json(json!({
            "head": null, "dirty": false, "ahead": 0, "behind": 0,
            "remote": null, "units": [], "aheadCommits": [],
        })));
    };
    let status = repo.status().map_err(repo_error)?;
    let remote = repo.remote_url().ok().flatten();
    let prefix = format!("{}/", section.id);
    let mut units: BTreeMap<String, Value> = BTreeMap::new();
    for unit in repo.pending().map_err(repo_error)? {
        for path in unit.paths.iter().filter(|path| path.starts_with(&prefix)) {
            let rest = &path[prefix.len()..];
            let (key, file) = rest.split_once('/').unwrap_or((rest, ""));
            let entry = units.entry(key.to_string()).or_insert_with(|| json!({
                "key": key,
                "kind": "preset",
                "name": key,
                "tracked": true,
                "files": [],
                "added": 0,
                "removed": 0,
                "code": false,
                "hasDeleted": false,
            }));
            let diff = repo.pending_diff(path).ok();
            let (added, removed) = diff
                .as_ref()
                .map(|diff| {
                    let before: BTreeSet<&str> = diff.before.lines().collect();
                    let after: BTreeSet<&str> = diff.after.lines().collect();
                    (
                        after.difference(&before).count() as u64,
                        before.difference(&after).count() as u64,
                    )
                })
                .unwrap_or((0, 0));
            if let Some(files) = entry["files"].as_array_mut() {
                files.push(json!({
                    "path": file,
                    "status": if unit.state == "untracked" { "A" } else { "M" },
                    "deleted": false,
                    "added": added,
                    "removed": removed,
                }));
            }
            entry["added"] = json!(entry["added"].as_u64().unwrap_or(0) + added);
            entry["removed"] = json!(entry["removed"].as_u64().unwrap_or(0) + removed);
        }
    }
    Ok(Json(json!({
        "head": status.head,
        "dirty": status.dirty,
        "ahead": status.ahead,
        "behind": status.behind,
        "remote": remote,
        "units": units.into_values().collect::<Vec<_>>(),
        "aheadCommits": [],
    })))
}

#[derive(Debug, Deserialize, Default)]
pub struct MessageBody {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// `POST /v1/presets/:section/publish`：形状闸 → 提交这一段。闸不过一个字节都不动。
pub async fn publish<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<MessageBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let problems = store.gate(&section)?;
    if !problems.is_empty() {
        return Err(Refusal::BadRequest(gate_message(&problems)));
    }
    let Some(repo) = store.git() else {
        return Ok(Json(json!({ "ok": true, "commit": null, "pushed": null, "noRemote": true })));
    };
    let prefix = format!("{}/", section.id);
    let paths: Vec<String> = repo
        .pending()
        .map_err(repo_error)?
        .into_iter()
        .flat_map(|unit| unit.paths)
        .filter(|path| path.starts_with(&prefix))
        .collect();
    if paths.is_empty() {
        return Ok(Json(json!({ "ok": true, "commit": null, "pushed": null })));
    }
    let message = body.message.unwrap_or_else(|| "preset: 更新预设仓库".into());
    let result = repo.commit(&paths, &message, None).map_err(repo_error)?;
    Ok(Json(json!({
        "ok": !matches!(result.pushed, nous_repo::Pushed::No),
        "commit": { "hash": result.commit },
        "pushed": result.pushed,
    })))
}

/// `POST /v1/presets/:section/sync`：提交指定的路径（不指定 = 这一段的全部待提交）。
pub async fn sync<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
    Json(body): Json<MessageBody>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    let Some(repo) = store.git() else {
        return Ok(Json(json!({ "ok": true, "commit": null, "pushed": null, "noRemote": true })));
    };
    let prefix = format!("{}/", section.id);
    let paths: Vec<String> = match body.paths {
        Some(paths) => paths.into_iter().map(|path| format!("{prefix}{path}")).collect(),
        None => repo
            .pending()
            .map_err(repo_error)?
            .into_iter()
            .flat_map(|unit| unit.paths)
            .filter(|path| path.starts_with(&prefix))
            .collect(),
    };
    if paths.is_empty() {
        return Ok(Json(json!({ "ok": true, "commit": null, "pushed": null })));
    }
    let message = body.message.unwrap_or_else(|| "preset: 同步预设仓库".into());
    let result = repo.commit(&paths, &message, None).map_err(repo_error)?;
    Ok(Json(json!({
        "ok": !matches!(result.pushed, nous_repo::Pushed::No),
        "commit": { "hash": result.commit },
        "pushed": result.pushed,
    })))
}

/// `POST /v1/presets/:section/reload`：没有常驻名册，重扫就是「每次读都扫」，如实回 ok。
pub async fn reload<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath(section_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let count = store.scan(&section)?.len();
    Ok(Json(json!({ "ok": true, "section": section.id, "count": count })))
}

pub async fn list_revisions<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let _ = store.one(&section, &name)?;
    let revisions = store.revisions(&section.id, &name)?;
    let items = revisions.iter().map(|revision| json!({
        "n": revision.n,
        "at": revision.at,
        "actor": revision.actor,
        "sha": revision.sha,
        "summary": revision.body.lines().next().unwrap_or(""),
        "requestId": revision.request_id,
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "section": section.id, "name": name, "items": items })))
}

pub async fn get_revision<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name, n)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let n = n.parse::<i64>().map_err(|_| Refusal::BadRequest(format!("非法修订号: {n}")))?;
    let revision = store.revisions(&section.id, &name)?
        .into_iter().find(|r| r.n == n)
        .ok_or_else(|| Refusal::NotFound(format!("修订不存在: {}/{name}#{n}", section.id)))?;
    Ok(Json(json!({
        "section": section.id,
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

pub async fn rollback<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    AxumPath((section_id, name)): AxumPath<(String, String)>,
    Json(body): Json<RollbackBody>,
) -> Result<Response> {
    let store = store_of(&host)?;
    let section = store.section(&section_id)?;
    let _serial = store.lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receipt) = store.receipt(&section.id, &name, &WriteKind::Rollback, &body.request_id)? {
        return Ok((receipt.status, Json(receipt.body)).into_response());
    }
    let revision = store.revisions(&section.id, &name)?
        .into_iter().find(|r| r.n == body.n)
        .ok_or_else(|| Refusal::NotFound(format!("修订不存在: {}/{name}#{}", section.id, body.n)))?;
    let bytes = revision.body.into_bytes();
    store.write_file(&store.manifest_path(&section, &name), &bytes)?;
    let version = store.append_revision(&section.id, &name, &bytes, &body.request_id)?;
    let response = json!({ "section": section.id, "name": name, "version": version, "rolledBackTo": body.n });
    store.remember(&section.id, &name, &WriteKind::Rollback, &body.request_id, &response)?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// 本插件的 32 条声明（19 条 `presets-admin` + 13 条 `presets`）。
///
/// 路径只写在这里：`nous-routes` 把 `{section}` 折成注册表要的 `:section`，
/// `capabilities` / `/v1/_routes` / `/v1/openapi.json` 三处读的是同一份。
/// 一条端点同时挂多个方法（`GET|POST`、`GET|PUT`）时，声明里**每个方法一行** ——
/// 注册表按「方法 + 路径」记账，一行塞两个方法会让 openapi 少一条。
pub fn routes<H>() -> Vec<Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    use axum::routing::{delete, get, patch, post, put};

    macro_rules! route {
        ($method:literal, $path:literal, $module:expr, $build:expr) => {
            Route::new($method, $path, $module, $build)
        };
    }

    // 段面与名册归 `presets`；写面 / 校验 / 文件 / 发布归 `presets-admin`（`docs/plugins.md`）。
    const P: &str = "presets";
    const A: &str = MODULE;
    vec![
        route!("GET", "/v1/presets", P, || get(sections::<H>)),
        route!("GET", "/v1/presets/model", P, || get(model::<H>)),
        route!("GET", "/v1/presets/{section}", P, || get(roster::<H>)),
        route!("GET", "/v1/presets/{section}/order", A, || get(get_order::<H>)),
        route!("PUT", "/v1/presets/{section}/order", A, || put(put_order::<H>)),
        route!("GET", "/v1/presets/{section}/categories", A, || get(categories::<H>)),
        route!("POST", "/v1/presets/{section}/categories", A, || post(mutate_categories::<H>)),
        route!("POST", "/v1/presets/{section}/category", A, || post(set_category::<H>)),
        route!("POST", "/v1/presets/{section}/create", A, || post(create_preset::<H>)),
        route!("POST", "/v1/presets/{section}/rename", A, || post(rename_preset::<H>)),
        route!("POST", "/v1/presets/{section}/archive", A, || post(archive_preset::<H>)),
        route!("POST", "/v1/presets/{section}/restore", A, || post(restore_preset::<H>)),
        route!("POST", "/v1/presets/{section}/purge", A, || post(purge_preset::<H>)),
        route!("GET", "/v1/presets/{section}/validate", A, || get(validate_get::<H>)),
        route!("POST", "/v1/presets/{section}/validate", A, || post(validate_post::<H>)),
        route!("GET", "/v1/presets/{section}/resolve", A, || get(resolve_get::<H>)),
        route!("POST", "/v1/presets/{section}/resolve", A, || post(resolve_post::<H>)),
        route!("GET", "/v1/presets/{section}/files", A, || get(files::<H>)),
        route!("GET", "/v1/presets/{section}/file", A, || get(file_get::<H>)),
        route!("PUT", "/v1/presets/{section}/file", A, || put(file_put::<H>)),
        route!("GET", "/v1/presets/{section}/pending", A, || get(pending::<H>)),
        route!("POST", "/v1/presets/{section}/sync", A, || post(sync::<H>)),
        route!("POST", "/v1/presets/{section}/publish", A, || post(publish::<H>)),
        route!("POST", "/v1/presets/{section}/reload", A, || post(reload::<H>)),
        route!("GET", "/v1/presets/{section}/{name}", P, || get(get_preset::<H>)),
        route!("PUT", "/v1/presets/{section}/{name}", P, || put(put_preset::<H>)),
        route!("PATCH", "/v1/presets/{section}/{name}", P, || patch(patch_preset::<H>)),
        route!("DELETE", "/v1/presets/{section}/{name}", P, || delete(delete_preset::<H>)),
        route!("POST", "/v1/presets/{section}/{name}/copy", P, || post(copy_preset::<H>)),
        route!("GET", "/v1/presets/{section}/{name}/revisions", P, || get(list_revisions::<H>)),
        route!("GET", "/v1/presets/{section}/{name}/revisions/{n}", P, || get(get_revision::<H>)),
        route!("POST", "/v1/presets/{section}/{name}/rollback", P, || post(rollback::<H>)),
    ]
}
