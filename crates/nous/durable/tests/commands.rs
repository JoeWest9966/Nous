//! `session::commands` 的判据：幂等、中止、fork。三条都在真 sqlite 上跑。

use nous_durable::actor::Actor;
use nous_durable::session::commands::{
    abort, fork, submit_message, CommandError, ForkRequest, SubmitRequest,
};
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{Storage, StorageWrite};
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-commands-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

async fn seeded(tag: &str) -> (Session, std::path::PathBuf) {
    let path = temp_db(tag);
    let session = Session::open(&path).unwrap();
    session
        .commit(
            vec![StorageWrite::Conversation(json!({ "id": 1 }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    (session, path)
}

fn request(content: Value, request_id: Option<&str>) -> SubmitRequest {
    SubmitRequest {
        conversation: 1,
        content,
        request_id: request_id.map(str::to_string),
    }
}

#[tokio::test]
async fn the_same_request_id_yields_one_submission_not_two() {
    let (session, path) = seeded("idempotent").await;

    let actor = Actor::human("joe");
    let first = submit_message(&session, request(json!("hello"), Some("req-1")), &actor)
        .await
        .expect("第一次受理");
    let second = submit_message(&session, request(json!("hello again"), Some("req-1")), &actor)
        .await
        .expect("第二次受理");
    assert_eq!(second.submission_id, first.submission_id, "同一个 requestId → 同一条 submission");
    assert_eq!(second.status, "queued");

    // 不同 requestId 才会再写一条。
    let third = submit_message(&session, request(json!("third"), Some("req-2")), &actor)
        .await
        .expect("第三次受理");
    assert_ne!(third.submission_id, first.submission_id);

    session.close().await.unwrap();
    let storage = Storage::open_read_only(&path).unwrap();
    let submissions = storage.submissions(1).unwrap();
    assert_eq!(submissions.len(), 2, "两次同 requestId 只该落一条；第三次才加一条");
    assert_eq!(
        submissions
            .iter()
            .filter(|record| record["requestId"] == json!("req-1"))
            .count(),
        1
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn content_that_is_neither_text_nor_parts_is_a_bad_request() {
    let (session, path) = seeded("bad-content").await;
    let error = submit_message(&session, request(json!(42), None), &Actor::system("test"))
        .await
        .expect_err("数字不是 content");
    assert!(matches!(error, CommandError::BadRequest(_)), "实测 {error:?}");
    // 不存在的会话是 404 那一类，不是 400。
    let error = submit_message(
        &session,
        SubmitRequest {
            conversation: 99,
            content: json!("hi"),
            request_id: None,
        },
        &Actor::system("test"),
    )
    .await
    .expect_err("会话不存在");
    assert!(matches!(error, CommandError::NotFound(_)), "实测 {error:?}");
    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn abort_marks_every_live_task_and_refuses_when_idle() {
    let (session, path) = seeded("abort").await;
    assert!(
        matches!(abort(&session, 1).await, Err(CommandError::Conflict(_))),
        "没有活跃任务时中止是冲突，不是成功"
    );

    session
        .commit(
            vec![
                StorageWrite::Task(json!({
                    "id": 10,
                    "conversationId": 1,
                    "kind": "pi.generation",
                    "version": 1,
                    "input": {},
                    "background": false,
                    "abortRequested": false,
                    "state": { "status": "running" },
                })),
                StorageWrite::Task(json!({
                    "id": 11,
                    "conversationId": 1,
                    "kind": "pi.generation",
                    "version": 1,
                    "input": {},
                    "background": false,
                    "abortRequested": false,
                    "state": { "status": "terminal" },
                })),
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();

    let outcome = abort(&session, 1).await.expect("有活跃任务");
    assert_eq!(outcome.tasks, vec![10], "终态任务不碰");

    session.close().await.unwrap();
    let storage = Storage::open_read_only(&path).unwrap();
    let tasks = storage.tasks(1).unwrap();
    let live = tasks.iter().find(|task| task["id"] == json!(10)).unwrap();
    assert_eq!(live["abortRequested"], json!(true));
    assert_eq!(live["state"]["status"], json!("running"), "状态机不解释、不伪造");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn fork_stores_the_parent_and_only_names_when_a_name_was_given() {
    let (session, path) = seeded("fork").await;
    session
        .commit(
            vec![StorageWrite::Entry(json!({
                "id": 5,
                "conversationId": 1,
                "kind": "pi.user",
                "model": [{ "role": "user", "content": "hi", "timestamp": 1 }],
            }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();

    let anonymous = fork(
        &session,
        ForkRequest {
            conversation: 1,
            at: None,
            name: None,
        },
    )
    .await
    .expect("空闲可以分叉");
    assert_eq!(anonymous.at, 5, "不给 entryId 就从最后一条分叉");

    let named = fork(
        &session,
        ForkRequest {
            conversation: 1,
            at: Some(5),
            name: Some("分支".to_string()),
        },
    )
    .await
    .expect("再分一条");
    assert_eq!(named.at, 5);

    session.close().await.unwrap();
    let storage = Storage::open_read_only(&path).unwrap();
    let record = storage.conversation(anonymous.id).unwrap().expect("新会话在");
    assert_eq!(record["parent"]["conversationId"], json!(1));
    assert_eq!(record["parent"]["at"], json!(5));

    // 名字住在 `qrd3wh.sessions` 文档里，会话树合并它。
    let store = nous_durable::storage::import::ImportedStore::read(&path).unwrap();
    let items = nous_durable::storage::sessions::render_sessions(&store).unwrap();
    let anonymous_item = items.iter().find(|item| item.id == anonymous.id).unwrap();
    assert_eq!(anonymous_item.name, "", "没给名字就是空串，不生成默认名");
    let named_item = items.iter().find(|item| item.id == named.id).unwrap();
    assert_eq!(named_item.name, "分支");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn fork_is_refused_while_the_session_is_busy() {
    let (session, path) = seeded("fork-busy").await;
    session
        .commit(
            vec![StorageWrite::Task(json!({
                "id": 20,
                "conversationId": 1,
                "kind": "pi.generation",
                "version": 1,
                "input": {},
                "background": false,
                "abortRequested": false,
                "state": { "status": "running" },
            }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        fork(
            &session,
            ForkRequest {
                conversation: 1,
                at: None,
                name: None,
            }
        )
        .await,
        Err(CommandError::Conflict(_))
    ));
    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}
