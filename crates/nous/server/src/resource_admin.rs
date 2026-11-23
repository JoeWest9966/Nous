//! 资源仓管理面：`/v1/{skills,prompts,extensions}/{files,file,create,publish}`。
//!
//! 三个私库共用一套文件形状（技能 / 提示词 / 扩展）：列清单、读写单个文件、新建、发布。
//! 只做 HTTP ↔ 盘面翻译：文件读写 / etag 撞闸 / git 提交 / push 都在 `nous_repo`
//! （platform 层），这里不直接 spawn `git`，也不另建一套锁。
//!
//! 一件事只做一遍：publish 自己不拼 `git add`，先问 `Repo::pending` 拿 dirty 路径，
//! 再交给 `Repo::commit`（内容闸与 pathspec 防搭车都在那一层）。

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use nous_repo::{Repo, RepoError, WriteFile, MAX_FILE_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};

/// 一次列清单最多这么多文件（旧面板 spec §6.1 同一个数）：递归不设层数上限，
/// 只靠这个数刹车 —— 截到第 4 层等于「文件在盘上、面板里看不见」。
pub const MAX_FILES: usize = 200;
const MAX_NAME: usize = 64;
/// 仓库根下不是条目的保留名（撞上它们 = 把 index.json 当技能编辑）。
const RESERVED: &[&str] = &[
    "categories.json",
    "deleted-extensions.json",
    "index.json",
    "index.mjs",
    "nous.json",
    "SKILL.md",
    "PROMPT.md",
    "README.md",
    "settings.json",
    "sources.json",
    "AGENTS.md",
    "test",
    "node_modules",
];

/// 三个资源仓。`repo` 是 `AppState` 里的私库键（`repos::from_env` 的名字），
/// `segment` 是 URL 里那一段 —— 扩展的私库键是 `extend`，URL 却是 `extensions`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skills,
    Prompts,
    Extensions,
}

impl Kind {
    pub fn segment(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Extensions => "extensions",
        }
    }

    fn repo_key(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Extensions => "extend",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Skills => "技能",
            Self::Prompts => "模板",
            Self::Extensions => "扩展",
        }
    }

    /// 发布时的默认文案（与旧面板 `skills-api.js` / `prompts-api.js` 一致）。
    fn default_message(self) -> &'static str {
        match self {
            Self::Skills => "skill: 更新技能仓库",
            Self::Prompts => "prompt: 更新模板仓库",
            Self::Extensions => "extend: 更新扩展仓库",
        }
    }
}

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    let router = router
        .route(
            "/v1/skills/files",
            axum::routing::get(|s, q| files(Kind::Skills, s, q)),
        )
        .route(
            "/v1/skills/file",
            axum::routing::get(|s, q| file(Kind::Skills, s, q))
                .put(|s, b| save(Kind::Skills, s, b)),
        )
        .route(
            "/v1/skills/create",
            axum::routing::post(|s, b| create(Kind::Skills, s, b)),
        )
        .route(
            "/v1/skills/publish",
            axum::routing::post(|s, b| publish(Kind::Skills, s, b)),
        )
        .route(
            "/v1/prompts/files",
            axum::routing::get(|s, q| files(Kind::Prompts, s, q)),
        )
        .route(
            "/v1/prompts/file",
            axum::routing::get(|s, q| file(Kind::Prompts, s, q))
                .put(|s, b| save(Kind::Prompts, s, b)),
        )
        .route(
            "/v1/prompts/create",
            axum::routing::post(|s, b| create(Kind::Prompts, s, b)),
        )
        .route(
            "/v1/prompts/publish",
            axum::routing::post(|s, b| publish(Kind::Prompts, s, b)),
        )
        .route(
            "/v1/extensions/files",
            axum::routing::get(|s, q| files(Kind::Extensions, s, q)),
        )
        .route(
            "/v1/extensions/file",
            axum::routing::get(|s, q| file(Kind::Extensions, s, q))
                .put(|s, b| save(Kind::Extensions, s, b)),
        )
        // 扩展的 create 已经在 `ext_admin`（#84，带骨架与运行时），这里不再造第二条。
        .route(
            "/v1/extensions/publish",
            axum::routing::post(|s, b| publish(Kind::Extensions, s, b)),
        );
    router
}

#[derive(Debug, Deserialize)]
pub struct NameQuery {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SaveBody {
    pub name: String,
    pub path: String,
    pub text: String,
    /// 省掉 = 硬写（新建 / 覆盖）；给上 = If-Match，不符回 409。
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    /// 技能用它（落 `SKILL.md` 的 frontmatter）。
    #[serde(default)]
    pub description: String,
    /// 提示词用它（落 `nous.json`）。
    #[serde(default)]
    pub summary: String,
    /// 提示词可给正文；不给就落一个 `# <name>` 标题。
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub category: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PublishBody {
    #[serde(default)]
    pub message: Option<String>,
}

/// 资源私库没装配（`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_EXTEND` 指到的目录不存在）
/// 时诚实地 409，不退回空 200：编辑器和「没有文件」是两件事。
fn repo(state: &AppState, kind: Kind) -> Result<Arc<Repo>> {
    state.repo(kind.repo_key()).ok_or_else(|| {
        ServerError::Conflict(format!(
            "{}私库未装配（{} 指向的目录不存在？）",
            kind.label(),
            match kind {
                Kind::Skills => "NOUS_SKILLS",
                Kind::Prompts => "NOUS_PROMPTS",
                Kind::Extensions => "NOUS_EXTEND",
            }
        ))
    })
}

/// 旧面板 `checkName` 的同一套规矩：名字就是目录名、就是会话里的键。
pub fn check_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ServerError::BadRequest("名字不能为空".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(ServerError::BadRequest(format!(
            "名字太长（最多 {MAX_NAME} 字）：{name}"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or_default();
    let valid = first.is_ascii_lowercase() || first.is_ascii_digit();
    let rest = name
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !valid || !rest {
        return Err(ServerError::BadRequest(format!(
            "名字只认小写字母、数字、点、下划线、连字符，且以字母或数字开头：{name}"
        )));
    }
    if RESERVED.contains(&name) {
        return Err(ServerError::BadRequest(format!(
            "「{name}」是仓库自己的文件名，不能当条目名"
        )));
    }
    Ok(name.to_string())
}

/// 条目目录里的相对路径：不许绝对路径、不许 `..`、不许反斜杠 —— 串到别的条目里
/// 也算「逃出这一条」（`nous_repo::files::safe_path` 只保证不出仓根）。
pub fn check_path(raw: &str) -> Result<String> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(ServerError::BadRequest("path 不能为空".into()));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(ServerError::BadRequest(format!(
            "路径必须是条目目录里的相对路径：{path}"
        )));
    }
    if path.split('/').any(|part| part == ".." || part == ".") {
        return Err(ServerError::BadRequest(format!("路径里有 .. 或 .：{path}")));
    }
    Ok(path.to_string())
}

fn require_entry(repo: &Repo, kind: Kind, name: &str) -> Result<()> {
    if !repo.root().join(name).is_dir() {
        return Err(ServerError::NotFound(format!(
            "仓库里没有这个{}：{name}",
            kind.label()
        )));
    }
    Ok(())
}

pub(crate) fn repo_error(error: RepoError) -> ServerError {
    match error {
        RepoError::BadRequest(m) | RepoError::InvalidPath(m) => ServerError::BadRequest(m),
        RepoError::Validation { field, problem } => ServerError::Validation { field, problem },
        RepoError::NotFound(m) => ServerError::NotFound(m),
        // 409 的 `{error}` 只留一句人话：Desk 原样显示它，JSON 塞进字符串里没人看得懂。
        RepoError::Conflict { message, .. } => {
            ServerError::Conflict(format!("{message} —— 先重新打开这个文件，再存一次"))
        }
        RepoError::Busy(m) => ServerError::Conflict(m),
        RepoError::Locked(m) => ServerError::Locked(m),
        RepoError::Io(m) => ServerError::Internal(m),
        RepoError::LockPoisoned => ServerError::Internal("私库写锁中毒".into()),
    }
}

/// `GET /v1/{kind}/files?name=`：递归清单，正文与附件一起给（不按扩展名筛）。
async fn files(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Query(query): Query<NameQuery>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let name = check_name(&query.name)?;
    require_entry(&repo, kind, &name)?;
    let mut out = Vec::new();
    collect(&repo.root().join(&name), "", &mut out);
    Ok(Json(json!({ "name": name, "files": out })))
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<Value>) {
    if out.len() >= MAX_FILES {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if kind.is_dir() {
            collect(&entry.path(), &child, out);
            continue;
        }
        // 符号链接一律不进列表：面板是编辑器，不是任意路径的读口。
        if !kind.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES as u64 {
            continue;
        }
        out.push(json!({ "path": child, "size": meta.len() }));
    }
}

/// `GET /v1/{kind}/file?name=&path=`：正文 + `etag`（写回时当 `base`）。
async fn file(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let name = check_name(&query.name)?;
    require_entry(&repo, kind, &name)?;
    let path = check_path(&query.path)?;
    let read = repo
        .read_file(&format!("{name}/{path}"))
        .map_err(repo_error)?;
    if read.content.contains(&0) {
        return Err(ServerError::BadRequest(format!(
            "{name}/{path} 是二进制，这一页不编辑它"
        )));
    }
    let text = String::from_utf8(read.content)
        .map_err(|_| ServerError::BadRequest(format!("{name}/{path} 不是 UTF-8 文本")))?;
    Ok(Json(
        json!({ "name": name, "path": path, "text": text, "etag": read.etag }),
    ))
}

/// `PUT /v1/{kind}/file`：只落盘、不提交。`base` 不符时 `nous_repo` 回 409。
async fn save(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let name = check_name(&body.name)?;
    require_entry(&repo, kind, &name)?;
    let path = check_path(&body.path)?;
    let created = body.base.is_none();
    let written = repo
        .write_file(WriteFile {
            path: &format!("{name}/{path}"),
            content: body.text.as_bytes(),
            base_etag: body.base.as_deref(),
        })
        .map_err(repo_error)?;
    Ok(Json(
        json!({ "name": name, "path": path, "etag": written.etag, "created": created }),
    ))
}

/// `POST /v1/{skills,prompts}/create`：落骨架两个文件，**不提交**。
/// 先写点开头的暂存目录再 rename（半截目录不许落到条目根里）。
/// 扩展不走这里（`ext_admin` 有带骨架与运行时的 create）。
async fn create(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Value>)> {
    debug_assert_ne!(kind, Kind::Extensions, "扩展的 create 在 ext_admin");
    let repo = repo(&state, kind)?;
    let name = check_name(&body.name)?;

    let skeleton = match kind {
        Kind::Skills => {
            let description = body.description.trim();
            if description.is_empty() {
                return Err(ServerError::BadRequest(
                    "description 不能空 —— 模型靠它判断该不该用这条技能".into(),
                ));
            }
            if description.contains(['\n', '\r']) {
                return Err(ServerError::BadRequest("description 只能是一行".into()));
            }
            Skeleton::Skill(description.to_string())
        }
        Kind::Prompts => {
            let summary = body.summary.trim();
            if summary.is_empty() {
                return Err(ServerError::BadRequest(
                    "summary 不能空 —— 人靠这一行决定选不选它".into(),
                ));
            }
            let text = if body.body.trim().is_empty() {
                format!("# {name}\n")
            } else {
                body.body.clone()
            };
            Skeleton::Prompt {
                summary: summary.to_string(),
                body: text,
            }
        }
        Kind::Extensions => {
            return Err(ServerError::BadRequest(
                "扩展的 create 走 /v1/extensions/create（带骨架与运行时）".into(),
            ))
        }
    };

    let landing = repo.root().join(&name);
    if landing.exists() {
        return Err(ServerError::Conflict(format!(
            "{name}/ 已经在了 —— 换个名字，或者直接去改它"
        )));
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let staging = repo.root().join(format!(".new-{name}-{stamp}"));
    std::fs::create_dir(&staging).map_err(|e| ServerError::Internal(e.to_string()))?;

    let landed = write_skeleton(&staging, &name, &skeleton, &body.category).and_then(|()| {
        std::fs::rename(&staging, &landing).map_err(|e| ServerError::Internal(e.to_string()))
    });
    if let Err(error) = landed {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "name": name, "path": name, "files": 2 })),
    ))
}

enum Skeleton {
    Skill(String),
    Prompt { summary: String, body: String },
}

fn write_skeleton(dir: &Path, name: &str, skeleton: &Skeleton, category: &[String]) -> Result<()> {
    let (primary, primary_text, summary) = match skeleton {
        Skeleton::Skill(description) => {
            // description 走 JSON 字符串字面量：引号 / 反斜杠都被转义，frontmatter 不会被
            // 一句话里的 `---` 或换行撕开（换行已经在上面挡掉了）。
            let quoted = serde_json::to_string(description).unwrap_or_else(|_| "\"\"".to_string());
            let skill = format!("---\nname: {name}\ndescription: {quoted}\n---\n\n# {name}\n");
            ("SKILL.md", skill, String::new())
        }
        Skeleton::Prompt { summary, body } => ("PROMPT.md", body.clone(), summary.clone()),
    };
    std::fs::write(dir.join(primary), primary_text)
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    let manifest = json!({ "name": name, "summary": summary, "category": category });
    let body = serde_json::to_string_pretty(&manifest)
        .map(|text| text + "\n")
        .unwrap_or_else(|_| "{}\n".to_string());
    std::fs::write(dir.join("nous.json"), body)
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(())
}

/// `POST /v1/{kind}/publish`：把私库里 dirty 的路径提交（并尝试 push）。
/// body 可以省；`message` 空着就用旧面板那句默认文案（随仓变）。
async fn publish(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PublishBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let units = repo.pending().map_err(repo_error)?;
    let mut paths: Vec<String> = Vec::new();
    let mut unpushed = false;
    for unit in &units {
        if unit.state == "unpushed" {
            unpushed = true;
            continue;
        }
        for path in &unit.paths {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
    }
    let message = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(kind.default_message());

    if !paths.is_empty() {
        let result = repo.commit(&paths, message, None).map_err(repo_error)?;
        return Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)));
    }
    if unpushed {
        // 只有「已提交没推」：再 commit 一次会撞 `nothing to commit`，这里只推。
        let pushed = repo.sync().map_err(repo_error)?;
        return Ok(Json(
            json!({ "commit": Value::Null, "pushed": pushed, "refs": [] }),
        ));
    }
    Err(ServerError::BadRequest("没有待发布的改动".into()))
}
