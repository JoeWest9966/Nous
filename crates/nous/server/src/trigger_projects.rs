//! 触发器工程面（n8qkkj 的宿主）：一个触发器 = 一个独立 cargo 项目。
//!
//! 与 `/v1/triggers`（定义 + 调度 + 点火账）**不是同一层**：
//! 那边是「库里的一行定义」，这边是「盘上的一个项目目录」，有源码、能编译、能发布。
//! 这一层只做 HTTP ↔ `nous_repo` 的翻译；文件读写、etag、git、归档三代全在
//! `nous_repo`（唯一 git 进程面），这里不拼第二条 `git add`。
//!
//! 数据根：`NOUS_TRIGGERS`，默认 `/opt/Nous-Trigger`（与旧 n8qkkj 同一个目录）。
//! 目录形状：`<root>/<name>/{nous.json,Cargo.toml,src/main.rs,README.md}`、
//! `<root>/categories.json`、`<root>/index.json`、`<root>/.archive/<name>/`。
//!
//! **保存 ≠ 发布**：`PUT file` 只落盘；`POST publish` 才
//! `cargo check → cargo build → index.json → commit → push`，中间进度拿不到，
//! 失败用 422 + `stage` 说是哪一步。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nous_repo::{Pushed, Repo, RepoError, WriteFile};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};

/// 构建超时：旧 n8qkkj 是 180s；首次编译要比 check 长，给同一份预算。
const BUILD_TIMEOUT: Duration = Duration::from_secs(180);
/// 单个触发器目录里允许读写的最大文件（与 `nous_repo` 同一道闸）。
const MAX_BYTES: usize = nous_repo::MAX_FILE_BYTES;
/// 只认这些扩展名/文件名，避免把 target/ 里的东西当源码列出来。
const SKIP_DIRS: [&str; 4] = [".git", "target", ".archive", "node_modules"];

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/v1/trigger-projects", get(list))
        .route("/v1/trigger-projects/files", get(files))
        .route("/v1/trigger-projects/file", get(read).put(write))
        .route("/v1/trigger-projects/create", post(create))
        .route("/v1/trigger-projects/publish", post(publish))
        .route("/v1/trigger-projects/archive", post(archive))
        .route("/v1/trigger-projects/restore", post(restore))
        .route("/v1/trigger-projects/purge", post(purge))
        .route("/v1/trigger-projects/categories", get(categories).post(edit_vocab))
        .route("/v1/trigger-projects/category", post(set_category))
        .route("/v1/trigger-projects/sync", post(sync))
}

// ── 仓 / 路径 ───────────────────────────────────────────────────────────

fn repo(state: &AppState) -> Result<Arc<Repo>> {
    state.repo("triggers").ok_or_else(|| {
        ServerError::Conflict(
            "触发器仓没装配：NOUS_TRIGGERS 指向的目录不存在或不是 git 仓".into(),
        )
    })
}

/// 名字是目录名，也是 `nous.json.name` 和二进制名；**建完不可变**。
fn check_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let first = chars.next().unwrap_or(' ');
    if !first.is_ascii_lowercase()
        || !(3..=64).contains(&name.len())
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
    {
        return Err(ServerError::BadRequest(format!(
            "名字不合法：{name}（小写字母开头，只含小写 / 数字 / . _ -，3–64 位）"
        )));
    }
    Ok(())
}

/// 触发器目录下的文件路径（`src/main.rs`），不接受绝对路径 / `..`。
fn check_rel(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(ServerError::BadRequest(format!("文件路径不合法：{path}")));
    }
    Ok(())
}

fn project_dir(repo: &Repo, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let dir = repo.root().join(name);
    if !dir.is_dir() {
        return Err(ServerError::NotFound(format!("没有这个触发器：{name}")));
    }
    Ok(dir)
}

fn repo_error(error: RepoError) -> ServerError {
    match error {
        RepoError::BadRequest(m) | RepoError::InvalidPath(m) => ServerError::BadRequest(m),
        RepoError::Validation { field, problem } => ServerError::Validation { field, problem },
        RepoError::NotFound(m) => ServerError::NotFound(m),
        RepoError::Conflict { message, .. } => ServerError::Conflict(format!(
            "{message} —— 文件被别处改过了，先重新读盘再存"
        )),
        RepoError::Busy(m) => ServerError::Conflict(m),
        RepoError::Io(m) => ServerError::Internal(m),
        RepoError::LockPoisoned => ServerError::Internal("触发器仓写锁中毒".into()),
    }
}

fn pushed_json(pushed: Pushed) -> Value {
    match pushed {
        Pushed::Yes => json!(true),
        Pushed::No => json!(false),
        Pushed::Unknown => Value::Null,
    }
}

// ── 清单 ───────────────────────────────────────────────────────────────

/// `GET /v1/trigger-projects`：旧 n8qkkj 的清单形状。
async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    Ok(Json(list_value(&repo)?))
}

fn list_value(repo: &Repo) -> Result<Value> {
    let status = repo.status().map_err(repo_error)?;
    let pending = repo.pending().map_err(repo_error)?;
    // `state == "unpushed"` 是「提交了没推上去」（`ahead`），不是工作树脏 —— 旧版
    // 顶栏把这两件事分成两个 chip，别把未推送算成「有改动」。
    let mut dirty_files: std::collections::BTreeMap<String, usize> = Default::default();
    let mut dirty_count = 0usize;
    for unit in &pending {
        if unit.state == "unpushed" {
            continue;
        }
        for path in &unit.paths {
            let name = path.split('/').next().unwrap_or(path).to_string();
            *dirty_files.entry(name).or_default() += 1;
            dirty_count += 1;
        }
    }

    let mut triggers = Vec::new();
    let mut archived = Vec::new();
    if let Ok(entries) = std::fs::read_dir(repo.root()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                if name == ".archive" {
                    archived = archived_value(&path);
                }
                continue;
            }
            let item = item_of(repo, &name, dirty_files.get(&name).copied().unwrap_or(0));
            triggers.push(item);
        }
    }
    triggers.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .cmp(&b.get("name").and_then(Value::as_str))
    });
    let categories = vocab_with_usage(repo.root());
    Ok(json!({
        "triggers": triggers,
        "repo": {
            "path": status.root,
            "head": status.head.map(short_head),
            "branch": status.branch,
            "ahead": status.ahead,
            "behind": status.behind,
            "dirty": status.dirty,
            "dirtyCount": dirty_count,
        },
        "categories": categories,
        "archived": archived,
    }))
}

fn short_head(head: String) -> String {
    head.chars().take(7).collect()
}

fn archived_value(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let manifest = read_manifest(&path);
            let at = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| ymd(d.as_secs() as i64))
                .unwrap_or_default();
            out.push(json!({
                "name": name,
                "at": at,
                "version": manifest.as_ref().and_then(|m| m.get("version")).cloned().unwrap_or(Value::Null),
                "summary": manifest.as_ref().and_then(|m| m.get("summary")).cloned().unwrap_or(Value::Null),
                "category": manifest.as_ref().and_then(|m| m.get("category")).cloned().unwrap_or(json!([])),
            }));
        }
    }
    out.sort_by(|a, b| {
        b.get("at").and_then(Value::as_str).cmp(&a.get("at").and_then(Value::as_str))
    });
    out
}

/// 一条卡片：清单投影 + 仓库状态。`error` 是内容闸的第一句，页面直接显示。
fn item_of(repo: &Repo, name: &str, dirty_files: usize) -> Value {
    let dir = repo.root().join(name);
    let manifest = read_manifest(&dir);
    let (error, params, version, summary, category, binary) = match &manifest {
        None => (
            Some("nous.json 读不出来（缺文件或不是 JSON）".to_string()),
            json!([]),
            Value::Null,
            Value::Null,
            json!([]),
            Value::Null,
        ),
        Some(value) => {
            let issue = nous_repo::validate_manifest(&dir);
            (
                issue,
                value.get("params").cloned().unwrap_or(json!([])),
                value.get("version").cloned().unwrap_or(Value::Null),
                value.get("summary").cloned().unwrap_or(Value::Null),
                value.get("category").cloned().unwrap_or(json!([])),
                value.get("binary").cloned().unwrap_or(Value::Null),
            )
        }
    };
    // tracked = HEAD 里有这个目录的任何文件；建完没发布就是 false。
    let tracked = repo
        .tracked_paths(name)
        .map(|paths| !paths.is_empty())
        .unwrap_or(false);
    json!({
        "name": name,
        "version": version,
        "summary": summary,
        "category": category,
        "binary": binary,
        "params": params,
        "tracked": tracked,
        "dirty": dirty_files > 0,
        "dirtyFiles": dirty_files,
        "error": error,
    })
}

/// 这批路径里有没有和 HEAD 不同的内容（用于「重复发布」的幂等判定）。
fn has_changes(repo: &Repo, paths: &[String]) -> Result<bool> {
    for path in paths {
        let diff = repo.pending_diff(path).map_err(repo_error)?;
        if diff.before != diff.after {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_manifest(dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join("nous.json")).ok()?;
    serde_json::from_str(&text).ok()
}

// ── 文件面 ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NameQuery {
    name: String,
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    name: String,
    path: String,
}

/// `GET /v1/trigger-projects/files?name=`：递归清单，路径相对触发器目录。
async fn files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NameQuery>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let dir = project_dir(&repo, &query.name)?;
    let mut files = Vec::new();
    walk(&dir, "", &mut files);
    for file in &mut files {
        let Some(rel_path) = file.get("path").and_then(Value::as_str) else { continue };
        let rel = format!("{}/{}", query.name, rel_path);
        let Ok(diff) = repo.pending_diff(&rel) else { continue };
        let before = diff.before.lines().count() as i64;
        let after = diff.after.lines().count() as i64;
        let changed = diff.before != diff.after;
        if let Some(object) = file.as_object_mut() {
            object.insert("changed".into(), json!(changed));
            object.insert("added".into(), json!((after - before).max(0)));
            object.insert("removed".into(), json!((before - after).max(0)));
        }
    }
    files.sort_by(|a, b| {
        let rank = |p: &str| match p {
            "nous.json" => 0,
            "src/main.rs" => 1,
            "Cargo.toml" => 2,
            "README.md" => 3,
            _ => 9,
        };
        rank(a.get("path").and_then(Value::as_str).unwrap_or(""))
            .cmp(&rank(b.get("path").and_then(Value::as_str).unwrap_or("")))
            .then_with(|| {
                a.get("path")
                    .and_then(Value::as_str)
                    .cmp(&b.get("path").and_then(Value::as_str))
            })
    });
    Ok(Json(json!({ "files": files })))
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let full = entry.path();
        if full.is_dir() {
            walk(&full, &path, out);
            continue;
        }
        if !full.is_file() {
            continue;
        }
        let size = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
        out.push(json!({
            "path": path,
            "size": size,
            "kind": kind_of(&path),
            // 相对 HEAD 的增删在下一条 `file` 里算（这里只看工作区大小，列表要快）。
            "changed": false,
            "added": 0,
            "removed": 0,
        }));
    }
}

fn kind_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "json" => "json",
        "rs" => "rust",
        "toml" => "toml",
        "md" => "markdown",
        "txt" => "text",
        _ => "other",
    }
}

/// `GET /v1/trigger-projects/file?name=&path=`：正文 + etag + HEAD 基线。
async fn read(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let _dir = project_dir(&repo, &query.name)?;
    check_rel(&query.path)?;
    let rel = format!("{}/{}", query.name, query.path);
    let file = repo.read_file(&rel).map_err(repo_error)?;
    if file.content.len() > MAX_BYTES {
        return Err(ServerError::BadRequest(format!(
            "文件超过 {}KB：{}",
            MAX_BYTES / 1024,
            query.path
        )));
    }
    let text = String::from_utf8(file.content).map_err(|_| {
        ServerError::BadRequest(format!("拒绝非 UTF-8 文件：{}", query.path))
    })?;
    let base = repo
        .pending_diff(&rel)
        .map(|diff| {
            if diff.before.trim().is_empty() {
                Value::Null
            } else {
                Value::String(diff.before)
            }
        })
        .unwrap_or(Value::Null);
    Ok(Json(json!({
        "path": query.path,
        "kind": kind_of(&query.path),
        "text": text,
        "etag": file.etag,
        "base": base,
    })))
}

#[derive(Debug, Deserialize)]
struct SaveBody {
    name: String,
    path: String,
    text: String,
    /// 旧客户端叫 `base`，是上一次读到的 etag；不匹配 → 409。
    #[serde(default)]
    base: Option<String>,
}

/// `PUT /v1/trigger-projects/file`：只落盘，不提交、不编译。
async fn write(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let _dir = project_dir(&repo, &body.name)?;
    check_rel(&body.path)?;
    if body.text.len() > MAX_BYTES {
        return Err(ServerError::BadRequest(format!(
            "文件超过 {}KB：{}",
            MAX_BYTES / 1024,
            body.path
        )));
    }
    let rel = format!("{}/{}", body.name, body.path);
    let written = repo
        .write_file(WriteFile {
            path: &rel,
            content: body.text.as_bytes(),
            base_etag: body.base.as_deref(),
        })
        .map_err(repo_error)?;
    Ok(Json(json!({
        "path": body.path,
        "etag": written.etag,
    })))
}

// ── 新建 ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CreateBody {
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    category: Vec<String>,
}

/// `POST /v1/trigger-projects/create`：生成能编过的骨架，**不提交**（建完是「未发布」）。
async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    check_name(&body.name)?;
    let dir = repo.root().join(&body.name);
    if dir.exists() {
        return Err(ServerError::Conflict(format!("目录已存在：{}", body.name)));
    }
    if repo.root().join(".archive").join(&body.name).exists() {
        return Err(ServerError::Conflict(format!(
            "归档里还有同名目录：{}（code 永不复用）",
            body.name
        )));
    }
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| ServerError::Internal(format!("建目录失败：{e}")))?;
    let summary = if body.summary.trim().is_empty() {
        "还没写说明"
    } else {
        body.summary.trim()
    };
    let manifest = json!({
        "name": body.name,
        "version": "0.1.0",
        "summary": summary,
        "category": body.category,
        "binary": body.name,
        "params": [],
    });
    let files = [
        ("nous.json", serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n"),
        (
            "Cargo.toml",
            format!(
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde_json = \"1\"\n",
                body.name
            ),
        ),
        (
            "src/main.rs",
            format!(
                "//! {} —— {}。\n//! 契约：stdin 一行 JSON 进，stdout 一行 JSON 出。\n\nfn main() {{\n    println!(r#\"{{\"fire\":false,\"reason\":\"还没写条件\"}}\"#);\n}}\n",
                body.name, summary
            ),
        ),
        ("README.md", format!("# {}\n\n{}\n", body.name, summary)),
    ];
    for (path, content) in files {
        std::fs::write(dir.join(path), content)
            .map_err(|e| ServerError::Internal(format!("写 {path} 失败：{e}")))?;
    }
    Ok(Json(json!({ "name": body.name, "untracked": true })))
}

// ── 发布 ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PublishBody {
    name: String,
}

fn unprocessable(stage: &str, error: String) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": error, "stage": stage })),
    )
        .into_response()
}

/// `POST /v1/trigger-projects/publish`：五步同步长请求。失败 422 + `stage`。
async fn publish(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PublishBody>,
) -> Result<Response> {
    let repo = repo(&state)?;
    let dir = project_dir(&repo, &body.name)?;
    if !dir.join("Cargo.toml").is_file() {
        return Ok(unprocessable("check", "缺 Cargo.toml，没法编译".into()));
    }
    if !dir.join("nous.json").is_file() {
        return Ok(unprocessable("index", "缺 nous.json".into()));
    }
    let manifest = read_manifest(&dir).unwrap_or_else(|| json!({}));
    if let Some(problem) = nous_repo::validate_manifest(&dir) {
        return Ok(unprocessable("index", problem));
    }

    let target_dir = std::env::temp_dir().join("nous-trigger-build").join(&body.name);
    if let Err(error) = run_cargo("check", &dir, &target_dir).await {
        return Ok(unprocessable("check", error));
    }
    if let Err(error) = run_cargo("build", &dir, &target_dir).await {
        return Ok(unprocessable("build", error));
    }
    if let Err(error) = write_index(&repo) {
        return Ok(unprocessable("index", error));
    }

    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0.1.0");
    // 显式文件清单，不给目录 pathspec：旧仓可能没有 .gitignore，`git add <name>/`
    // 会把 `target/` 一起带进提交。也只 add 这一个项目（不搭车）。
    let mut paths = project_files(&dir, &body.name);
    paths.push("index.json".to_string());
    // 重复发布同一条（源码与清单都没变）不该红：没有新提交就如实回执。
    // 只看这清单里的文件 —— 仓里别处的 untracked（比如项目下的 target/）不影响。
    if !has_changes(repo.as_ref(), &paths)? {
        let snapshot = repo.status().map_err(repo_error)?;
        return Ok(Json(json!({
            "committed": false,
            "pushed": Value::Null,
            "pushError": Value::Null,
            "head": snapshot.head.map(short_head),
            "branch": snapshot.branch,
            "ahead": snapshot.ahead,
            "version": version,
            "note": "没有改动：源码与清单都和 HEAD 一致，未产生新提交",
        }))
        .into_response());
    }
    let message = format!("trigger({}): v{}", body.name, version);
    let commit = match repo.commit(&paths, &message, None) {
        Ok(commit) => commit,
        Err(error) => {
            // 这批路径 add 完 staged 没变化时 `git commit` 会把提示写到 stdout，
            // 错误串只剩前缀 —— 那说明「没东西可提交」，按幂等回执，不冒充失败。
            let text = error.to_string();
            let empty = text.trim_end().ends_with('：') || text.contains("nothing to commit");
            if empty {
                let snapshot = repo.status().map_err(repo_error)?;
                return Ok(Json(json!({
                    "committed": false,
                    "pushed": Value::Null,
                    "pushError": Value::Null,
                    "head": snapshot.head.map(short_head),
                    "branch": snapshot.branch,
                    "ahead": snapshot.ahead,
                    "version": version,
                    "note": "没有改动：未产生新提交",
                }))
                .into_response());
            }
            return Ok(unprocessable("commit", text));
        }
    };
    let status = repo.status().map_err(repo_error)?;
    let push_error = match commit.pushed {
        Pushed::Yes => Value::Null,
        Pushed::Unknown => Value::String("没有远端，没推".into()),
        Pushed::No => Value::String("推送未成功：本地提交已保留（查 git 凭据 / 远端）".into()),
    };
    Ok(Json(json!({
        "committed": true,
        "pushed": pushed_json(commit.pushed),
        "pushError": push_error,
        "head": status.head.map(short_head),
        "branch": status.branch,
        "ahead": status.ahead,
        "commit": commit.commit,
    }))
    .into_response())
}

async fn run_cargo(step: &str, dir: &Path, target_dir: &Path) -> std::result::Result<(), String> {
    let mut command = tokio::process::Command::new("cargo");
    command
        .arg(step)
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    let output = match tokio::time::timeout(BUILD_TIMEOUT, command.output()).await {
        Err(_) => return Err(format!("cargo {step} 超时（{}s）", BUILD_TIMEOUT.as_secs())),
        Ok(Err(error)) => return Err(format!("cargo {step} 起不来：{error}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: String = stderr.chars().skip(stderr.chars().count().saturating_sub(2000)).collect();
    Err(format!("cargo {step} 失败：\n{}", tail.trim()))
}

/// 项目内所有该提交的文件（相对仓根），跳过 target / .git / 隐藏项。
fn project_files(dir: &Path, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(dir: &Path, name: &str, prefix: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_string();
            if file.starts_with('.') || SKIP_DIRS.contains(&file.as_str()) {
                continue;
            }
            let rel = if prefix.is_empty() {
                file.clone()
            } else {
                format!("{prefix}/{file}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), name, &rel, out);
            } else if entry.path().is_file() {
                out.push(format!("{name}/{rel}"));
            }
        }
    }
    walk(dir, name, "", &mut out);
    out.sort();
    out
}

/// 根目录 `index.json`：一个触发器一行投影。旧版由 `index.mjs` 生成，现在同一进程里写。
fn write_index(repo: &Repo) -> std::result::Result<(), String> {
    let mut items = Vec::new();
    let entries = std::fs::read_dir(repo.root()).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if let Some(manifest) = read_manifest(&path) {
            items.push(json!({
                "name": name,
                "version": manifest.get("version"),
                "summary": manifest.get("summary"),
                "category": manifest.get("category"),
                "binary": manifest.get("binary"),
                "params": manifest.get("params"),
            }));
        }
    }
    items.sort_by(|a, b| a.get("name").and_then(Value::as_str).cmp(&b.get("name").and_then(Value::as_str)));
    let text = serde_json::to_string_pretty(&json!({ "triggers": items })).map_err(|e| e.to_string())?;
    std::fs::write(repo.root().join("index.json"), text + "\n").map_err(|e| e.to_string())
}

// ── 归档三代 ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ArchiveBody {
    name: String,
}

async fn archive(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ArchiveBody>,
) -> Result<Json<Value>> {
    lifecycle(state, body, |repo, name| repo.archive(name, None)).await
}

async fn restore(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ArchiveBody>,
) -> Result<Json<Value>> {
    lifecycle(state, body, |repo, name| repo.restore(name, None)).await
}

async fn purge(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ArchiveBody>,
) -> Result<Json<Value>> {
    lifecycle(state, body, |repo, name| repo.purge(name, None)).await
}

async fn lifecycle(
    state: Arc<AppState>,
    body: ArchiveBody,
    action: impl FnOnce(&Repo, &str) -> std::result::Result<nous_repo::LifecycleResult, RepoError>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    check_name(&body.name)?;
    let result = action(&repo, &body.name).map_err(repo_error)?;
    let push_error = match result.pushed {
        Pushed::Yes => Value::Null,
        Pushed::Unknown => Value::String("没有远端，没推".into()),
        Pushed::No => Value::String("推送未成功：本地提交已保留（查 git 凭据 / 远端）".into()),
    };
    Ok(Json(json!({
        "pushed": pushed_json(result.pushed),
        "pushError": push_error,
        "commit": result.commit,
        "dropped": false,
    })))
}

// ── 类别 ───────────────────────────────────────────────────────────────

fn vocab_path(root: &Path) -> PathBuf {
    root.join("categories.json")
}

/// 词表两种历史写法都认：`["a","b"]` 或 `{"categories":["a","b"]}`。
fn read_vocab(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(vocab_path(root)) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let array = value
        .as_array()
        .cloned()
        .or_else(|| value.get("categories").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut out: Vec<String> = array
        .into_iter()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 词表 = `categories.json` ∪ 所有 `nous.json` 的 category ∪ 它们的每一级前缀。
///
/// 旧仓（j1900 的 `/opt/Nous-Trigger`）根本没有 `categories.json`，类别只写在每条
/// `nous.json` 里；只读文件会让类别树空掉。前缀祖先也要登记，否则 `日常/摘要` 挂不上 `日常`。
fn vocab_with_usage(root: &Path) -> Vec<String> {
    let mut set = BTreeSet::new();
    for category in read_vocab(root) {
        insert_with_parents(&mut set, &category);
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Some(manifest) = read_manifest(&path) else { continue };
            let Some(categories) = manifest.get("category").and_then(Value::as_array) else {
                continue;
            };
            for category in categories.iter().filter_map(Value::as_str) {
                insert_with_parents(&mut set, category);
            }
        }
    }
    set.into_iter().collect()
}

fn insert_with_parents(set: &mut BTreeSet<String>, category: &str) {
    let mut acc = String::new();
    for part in category.split('/').filter(|part| !part.is_empty()) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(part);
        set.insert(acc.clone());
    }
}

fn write_vocab(root: &Path, vocab: &[String]) -> Result<()> {
    let text = serde_json::to_string_pretty(vocab)
        .map_err(|e| ServerError::Internal(format!("词表序列化失败：{e}")))?;
    std::fs::write(vocab_path(root), text + "\n")
        .map_err(|e| ServerError::Internal(format!("写 categories.json 失败：{e}")))
}

/// 改所有 `nous.json` 的 `category` 数组：`rename` 带子类一起搬，`remove` 连子类一起摘。
fn rewrite_manifests(root: &Path, old: &str, new: Option<&str>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let manifest_path = path.join("nous.json");
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(categories) = value.get("category").and_then(Value::as_array).cloned() else {
            continue;
        };
        let mapped: Vec<Value> = categories
            .into_iter()
            .filter_map(|item| {
                let category = item.as_str()?;
                if category == old {
                    new.map(|value| Value::String(value.to_string()))
                } else if let Some(rest) = category.strip_prefix(&format!("{old}/")) {
                    new.map(|value| Value::String(format!("{value}/{rest}")))
                } else {
                    Some(item.clone())
                }
            })
            .collect();
        if let Some(object) = value.as_object_mut() {
            object.insert("category".into(), Value::Array(mapped));
        }
        let text = serde_json::to_string_pretty(&value)
            .map_err(|e| ServerError::Internal(format!("序列化 {name}/nous.json 失败：{e}")))?;
        std::fs::write(&manifest_path, text + "\n")
            .map_err(|e| ServerError::Internal(format!("写 {name}/nous.json 失败：{e}")))?;
    }
    Ok(())
}

async fn categories(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    Ok(Json(json!({ "categories": vocab_with_usage(repo.root()) })))
}

#[derive(Debug, Deserialize)]
struct VocabBody {
    op: String,
    name: String,
    #[serde(default)]
    to: Option<String>,
}

/// `POST /v1/trigger-projects/categories`：`add` / `rename` / `remove`。不提交（sync 才提交）。
async fn edit_vocab(
    State(state): State<Arc<AppState>>,
    Json(body): Json<VocabBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let root = repo.root();
    let mut vocab = vocab_with_usage(root);
    match body.op.as_str() {
        "add" => {
            if !vocab.iter().any(|c| c == &body.name) {
                vocab.push(body.name.clone());
            }
        }
        "rename" => {
            let to = body
                .to
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| ServerError::BadRequest("rename 缺 to".into()))?;
            check_category(to)?;
            for category in &mut vocab {
                if category == &body.name {
                    *category = to.to_string();
                } else if let Some(rest) = category.strip_prefix(&format!("{}/", body.name)) {
                    *category = format!("{to}/{rest}");
                }
            }
            rewrite_manifests(root, &body.name, Some(to))?;
        }
        "remove" => {
            vocab.retain(|category| {
                category != &body.name && !category.starts_with(&format!("{}/", body.name))
            });
            rewrite_manifests(root, &body.name, None)?;
        }
        other => return Err(ServerError::BadRequest(format!("不认识的 op：{other}"))),
    }
    vocab.sort();
    vocab.dedup();
    write_vocab(root, &vocab)?;
    Ok(Json(catalog_value(&repo)?))
}

fn check_category(category: &str) -> Result<()> {
    if category.trim().is_empty()
        || category.starts_with('/')
        || category.ends_with('/')
        || category.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(ServerError::BadRequest(format!("类别不合法：{category}")));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct SetCategoryBody {
    name: String,
    category: Vec<String>,
}

/// `POST /v1/trigger-projects/category`：给一个触发器挂/换类别（单值写进 `category` 数组）。
async fn set_category(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetCategoryBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let dir = project_dir(&repo, &body.name)?;
    for category in &body.category {
        check_category(category)?;
    }
    let manifest_path = dir.join("nous.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| ServerError::Internal(format!("读 nous.json 失败：{e}")))?;
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|e| ServerError::Internal(format!("nous.json 不是 JSON：{e}")))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("category".into(), json!(body.category));
    }
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| ServerError::Internal(format!("序列化 nous.json 失败：{e}")))?;
    std::fs::write(&manifest_path, text + "\n")
        .map_err(|e| ServerError::Internal(format!("写 nous.json 失败：{e}")))?;

    let mut vocab = vocab_with_usage(repo.root());
    for category in &body.category {
        if !vocab.iter().any(|c| c == category) {
            vocab.push(category.clone());
        }
    }
    vocab.sort();
    vocab.dedup();
    write_vocab(repo.root(), &vocab)?;
    Ok(Json(catalog_value(&repo)?))
}

fn catalog_value(repo: &Repo) -> Result<Value> {
    let value = list_value(repo)?;
    Ok(json!({
        "catalog": value.get("triggers").cloned().unwrap_or(json!([])),
        "categories": value.get("categories").cloned().unwrap_or(json!([])),
    }))
}

/// `POST /v1/trigger-projects/sync`：只提交词表 / 清单，不碰 `.rs`（代码要走 publish）。
async fn sync(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let mut paths = vec!["categories.json".to_string(), "index.json".to_string()];
    if let Ok(entries) = std::fs::read_dir(repo.root()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.join("nous.json").is_file() {
                paths.push(format!("{name}/nous.json"));
            }
        }
    }
    paths.retain(|path| repo.root().join(path).exists());
    let commit = repo
        .commit(&paths, "trigger: 同步词表与清单", None)
        .map_err(repo_error)?;
    let push_error = match commit.pushed {
        Pushed::Yes => Value::Null,
        Pushed::Unknown => Value::String("没有远端，没推".into()),
        Pushed::No => Value::String("推送未成功：本地提交已保留（查 git 凭据 / 远端）".into()),
    };
    Ok(Json(json!({
        "committed": true,
        "pushed": pushed_json(commit.pushed),
        "pushError": push_error,
        "head": short_head(commit.commit),
    })))
}

/// 把 epoch 秒转成 `YYYY-MM-DD`（归档行只需要日期，不引时间库）。
fn ymd(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
