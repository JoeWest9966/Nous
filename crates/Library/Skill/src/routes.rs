//! 本插件自己的端点：`/v1/skills` 那 6 条。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 六条端点分两半，接缝也不同：
//!
//! - `GET /v1/skills` 是**只读投影**：盘上真相由 `library_skill::scan` 解析，
//!   这里只把 `PathBuf` 转字符串、`Option` 转 `null`。
//! - 另外五条是**编辑页的管理面**（`files` / `file` / `create` / `publish`）：
//!   盘面规矩（名字校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，
//!   这里只做 HTTP ↔ 那层的翻译。**不 spawn git、不另建锁** ——
//!   `nous-repo` 是唯一的 git 进程调用面。
//!
//! `?usage=1` 的反查（#85）走宿主能力 [`Host::usage`]：会话记录归 `nous-durable`，
//! 插件不自己开库。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use nous_repo::Refusal;
use nous_routes::HttpRefusal;
use serde::Deserialize;
use serde_json::{json, Value};

/// 模块全名（规则 12：`Skill` 是单段 PascalCase）：`/v1/capabilities` 的 `modules[].name`、
/// `/v1/_routes` 的 `module` 标签都用它 —— 面板靠这个标签把端点挂到模块名下。
pub const MODULE: &str = "Skill";

/// 私库在宿主仓库表里的键（`repos::from_env` 的名字）。
pub const REPO_KEY: &str = "skills";

/// 宿主能力：这个插件只认识三件事。
///
/// 每个方法都返回 `Option` —— `None` 是「宿主没装配」（目录不存在 / 没开库），
/// 端点据此诚实回 409，**不退回空 200**：编辑器和「没有文件」是两件事。
pub trait Host {
    /// 技能私库的仓库句柄。
    fn skills_repo(&self) -> Option<Arc<nous_repo::Repo>>;

    /// 技能私库的根目录（`NOUS_SKILLS` 解析结果）。只读投影用它 ——
    /// `library_skill::scan` 要的是路径，不是仓库句柄。
    fn skills_root(&self) -> PathBuf;

    /// `?usage=1`：谁在用这条技能（#85）。会话记录归 durable，插件不自己开库。
    fn skills_usage(&self) -> Option<Value>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn skills_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        (**self).skills_repo()
    }
    fn skills_root(&self) -> PathBuf {
        (**self).skills_root()
    }
    fn skills_usage(&self) -> Option<Value> {
        (**self).skills_usage()
    }
}



type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// `RepoError` → HTTP 回包：先由 `nous_repo::Refusal` 判语义，再逐字搬两格。
fn http(error: nous_repo::RepoError) -> HttpRefusal {
    let refusal = Refusal::from_repo(error);
    HttpRefusal::new(refusal.status, refusal.body())
}

fn http_conflict(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::conflict(message)
}

fn http_bad_request(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::bad_request(message)
}


/// 私库没装配时那句熟话（三个仓共用同一个形状，只换标签）。
fn repo<H: Host>(host: &H) -> std::result::Result<Arc<nous_repo::Repo>, HttpRefusal> {
    host.skills_repo().ok_or_else(|| {
        http_conflict("技能私库未装配（NOUS_SKILLS 指向的目录不存在？）")
    })
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
    #[serde(default)]
    pub category: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PublishBody {
    #[serde(default)]
    pub message: Option<String>,
}

/// `?usage=1`：反查模式开关。缺席 / 0 / false / 其它值都不是反查，
/// 不因为一个可选参数让整个端点 400（serde 的 `bool` 会把 `1` 判成非法）。
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub usage: Option<String>,
}

impl UsageQuery {
    fn requested(&self) -> bool {
        matches!(self.usage.as_deref(), Some("1" | "true"))
    }
}

/// `GET /v1/skills`：目录不存在就是空表，不伪造内容、不 404。
///
/// 盘上真相由 `library_skill` 解析（`SKILL.md` frontmatter / 附件 / 类别词表）。
/// 这里只做 wire 投影：目录路径转字符串、`Option` 转 `null`，`tracked` 恒 null
/// （#71 不碰 git，判定留给别的票）。`repo` 块只给 root + present（#63 形状）。
async fn skills<H: Host>(State(host): State<H>, Query(query): Query<UsageQuery>) -> EndpointResult {
    if query.requested() {
        return Ok(Json(
            host.skills_usage()
                .ok_or_else(|| http_conflict("会话库未装配，查不了谁在用"))?,
        ));
    }
    let root = host.skills_root();
    let present = root.is_dir();
    let scanned = crate::scan(&root);
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
    let shown = Value::String(root.to_string_lossy().into_owned());
    let mut status = if present {
        json!({ "root": shown, "present": true, "missing": Value::Null })
    } else {
        json!({
            "root": shown,
            "present": false,
            "missing": format!(
                "私库路径不存在：{}（设 {} 指到你的副本）",
                nous_env::SKILLS.default,
                nous_env::SKILLS.var
            ),
        })
    };
    status["categories"] = json!(scanned.categories);
    status["skills"] = json!(entries);
    status["repo"] = json!({
        "root": root.to_string_lossy().into_owned(),
        "present": present,
    });
    Ok(Json(status))
}

/// `GET /v1/skills/files?name=`：递归清单，正文与附件一起给（不按扩展名筛）。
async fn files<H: Host>(State(host): State<H>, Query(query): Query<NameQuery>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&query.name).map_err(http)?;
    let list = nous_repo::list_entry_files(&repo, &name).map_err(http)?;
    Ok(Json(json!({ "name": name, "files": list })))
}

/// `GET /v1/skills/file?name=&path=`：正文 + `etag`（写回时当 `base`）。
async fn file<H: Host>(State(host): State<H>, Query(query): Query<FileQuery>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&query.name).map_err(http)?;
    let path = nous_repo::check_path(&query.path).map_err(http)?;
    nous_repo::require_entry(&repo, &name, "技能").map_err(http)?;
    let (text, etag) = nous_repo::read_text(&repo, &format!("{name}/{path}"))
        .map_err(http)?;
    Ok(Json(json!({ "name": name, "path": path, "text": text, "etag": etag })))
}

/// `PUT /v1/skills/file`：只落盘、不提交。`base` 不符时 `nous_repo` 回 409。
async fn save<H: Host>(State(host): State<H>, Json(body): Json<SaveBody>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&body.name).map_err(http)?;
    let path = nous_repo::check_path(&body.path).map_err(http)?;
    nous_repo::require_entry(&repo, &name, "技能").map_err(http)?;
    let created = body.base.is_none();
    let etag = nous_repo::write_text(
        &repo,
        &format!("{name}/{path}"),
        &body.text,
        body.base.as_deref(),
    )
    .map_err(http)?;
    Ok(Json(json!({ "name": name, "path": path, "etag": etag, "created": created })))
}

/// `POST /v1/skills/create`：落骨架两个文件，**不提交**。
async fn create<H: Host>(
    State(host): State<H>,
    Json(body): Json<CreateBody>,
) -> std::result::Result<(StatusCode, Json<Value>), HttpRefusal> {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&body.name).map_err(http)?;
    let description = body.description.trim();
    if description.is_empty() {
        return Err(http_bad_request(
            "description 不能空 —— 模型靠它判断该不该用这条技能",
        ));
    }
    if description.contains(['\n', '\r']) {
        return Err(http_bad_request("description 只能是一行"));
    }
    let skeleton = nous_repo::Skeleton::skill(&name, description).with_category(body.category);
    nous_repo::create_entry(&repo, &name, &skeleton).map_err(http)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "name": name, "path": name, "files": 2 })),
    ))
}

/// `POST /v1/skills/publish`：把私库里 dirty 的路径提交（并尝试 push）。
/// body 可以省；`message` 空着就用旧面板那句默认文案。
async fn publish<H: Host>(
    State(host): State<H>,
    Json(body): Json<PublishBody>,
) -> EndpointResult {
    let repo = repo(&host)?;
    let message = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("skill: 更新技能仓库");
    let published = nous_repo::publish_entry(&repo, message).map_err(http)?;
    Ok(Json(serde_json::to_value(published).unwrap_or(Value::Null)))
}

/// 本插件的端点表：路径只写在这里（axum 的 `{id}` 形式），
/// `nous-routes` 自动折成注册表要的 `:id`。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/skills", MODULE, || get(skills::<H>)),
        nous_routes::Route::new("GET", "/v1/skills/files", MODULE, || get(files::<H>)),
        nous_routes::Route::new("GET", "/v1/skills/file", MODULE, || get(file::<H>)),
        nous_routes::Route::new("PUT", "/v1/skills/file", MODULE, || put(save::<H>)),
        nous_routes::Route::new("POST", "/v1/skills/create", MODULE, || post(create::<H>)),
        nous_routes::Route::new("POST", "/v1/skills/publish", MODULE, || post(publish::<H>)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared() -> Vec<String> {
        routes::<Arc<NullHost>>().iter().map(|route| route.declared()).collect()
    }

    struct NullHost;

    impl Host for NullHost {
        fn skills_repo(&self) -> Option<Arc<nous_repo::Repo>> {
            None
        }
        fn skills_root(&self) -> PathBuf {
            PathBuf::from("/nonexistent")
        }
        fn skills_usage(&self) -> Option<Value> {
            None
        }
    }

    /// 声明逐字：路径写错、漏一条、方法错，这一条立刻红。
    #[test]
    fn declares_all_six_endpoints_verbatim() {
        assert_eq!(
            declared(),
            vec![
                "GET /v1/skills",
                "GET /v1/skills/files",
                "GET /v1/skills/file",
                "PUT /v1/skills/file",
                "POST /v1/skills/create",
                "POST /v1/skills/publish",
            ]
        );
    }

    #[test]
    fn module_name_is_the_skill_full_name() {
        for route in routes::<Arc<NullHost>>() {
            assert_eq!(route.module(), "Skill");
        }
    }

    /// 未装配 = 409 且是那句熟话（不是空 200，也不是 404）。
    #[tokio::test]
    async fn unassembled_repo_is_a_conflict_not_an_empty_list() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let mut router = axum::Router::new();
        for route in &routes::<Arc<NullHost>>() {
            router = route.mount(router);
        }
        let router = router.with_state(Arc::new(NullHost));
        let request = Request::builder()
            .uri("/v1/skills/files?name=demo")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("NOUS_SKILLS"),
            "文案要点名环境变量：{body}"
        );
    }
}
