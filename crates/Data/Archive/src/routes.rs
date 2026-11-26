//! 本插件自己的 3 条端点：`POST /v1/sessions/{id}/archive`、`GET /v1/archive`、
//! `POST /v1/archive/{id}/restore`。
//!
//! Magento 2 那种「模块自带 controller」：handler 住在插件里，插件不认识 `server` 的
//! `AppState`，只声明它需要什么（[`Host`]：三个归档命令），由宿主实现并注入。
//!
//! 搬家前后行为一致的几条：
//!
//! - **回包形状逐字不变**：`{gen, ids, rows}` / `{gens:[{gen, items:[…]}]}` / `{restored, gen, rows}`。
//!   `wire.md` 与 NousDesk 都不动。
//! - **状态码跟着语义走**：忙 / 有活后代 / 原件还在 = 409（「现在不行」），
//!   参数错 = 400（「这么写不行」），会话不在 = 404。**不在这里降级成 400**。
//! - **四态不在引擎推**：`archived` + `live` 两个字段照实给，客户端自己判。
//!
//! 时钟只在这一层取：命令层零时钟，`at` 由端点传入。

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use nous_durable::session::commands::{ArchiveResult, RestoreResult};
use nous_durable::storage::archive::Generation;
use nous_routes::Route;
use serde::Deserialize;
use serde_json::{json, Value};

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "archive";

/// 宿主能力：三条归档命令。**一个方法一个能力**，不多给。
///
/// 插件不碰 `Storage`、不碰 `Session`、不开连接：活库的写锁与补偿方向都在命令层，
/// 这一层只负责「把 HTTP 翻成一次调用，再把结果翻成 JSON」。
#[async_trait]
pub trait Host: Send + Sync {
    /// `POST /v1/sessions/{id}/archive`：闭包复印进 gen 文件 + 写墓碑。
    async fn archive_session(&self, session: i64, at: i64) -> Result<ArchiveResult, nous_routes::HttpRefusal>;

    /// `GET /v1/archive`：世代列表 + 墓碑 + live。
    async fn archive_generations(&self) -> Result<Vec<Generation>, nous_routes::HttpRefusal>;

    /// `POST /v1/archive/{id}/restore`：从 gen 文件打捞回活库并抹墓碑。
    async fn restore_session(
        &self,
        session: i64,
        gen: String,
        at: i64,
    ) -> Result<RestoreResult, nous_routes::HttpRefusal>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
#[async_trait]
impl<T: Host + ?Sized> Host for Arc<T> {
    async fn archive_session(&self, session: i64, at: i64) -> Result<ArchiveResult, nous_routes::HttpRefusal> {
        (**self).archive_session(session, at).await
    }

    async fn archive_generations(&self) -> Result<Vec<Generation>, nous_routes::HttpRefusal> {
        (**self).archive_generations().await
    }

    async fn restore_session(
        &self,
        session: i64,
        gen: String,
        at: i64,
    ) -> Result<RestoreResult, nous_routes::HttpRefusal> {
        (**self).restore_session(session, gen, at).await
    }
}

type EndpointResult = std::result::Result<Json<Value>, nous_routes::HttpRefusal>;

/// 路径里的 id 用字符串接再自己 parse —— 让 axum 的 `Path<i64>` 拒掉会吐一段纯文本，
/// 破坏 `{error}` 的形状。文案与搬家前逐字一致。
fn parse_session(raw: &str) -> std::result::Result<i64, nous_routes::HttpRefusal> {
    raw.parse::<i64>()
        .map_err(|_| nous_routes::HttpRefusal::bad_request(format!("session id is not a number: {raw:?}")))
}

/// epoch 毫秒。时间由端点注入，命令层零时钟。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// `POST /v1/sessions/{id}/archive`：闭包复印到 gen 文件 + 墓碑（#67）。
async fn archive<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path(id): Path<String>,
) -> EndpointResult {
    let id = parse_session(&id)?;
    let result = host.archive_session(id, now_ms()).await?;
    Ok(Json(json!({
        "gen": result.gen,
        "ids": result.ids,
        "rows": result.rows,
    })))
}

/// `GET /v1/archive`：世代列表 + 墓碑 + live（#67）。
async fn archive_index<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
) -> EndpointResult {
    let gens = host.archive_generations().await?;
    let items = gens
        .into_iter()
        .map(|generation| {
            let items = generation
                .items
                .into_iter()
                .map(|item| {
                    let parent = item
                        .parent
                        .map(|(id, at)| json!({ "id": id, "at": at }))
                        .unwrap_or(Value::Null);
                    let last = item
                        .last
                        .map(|(id, kind, text)| json!({ "id": id, "kind": kind, "text": text }))
                        .unwrap_or(Value::Null);
                    let archived = item
                        .archived
                        .map(|tomb| {
                            json!({ "at": tomb.at, "gen": tomb.gen, "purged": tomb.purged })
                        })
                        .unwrap_or(Value::Null);
                    json!({
                        "id": item.id,
                        "parent": parent,
                        "entries": item.entries,
                        "last": last,
                        "name": item.name,
                        "archived": archived,
                        "live": item.live,
                    })
                })
                .collect::<Vec<_>>();
            json!({ "gen": generation.gen, "items": items })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "gens": items })))
}

#[derive(Debug, Deserialize)]
struct RestoreBody {
    gen: String,
}

/// `POST /v1/archive/{id}/restore`：从 gen 文件打捞回活库并抹墓碑（#67）。
async fn restore<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> EndpointResult {
    let id = parse_session(&id)?;
    let body: RestoreBody = if body.is_empty() {
        return Err(nous_routes::HttpRefusal::bad_request("gen is required"));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| nous_routes::HttpRefusal::bad_request(format!("restore body: {error}")))?
    };
    let result = host.restore_session(id, body.gen, now_ms()).await?;
    Ok(Json(json!({
        "restored": result.restored,
        "gen": result.gen,
        "rows": result.rows,
    })))
}

/// 本插件的端点表。路径与搬家前逐字一致（`wire.md` 与 Desk 都不动）。
pub fn routes<H>() -> Vec<Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        Route::new("POST", "/v1/sessions/{id}/archive", MODULE, || {
            post(archive::<H>)
        }),
        Route::new("GET", "/v1/archive", MODULE, || get(archive_index::<H>)),
        Route::new("POST", "/v1/archive/{id}/restore", MODULE, || {
            post(restore::<H>)
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_durable::storage::archive::{ArchiveItem, ArchivedTombstone};

    /// 假宿主：三条命令都给固定结果，用来钉「回包形状」与「参数错怎么答」。
    #[derive(Clone)]
    struct FakeHost {
        generations: Vec<Generation>,
        refuse_archive: Option<nous_routes::HttpRefusal>,
    }

    impl FakeHost {
        fn empty() -> Self {
            Self { generations: Vec::new(), refuse_archive: None }
        }
    }

    #[async_trait]
    impl Host for FakeHost {
        async fn archive_session(&self, session: i64, _at: i64) -> Result<ArchiveResult, nous_routes::HttpRefusal> {
            if let Some(refusal) = self.refuse_archive.clone() {
                return Err(refusal);
            }
            Ok(ArchiveResult { gen: "20261125".to_string(), ids: vec![session], rows: 3 })
        }

        async fn archive_generations(&self) -> Result<Vec<Generation>, nous_routes::HttpRefusal> {
            Ok(self.generations.clone())
        }

        async fn restore_session(
            &self,
            session: i64,
            gen: String,
            _at: i64,
        ) -> Result<RestoreResult, nous_routes::HttpRefusal> {
            Ok(RestoreResult { restored: vec![session], gen, rows: 5 })
        }
    }

    /// 注册表里的形状：3 条、路径与模块名都对得上（`:id` 写法由 `nous-routes` 折）。
    #[test]
    fn routes_declare_the_three_archive_endpoints_under_this_module() {
        let declared: Vec<String> = routes::<FakeHost>()
            .iter()
            .map(|route| format!("{} [{}]", route.declared(), route.module()))
            .collect();
        assert_eq!(
            declared,
            vec![
                "POST /v1/sessions/:id/archive [archive]",
                "GET /v1/archive [archive]",
                "POST /v1/archive/:id/restore [archive]",
            ]
        );
    }

    /// id 不是数字 → 400 且把收到的值回显在 `{error}` 里（不吞成 404）。
    #[test]
    fn a_non_numeric_id_is_a_400_with_the_raw_value() {
        let (status, body) = parse_session("abc").unwrap_err().split();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": "session id is not a number: \"abc\"" }));
    }

    /// 宿主拒了就是它给的状态码，插件不改写（409 不许被插件降级成 400）。
    #[tokio::test]
    async fn a_host_refusal_keeps_its_own_status() {
        let host = FakeHost {
            refuse_archive: Some(nous_routes::HttpRefusal::conflict("session 1 忙，不能归档")),
            ..FakeHost::empty()
        };
        let refusal = archive(State(host), Path("1".to_string())).await.unwrap_err();
        let (status, body) = refusal.split();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, json!({ "error": "session 1 忙，不能归档" }));
    }

    /// `GET /v1/archive` 的 item 形状：`parent` / `last` / `archived` 三个可空位照实给
    /// （`null` 是「没有」，不是「省略」），`live` 是引擎算好的。
    #[tokio::test]
    async fn archive_index_keeps_the_wire_shape() {
        let host = FakeHost {
            generations: vec![Generation {
                gen: "20261125".to_string(),
                items: vec![ArchiveItem {
                    id: 7,
                    parent: Some((6, 100)),
                    entries: 2,
                    last: Some((8, "pi.user".to_string(), "最后一条".to_string())),
                    name: "归档测试".to_string(),
                    archived: Some(ArchivedTombstone {
                        at: 200,
                        gen: "20261125".to_string(),
                        purged: false,
                    }),
                    live: false,
                }],
            }],
            ..FakeHost::empty()
        };
        let body = archive_index(State(host)).await.unwrap().0;
        assert_eq!(
            body,
            json!({
                "gens": [{
                    "gen": "20261125",
                    "items": [{
                        "id": 7,
                        "parent": { "id": 6, "at": 100 },
                        "entries": 2,
                        "last": { "id": 8, "kind": "pi.user", "text": "最后一条" },
                        "name": "归档测试",
                        "archived": { "at": 200, "gen": "20261125", "purged": false },
                        "live": false,
                    }],
                }],
            })
        );
    }

    /// restore 的 body 两条 400：空 body 与非法 JSON。文案与搬家前逐字一致。
    #[tokio::test]
    async fn restore_rejects_a_missing_or_broken_body_with_a_400() {
        let host = FakeHost::empty();
        let empty = restore(
            State(host.clone()),
            Path("1".to_string()),
            axum::body::Bytes::new(),
        )
        .await
        .unwrap_err();
        let (status, body) = empty.split();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": "gen is required" }));

        let broken = restore(
            State(host),
            Path("1".to_string()),
            axum::body::Bytes::from_static(b"{not json"),
        )
        .await
        .unwrap_err();
        let (status, body) = broken.split();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"].as_str().unwrap().starts_with("restore body: "),
            "{body}"
        );
    }

    /// 归档回包只有三个字段，`ids` 是数组（客户端不读，但形状不许漂）。
    #[tokio::test]
    async fn archive_answers_gen_ids_and_rows() {
        let body = archive(State(FakeHost::empty()), Path("9".to_string())).await.unwrap().0;
        assert_eq!(body, json!({ "gen": "20261125", "ids": [9], "rows": 3 }));
    }
}
