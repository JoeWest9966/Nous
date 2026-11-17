//! #38：终态五值 + 取消入口收敛（spec ch5/ch7）。
//!
//! 接缝：`session::commands` 的公共 API，观测点是真 sqlite 里的 entry / task。
//! 这里不测 HTTP 状态码（那是 `nous-server` 的接缝），也不读实现内部的私有状态。

use nous_chord::context::with_cancel;
use nous_chord::sqlite::SqliteExecutor;
use nous_durable::session::commands::{
    abort, cancel, scan_orphans, CancelOutcome, CancelRequest, CommandError,
};
use nous_durable::session::{CommitOptions, Session};
use nous_durable::types::TerminalReason;
use nous_durable::{DocumentContent, Storage, StorageWrite};
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-cancel-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// 一条会话 + 一个跑着的生成任务。终态 entry 与 live 文档由测试按需造。
async fn seeded(tag: &str) -> (Session, std::path::PathBuf) {
    let path = temp_db(tag);
    let session = Session::open(&path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
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
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    (session, path)
}

fn request(reason: TerminalReason) -> CancelRequest {
    CancelRequest {
        conversation: 1,
        reason,
    }
}

async fn terminal_entries(session: &Session) -> Vec<(i64, String, Option<TerminalReason>)> {
    // 公开面：从 session 逐条读 entry（没有私有访问器）。
    let mut out = Vec::new();
    for id in 1..100 {
        if let Some((entry, _)) = session.get_entry(id).await.unwrap() {
            out.push((entry.id, entry.kind, entry.reason));
        }
    }
    out
}

#[tokio::test]
async fn abort_goes_through_the_one_cancel_channel_and_writes_a_terminal_entry() {
    let (session, path) = seeded("abort-cancel").await;
    let outcome: CancelOutcome = cancel(&session, request(TerminalReason::UserCancel))
        .await
        .expect("有活跃任务就能取消");
    assert_eq!(outcome.tasks, vec![10]);

    let entries = terminal_entries(&session).await;
    assert_eq!(
        entries.len(),
        1,
        "取消落一条 entry，不是一条新的 entry 类型"
    );
    assert_eq!(entries[0].1, "pi.assistant");
    assert_eq!(entries[0].2, Some(TerminalReason::UserCancel));

    session.close().await.unwrap();
    let storage = Storage::open_read_only(&path).unwrap();
    let task = storage.task(10).unwrap().unwrap();
    assert_eq!(
        task["abortRequested"],
        json!(true),
        "标记只在 cancel 这一处写"
    );
    assert_eq!(
        task["state"]["status"],
        json!("running"),
        "不解释任务状态机"
    );
}

#[tokio::test]
async fn abort_twice_returns_the_same_outcome_and_adds_one_entry() {
    let (session, path) = seeded("abort-twice").await;
    let first = abort(&session, 1).await.expect("第一次取消");
    let second = abort(&session, 1).await.expect("第二次取消不报错");
    assert_eq!(first, second, "两次响应字节相等（同一个 outcome）");

    let entries = terminal_entries(&session).await;
    assert_eq!(entries.len(), 1, "第二次不再写 entry");
    assert_eq!(entries[0].2, Some(TerminalReason::UserCancel));

    session.close().await.unwrap();
    let storage = Storage::open_read_only(&path).unwrap();
    assert!(storage.tasks(1).unwrap()[0]["abortRequested"]
        .as_bool()
        .unwrap());
}

#[tokio::test]
async fn first_reason_wins_and_later_calls_do_not_rewrite_it() {
    let (session, _) = seeded("first-wins").await;
    cancel(&session, request(TerminalReason::UserCancel))
        .await
        .expect("第一个归因");
    cancel(&session, request(TerminalReason::Timeout))
        .await
        .expect("第二个不报错");
    let entries = terminal_entries(&session).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].2,
        Some(TerminalReason::UserCancel),
        "第一个写下的归因赢"
    );
}

#[tokio::test]
async fn idle_session_abort_is_still_a_conflict() {
    let path = temp_db("idle");
    let session = Session::open(&path).unwrap();
    session
        .commit(
            vec![StorageWrite::Conversation(json!({ "id": 1 }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        abort(&session, 1).await,
        Err(CommandError::Conflict(_))
    ));
}

// ── 三个触发源共用一个 CancelHandle ───────────────────────────────────────────

#[test]
fn three_trigger_sources_share_one_cancel_implementation() {
    // 每个源都拿到同一个 handle 的 clone，cancel 实现在 chord 里只有一处。
    let (_, handle) = with_cancel(nous_chord::context::Context::new());
    let clone = handle.clone();
    let handles = vec![handle, clone];
    for h in &handles {
        h.cancel();
    }
    // clone 出来的 handle 是同一个 token，任意一处取消，全部都能观察到。
    assert!(handles.iter().all(|h| h.token().is_cancelled()));
}

// ── 启动扫孤儿 ────────────────────────────────────────────────────────────────

/// 造一份「有正文无终态」的 live 文档；reopen 时扫描应补一条 `abandoned`。
async fn seed_orphan(tag: &str) -> std::path::PathBuf {
    let path = temp_db(tag);
    {
        let session = Session::open(&path).unwrap();
        session
            .commit(
                vec![
                    StorageWrite::Conversation(json!({ "id": 1 })),
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
                    StorageWrite::DocumentCreate {
                        id: 100,
                        record: nous_durable::harness::generation::live_record(100, 1),
                        content: DocumentContent::Base {
                            version: 1,
                            value: json!({
                                "generation": {
                                    "attempt": 1,
                                    "message": {
                                        "role": "assistant",
                                        "content": [{ "type": "text", "text": "half" }],
                                        "stopReason": "pending"
                                    }
                                }
                            }),
                        },
                    },
                    // 正文 entry 没带终态：整机崩在终态提交前一刻。
                    StorageWrite::Entry(json!({
                        "id": 9,
                        "conversationId": 1,
                        "kind": "pi.assistant",
                        "model": [{ "role": "assistant", "content": [{ "type": "text", "text": "half" }] }],
                    })),
                ],
                CommitOptions::default(),
            )
            .await
            .unwrap();
        session.close().await.unwrap();
    }
    path
}

#[tokio::test]
async fn startup_scan_adds_exactly_one_abandoned_and_stays_idempotent() {
    let path = seed_orphan("orphan").await;

    for round in 0..3 {
        let session = Session::open(&path).unwrap();
        let count = scan_orphans(&session).await.expect("扫描成功");
        assert_eq!(count, if round == 0 { 1 } else { 0 }, "只有第一次补一条");
        session.close().await.unwrap();
    }

    let storage = Storage::open_read_only(&path).unwrap();
    let live = storage
        .materialize(100, nous_durable::DocumentPoint::Current)
        .unwrap()
        .unwrap();
    assert!(
        live.value.get("generation").is_none(),
        "扫完 generation 要清掉"
    );
    let entries = storage
        .scan_entries(
            &nous_durable::EntryQuery {
                conversation_id: 1,
                min_entry_id: None,
                max_entry_id: None,
                order: None,
            },
            100,
            None,
        )
        .unwrap()
        .unwrap();
    let abandoned: Vec<&Value> = entries
        .items
        .iter()
        .filter(|e| e.get("reason") == Some(&json!("abandoned")))
        .collect();
    assert_eq!(abandoned.len(), 1, "连重启三次只多一条 abandoned");
}

/// 内核超时 / 扩展进程死两个触发源：只造 `CancelRequest`，走同一个 `cancel`。
#[tokio::test]
async fn timeout_and_crash_sources_enter_through_the_same_channel() {
    for reason in [TerminalReason::Timeout, TerminalReason::ExtensionCrash] {
        let tag = format!("source-{reason:?}").to_lowercase();
        let (session, path) = seeded(&tag).await;
        let outcome = cancel(&session, request(reason)).await.expect("能取消");
        assert_eq!(outcome.terminal_entry > 0, true);
        let entries = terminal_entries(&session).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].2,
            Some(reason),
            "{reason:?} 的归因写进同一个字段"
        );
        assert_eq!(entries[0].1, "pi.assistant", "没有第二种 entry 类型");
        session.close().await.unwrap();
        let _ = std::fs::remove_file(&path);
    }
}

/// kill -9 与 abort 同时发生：库不坏，正文和终态不分裂。
#[tokio::test]
async fn cancel_keeps_sqlite_integrity_even_when_interleaved_with_a_committed_body() {
    let (session, path) = seeded("integrity").await;
    // 先落一段正文（模拟事务结束后的提交），再取消：终态是事务结束后的一条新提交。
    session
        .commit(
            vec![StorageWrite::Entry(json!({
                "id": 9,
                "conversationId": 1,
                "kind": "pi.assistant",
                "model": [{ "role": "assistant", "content": [{ "type": "text", "text": "partial" }] }],
            }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    cancel(&session, request(TerminalReason::UserCancel))
        .await
        .expect("能取消");
    session.close().await.unwrap();

    // 真 sqlite 上跑 PRAGMA integrity_check —— 不经任何抽象。
    let store = nous_durable::Storage::open_read_only(&path).unwrap();
    let rows = store
        .store()
        .db()
        .all("PRAGMA integrity_check", &[])
        .expect("integrity_check 能跑");
    assert_eq!(rows.len(), 1);
    assert!(
        format!("{:?}", rows[0]).contains("ok"),
        "integrity_check 必须绿：{:?}",
        rows[0]
    );
    let _ = std::fs::remove_file(&path);
}

/// 取消时把 `pi.live.generation.message` 里已落库的 partial 搬进终态 entry，并清 generation：
/// 重连看到的是「有结局的一段文本」，不是「有正文无终态」。
#[tokio::test]
async fn cancel_moves_the_committed_partial_into_the_terminal_entry() {
    let path = temp_db("cancel-live-partial");
    {
        let session = Session::open(&path).unwrap();
        session
            .commit(
                vec![
                    StorageWrite::Conversation(json!({ "id": 1 })),
                    StorageWrite::Task(json!({
                        "id": 10, "conversationId": 1, "kind": "pi.generation", "version": 1,
                        "input": {}, "background": false, "abortRequested": false,
                        "state": { "status": "running" }
                    })),
                    StorageWrite::DocumentCreate {
                        id: 100,
                        record: nous_durable::harness::generation::live_record(100, 1),
                        content: DocumentContent::Base {
                            version: 1,
                            value: json!({
                                "generation": {
                                    "attempt": 1,
                                    "message": {
                                        "role": "assistant",
                                        "content": [{ "type": "text", "text": "half" }],
                                        "stopReason": "pending"
                                    }
                                }
                            }),
                        },
                    },
                ],
                CommitOptions::default(),
            )
            .await
            .unwrap();
        let outcome = cancel(&session, request(TerminalReason::UserCancel))
            .await
            .expect("能取消");
        let (entry, _) = session
            .get_entry(outcome.terminal_entry)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.model.unwrap()[0]["content"][0]["text"], json!("half"),
            "终态 entry 要带已提交的 partial 正文");
        let live = session.snapshot(100).await.unwrap().unwrap();
        assert!(live.value.get("generation").is_none(), "取消后 generation 要清掉");
    }
    let _ = std::fs::remove_file(&path);
}
