//! #80 判据：trigger-catalog 管理面的公共接缝（真 Router + 真 Store）。
//!
//! - 判据 1：POST 新建 → 列表出现；坏 code（大写 / 短于 3）→ 400 且库里没行。
//! - 判据 2：PUT 改 schedule → version +1；body 带 code → 400（写死：改名不生效也不许沉默）。
//! - 判据 3：archive → 列表不含但 fires 仍在；restore → 回来；purge → fires 一并删。
//! - 判据 4：坏 schedule → 400 且库里没有新行（发布闸）。
//!
//! 时间全部注入；不 spawn、不碰私库、不开第二条写通道。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::session::{ObserverRegistry, Session};
use nous_durable::Storage;
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use nous_store::Store as TriggerStore;
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "nous-80-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (dir.join("durable.db"), dir.join("triggers.db"))
}

struct Fixture {
    app: axum::Router,
    trigger_db: std::path::PathBuf,
}

fn fixture(tag: &str) -> Fixture {
    let (durable_db, trigger_db) = temp(tag);
    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(Storage::open(&durable_db).unwrap(), registry.clone()));
    let commands = DurableCommands::with_session(durable.clone(), registry.clone());
    let state = AppState::durable_with_commands(ImportedSource::open(&durable_db).unwrap(), commands);
    // 管理面不接 Session 也能用：store 从 NOUS_PLUGINS_DB 的邻居读，装配在 main.rs 里做。
    // 测试直接注入 trigger runtime 所需的 store 装配（见 common / with_triggers）。
    let store = TriggerStore::open(&trigger_db).unwrap();
    let lookup: nous_triggers::TemplateLookup = Arc::new(|_: &str| Some("hello".to_string()));
    let runtime = nous_triggers::TriggerRuntime::new(
        store, durable, lookup, None, None, durable_db,
    )
    .unwrap();
    let app = router(state.with_triggers(runtime));
    Fixture { app, trigger_db }
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn new_body(code: &str, cron: Option<&str>) -> Value {
    json!({
        "code": code,
        "name": "夜报",
        "spec": { "spec_version": 1, "schedule": cron, "on": null },
        "templateRef": { "kind": "preset", "ref": "nightly" },
        "firesAllowedBy": [{ "kind": "system", "id": "cron" }],
        "target": { "kind": "new" },
        "policy": "serial",
    })
}

fn store(fixture: &Fixture) -> TriggerStore {
    TriggerStore::open(&fixture.trigger_db).unwrap()
}

// —— 判据 1 -----------------------------------------------------------------

#[tokio::test]
async fn create_shows_up_in_list_and_rejects_bad_code_without_writing_a_row() {
    let fixture = fixture("create");
    let (status, body) = call(&fixture.app, "POST", "/v1/triggers", new_body("nightly_digest", Some("0 3 * * *"))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["code"], json!("nightly_digest"));
    assert_eq!(body["version"], json!(1));

    let (_, list) = call(&fixture.app, "GET", "/v1/triggers", Value::Null).await;
    let items = list["triggers"].as_array().unwrap();
    assert_eq!(items.len(), 1, "新建 → 列表里出现：{list}");
    assert_eq!(items[0]["code"], json!("nightly_digest"));

    for bad in ["Bad-Code", "ab"] {
        let (status, body) = call(&fixture.app, "POST", "/v1/triggers", new_body(bad, None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "非法 code {bad} 必须 400：{body}");
        assert!(body["error"].as_str().is_some(), "错误体 {body}");
    }
    // 三次失败 + 一次成功：库里只有那一行成功的。
    assert_eq!(store(&fixture).triggers().unwrap().len(), 1, "坏 code 不许入库");
}

// —— 判据 2 -----------------------------------------------------------------

#[tokio::test]
async fn put_changes_schedule_and_bumps_version_and_rejects_code_in_body() {
    let fixture = fixture("put");
    call(&fixture.app, "POST", "/v1/triggers", new_body("nightly_digest", Some("0 3 * * *"))).await;

    let (status, body) = call(
        &fixture.app,
        "PUT",
        "/v1/triggers/nightly_digest",
        json!({ "schedule": "*/15 * * * *", "version": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], json!(2), "改 schedule → version 涨 1");
    let spec = body["definition"]["spec"]["spec"].as_object().unwrap();
    assert_eq!(spec["schedule"], json!("*/15 * * * *"));

    // body 里想改名 → 400（写死：不是静默忽略）。
    let (status, body) = call(
        &fixture.app,
        "PUT",
        "/v1/triggers/nightly_digest",
        json!({ "code": "other_code", "schedule": "0 4 * * *", "version": 2 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap_or("").contains("code"),
        "错误要说清 code 不可改：{body}"
    );
    assert_eq!(store(&fixture).trigger("nightly_digest").unwrap().unwrap().version, 2);
    assert!(store(&fixture).trigger("other_code").unwrap().is_none(), "不许真改名");
}

// —— 判据 3 -----------------------------------------------------------------

#[tokio::test]
async fn archive_hides_from_list_but_keeps_fires_and_restore_brings_it_back() {
    let fixture = fixture("archive");
    call(&fixture.app, "POST", "/v1/triggers", new_body("nightly_digest", Some("* * * * *"))).await;
    // 一笔账（不点火；直接走 store 的记账 API，账是 archive 判据的素材）。
    {
        let store = store(&fixture);
        store.record_fire("nightly_digest", 7_000, None).unwrap();
    }

    let (status, body) = call(&fixture.app, "POST", "/v1/triggers/nightly_digest/archive", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = call(&fixture.app, "GET", "/v1/triggers", Value::Null).await;
    assert!(list["triggers"].as_array().unwrap().is_empty(), "归档后列表不含它：{list}");
    assert_eq!(store(&fixture).fires().unwrap().len(), 1, "归档后账仍在");

    let (status, body) = call(&fixture.app, "POST", "/v1/triggers/nightly_digest/restore", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = call(&fixture.app, "GET", "/v1/triggers", Value::Null).await;
    assert_eq!(list["triggers"].as_array().unwrap().len(), 1, "捞回后回来：{list}");

    // 先归档再 purge：账一并删，墓碑留着占 code。
    call(&fixture.app, "POST", "/v1/triggers/nightly_digest/archive", json!({})).await;
    let (status, body) = call(&fixture.app, "POST", "/v1/triggers/nightly_digest/purge", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fires"], json!(1), "purge 回包说清删了几笔账：{body}");
    let store = store(&fixture);
    assert!(store.fires().unwrap().is_empty(), "purge 把 fires 一并处理（真删）");
    let row = store.trigger("nightly_digest").unwrap().unwrap();
    assert!(row.deleted_at.is_some(), "触发器行留墓碑，code 永不复用");

    // 再 purge → 409。
    let (status, _) = call(&fixture.app, "POST", "/v1/triggers/nightly_digest/purge", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "已真删再 purge → 409");
}

#[tokio::test]
async fn purge_without_archive_is_rejected() {
    let fixture = fixture("purge-first");
    call(&fixture.app, "POST", "/v1/triggers", new_body("nightly_digest", None)).await;
    let (status, _) = call(&fixture.app, "POST", "/v1/triggers/nightly_digest/purge", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "必须先归档才能 purge");
    assert_eq!(store(&fixture).fires().unwrap().len(), 0);
}

// —— 判据 4 -----------------------------------------------------------------

#[tokio::test]
async fn bad_schedule_is_rejected_at_publish_gate_without_writing_a_row() {
    let fixture = fixture("bad-cron");
    let mut body = new_body("nightly_digest", Some("0 3 * *")); // 四段，坏
    body["name"] = json!("坏定义");
    let (status, body) = call(&fixture.app, "POST", "/v1/triggers", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "坏 schedule → 400：{body}");
    assert!(store(&fixture).trigger("nightly_digest").unwrap().is_none(), "坏定义不许入库");

    // on 形状也不许入库（发布闸同管 schedule 与 on）。
    let mut body = new_body("other_code", None);
    body["spec"]["on"] = json!("trigger:another"); // 编排：禁止
    let (status, _) = call(&fixture.app, "POST", "/v1/triggers", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "坏 on → 400");
    assert!(store(&fixture).trigger("other_code").unwrap().is_none(), "坏 on 不许入库");
}

// —— 类别词表 ---------------------------------------------------------------

#[tokio::test]
async fn category_vocabulary_and_per_trigger_category() {
    let fixture = fixture("category");
    call(&fixture.app, "POST", "/v1/triggers", new_body("nightly_digest", None)).await;

    let (status, body) = call(
        &fixture.app,
        "POST",
        "/v1/triggers/category",
        json!({ "code": "nightly_digest", "category": "运维/夜报" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["category"], json!("运维/夜报"));

    let (_, list) = call(&fixture.app, "GET", "/v1/triggers", Value::Null).await;
    assert_eq!(list["triggers"][0]["category"], json!("运维/夜报"), "列表带 category");

    let (status, cats) = call(&fixture.app, "GET", "/v1/triggers/categories", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{cats}");
    let cats = cats["categories"].as_array().unwrap();
    assert!(cats.iter().any(|c| c == "运维/夜报"), "挂用的类别自动进词表：{cats:?}");

    let (status, body) = call(
        &fixture.app,
        "POST",
        "/v1/triggers/category",
        json!({ "code": "nightly_digest", "category": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["category"].is_null(), "null 清除该行的挂值");
    let (_, cats) = call(&fixture.app, "GET", "/v1/triggers/categories", Value::Null).await;
    assert!(cats["categories"].as_array().unwrap().iter().any(|c| c == "运维/夜报"), "词表条目不因清除而消失");
}

// —— review 1：协议是哑的 —— 未知字段必须忽略，不许 400 -------------------

#[tokio::test]
async fn unknown_body_fields_are_ignored_instead_of_400() {
    let fixture = fixture("unknown-fields");

    // POST /v1/triggers：多一个未知字段仍 201，且库里只有这一条。
    let mut create = new_body("nightly_digest", Some("0 3 * * *"));
    create["engineFromTheFuture"] = json!({ "shape": ["new", "fields"] });
    let (status, body) = call(&fixture.app, "POST", "/v1/triggers", create).await;
    assert_eq!(status, StatusCode::CREATED, "未知字段不该让新建 400：{body}");
    assert_eq!(body["code"], json!("nightly_digest"));

    // PATCH：未知字段 + 合法的 schedule/version，必须 200 且真的改了 enabled。
    let mut patch = json!({ "enabled": false, "version": 1 });
    patch["engineFromTheFuture"] = json!("ignored");
    let (status, body) = call(&fixture.app, "PATCH", "/v1/triggers/nightly_digest", patch).await;
    assert_eq!(status, StatusCode::OK, "未知字段不该让 PATCH 400：{body}");
    assert_eq!(body["enabled"], json!(false));
    assert_eq!(body["version"], json!(2));

    // PUT：未知字段仍 200。code 是唯一例外：写死 400，不能混同于“未知字段”。
    let mut put = json!({ "schedule": "*/15 * * * *", "version": 2 });
    put["engineFromTheFuture"] = json!(42);
    let (status, body) = call(&fixture.app, "PUT", "/v1/triggers/nightly_digest", put).await;
    assert_eq!(status, StatusCode::OK, "未知字段不该让 PUT 400：{body}");
    assert_eq!(body["version"], json!(3));

    // category 的 body 也一样：多字段 200，挂值生效。
    let mut category = json!({ "code": "nightly_digest", "category": "夜报" });
    category["engineFromTheFuture"] = json!(true);
    let (status, body) = call(&fixture.app, "POST", "/v1/triggers/category", category).await;
    assert_eq!(status, StatusCode::OK, "未知字段不该让 category 400：{body}");
    assert_eq!(body["category"], json!("夜报"));
}
