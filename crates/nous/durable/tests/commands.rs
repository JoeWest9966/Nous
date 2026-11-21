//! `session::commands` 的判据：幂等、中止、fork。三条都在真 sqlite 上跑。

use nous_chord::sqlite::SqliteExecutor;
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

// ── review 4：归档闭包 + 墓碑同一次提交 ────────────────────────────────────

fn count(path: &std::path::Path, table: &str) -> i64 {
    let storage = Storage::open_read_only(path).unwrap();
    let table = match table {
        "conversations" => "conversations",
        "entries" => "entries",
        other => panic!("未知表 {other}"),
    };
    let row = storage.store().db().get(&format!("SELECT count(*) AS n FROM {table}"), &[]).unwrap().unwrap();
    match row.get("n").unwrap() {
        nous_chord::sqlite::SqliteValue::Integer(n) => *n,
        other => panic!("count 不是整数：{other:?}"),
    }
}

fn archived_tombstone(path: &std::path::Path, id: i64) -> bool {
    let storage = Storage::open_read_only(path).unwrap();
    storage.store().db().get(
        "SELECT 1 AS hit FROM document_revisions WHERE content LIKE ?",
        &[nous_chord::sqlite::SqliteValue::Text(format!("%\"archived\":{{\"{id}\"%"))],
    ).map(|r| r.is_some()).unwrap_or(false)
}

#[tokio::test]
async fn archive_writes_the_tombstone_in_the_same_commit_as_the_generation() {
    let (session, path) = seeded("archive-atomic").await;
    session.commit(
        vec![StorageWrite::Entry(json!({
            "id": 2, "conversationId": 1, "kind": "pi.user",
            "model": [{ "role": "user", "content": "hello" }],
        }))],
        Default::default(),
    ).await.unwrap();
    let before_entries = count(&path, "entries");
    let before_tombstone = archived_tombstone(&path, 1);

    let result = nous_durable::session::commands::archive_conversation(&session, 1, 1_760_000_000_000, Some("20260101".into())).await.unwrap();
    assert_eq!(result.rows, 4, "闭包含会话 + entry + 两份 record_ids");
    assert_eq!(count(&path, "entries"), before_entries, "归档不删活库行");
    assert!(archived_tombstone(&path, 1), "墓碑与 generation 一次提交可见");
    assert!(!before_tombstone, "归档前不该有墓碑");
    // 真删原件后走 restore（wire.md：原件还在时 restore 必须 500，这里只测打捞路径）。
    nous_durable::session::commands::purge_session(&session, 1, Actor::system("test")).await.unwrap();
    assert_eq!(count(&path, "entries"), 0, "purge 撕掉活库行");

    // 恢复同理：一行提交把闭包写回 + 抹墓碑。
    let restored = nous_durable::session::commands::restore_conversation(&session, 1, "20260101".into(), 1_760_000_010_000).await.unwrap();
    assert_eq!(restored.rows, 4);
    assert_eq!(count(&path, "entries"), before_entries, "捞回的 entries 与归档前一致");
    assert!(!archived_tombstone(&path, 1), "恢复后墓碑抹掉");
}

/// review 4 负向哨兵：墓碑提交前 generation 写入失败 → 库里墓碑 / entries 不变。
/// 注入点只在 #[cfg(test)] 下由环境变量开；失败时 generation 文件也不许留下半截。
#[tokio::test]
async fn failed_generation_write_leaves_no_tombstone_and_no_orphan_file() {
    let dir = std::env::temp_dir().join(format!("nous-archive-fail-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite");
    let session = Session::open(&path).unwrap();
    session.commit(vec![StorageWrite::Conversation(json!({"id": 9}))], Default::default()).await.unwrap();
    let before_entries = count(&path, "conversations");
    assert!(!archived_tombstone(&path, 9));

    std::env::set_var("NOUS_TEST_ARCHIVE_FAIL_AFTER_WRITE", "20260102");
    let result = nous_durable::session::commands::archive_conversation(&session, 9, 1_760_000_000_000, Some("20260102".into())).await;
    std::env::remove_var("NOUS_TEST_ARCHIVE_FAIL_AFTER_WRITE");

    assert!(result.is_err(), "注入失败后 archive 必须报错：{result:?}");
    assert_eq!(count(&path, "conversations"), before_entries, "失败不许动活库行");
    assert!(!archived_tombstone(&path, 9), "失败不许留墓碑");
    let gen = path.parent().unwrap().join("archives/gen-20260102.db");
    assert!(!gen.exists(), "失败不许留半截 generation 文件");
    assert!(!gen.with_extension("db.tmp").exists(), "失败也不许留 .db.tmp 半截文件");
}

#[tokio::test]
async fn tombstone_failure_after_generation_write_removes_the_orphan() {
    let dir = std::env::temp_dir().join(format!("nous-archive-tomb-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite");
    let session = Session::open(&path).unwrap();
    session.commit(vec![StorageWrite::Conversation(json!({"id": 7}))], Default::default()).await.unwrap();

    std::env::set_var("NOUS_TEST_ARCHIVE_FAIL_BEFORE_TOMBSTONE", "20260103");
    let result = nous_durable::session::commands::archive_conversation(&session, 7, 1_760_000_000_000, Some("20260103".into())).await;
    std::env::remove_var("NOUS_TEST_ARCHIVE_FAIL_BEFORE_TOMBSTONE");
    assert!(result.is_err(), "墓碑前失败必须报错：{result:?}");
    assert!(!archived_tombstone(&path, 7));
    assert!(!path.parent().unwrap().join("archives/gen-20260103.db").exists(), "孤儿 generation 必须删掉");
}
