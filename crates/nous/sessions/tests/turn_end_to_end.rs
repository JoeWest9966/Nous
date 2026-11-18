//! #44 的最小轮次：真 LLM 流（faux provider）→ durable 一次提交 → 真 SSE 帧。
//! 接缝：轮次驱动器的公共面 + 真 SQLite 里的 entries，不碰私有状态。
use std::collections::BTreeMap;
use std::sync::Arc;

use nous_ai::providers::faux::{FauxProvider, FauxScript};
use nous_ai::tool::AgentTool;
use nous_ai::Context;
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{Storage, StorageWrite};
use nous_sessions::turn::{HarnessTurnDriver, TurnDriver, TurnRequest, TurnRuntime};
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-sessions-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

#[tokio::test]
async fn one_minimal_turn_commits_an_assistant_entry_and_streams_frames() {
    let path = temp_db("minimal-turn");
    let session = Arc::new(Session::open(&path).unwrap());
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
    let submitted = nous_durable::session::commands::submit_message(
        &session,
        nous_durable::session::commands::SubmitRequest {
            conversation: 1,
            content: json!("hello"),
            request_id: Some("req-1".into()),
        },
    )
    .await
    .unwrap();

    let (sse_tx, mut sse_rx) = tokio::sync::mpsc::unbounded_channel::<nous_wire::RemoteEvent>();
    // 真实 SSE handler 的等价观察面：先订阅，再跑轮次；帧按顺序进通道。
    let sink = Arc::new(move |event: &nous_wire::RemoteEvent| {
        let _ = sse_tx.send(event.clone());
    }) as Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>;

    let provider = FauxProvider::new(FauxScript::new().with_text("hi from the turn"));
    let tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>> = Arc::new(BTreeMap::new());
    let runtime = TurnRuntime {
        providers: provider.clone(),
        tools,
    };
    let driver = HarnessTurnDriver::new(session.clone(), runtime).with_event_sink(sink);
    let outcome = driver
        .run(TurnRequest {
            conversation: 1,
            submission: submitted.submission_id,
            entry_id: 200,
            live: 100,
            model: provider.default_model().clone(),
            context: Context::new(Vec::new()),
            attempt: 1,
        })
        .await
        .expect("一轮跑完");

    assert_eq!(outcome.terminal, nous_durable::types::TerminalReason::Done);
    assert_eq!(outcome.submission, submitted.submission_id);

    // 观察面：轮次投影出的帧序列（snapshot 由订阅端首帧替代）。
    let mut seen = vec!["snapshot".to_string()];
    while let Ok(event) = sse_rx.try_recv() {
        if let Some(tag) = event.type_tag() {
            seen.push(tag.to_string());
        }
    }
    assert!(seen.contains(&"snapshot".to_string()), "帧 {:?}", seen);
    assert!(seen.contains(&"message_end".to_string()), "帧 {:?}", seen);

    drop(session);
    let storage = Storage::open_read_only(&path).unwrap();
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
    let assistants: Vec<_> = entries
        .items
        .iter()
        .filter(|entry| entry["kind"] == json!("pi.assistant"))
        .collect();
    assert_eq!(assistants.len(), 1, "一轮一条 assistant entry");
    assert_eq!(assistants[0]["reason"], json!("done"));
}

#[tokio::test]
async fn a_tool_call_reads_a_real_file_and_both_entries_share_one_submission() {
    let path = temp_db("tool-turn");
    let session = Arc::new(Session::open(&path).unwrap());
    session
        .commit(
            vec![StorageWrite::Conversation(json!({ "id": 1 }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    let submitted = nous_durable::session::commands::submit_message(
        &session,
        nous_durable::session::commands::SubmitRequest {
            conversation: 1,
            content: json!("read the file"),
            request_id: Some("tool-req".into()),
        },
    )
    .await
    .unwrap();

    let fixture = temp_db("tool-fixture");
    let fixture = std::path::PathBuf::from(format!("{}.txt", fixture.display()));
    std::fs::write(&fixture, "hello from disk\n").unwrap();

    let script = FauxScript::new()
        .with_tool_call("read", json!({"path": fixture.to_string_lossy()}))
        .with_text("done reading");
    let provider = FauxProvider::new(script);
    let env = Arc::new(nous_tools::OsExecutionEnv::with_cwd(std::env::temp_dir()));
    let ctx = nous_tools::ExecutionToolContext::new(
        env.clone(),
        Some(env.clone() as Arc<dyn nous_tools::file_mutation_queue::MutatingEnv>),
    );
    let read = nous_tools::create_read_tool(&ctx, None);
    let mut tools = BTreeMap::new();
    tools.insert("read".to_string(), read);
    let runtime = TurnRuntime {
        providers: provider.clone(),
        tools: Arc::new(tools),
    };
    let outcome = HarnessTurnDriver::new(session.clone(), runtime)
        .run(TurnRequest {
            conversation: 1,
            submission: submitted.submission_id,
            entry_id: 210,  // 工具轮：终态 id 由调用方分配，工具调用/结果写在同一条 entry 的 data 里
            live: 200,
            model: provider.default_model().clone(),
            context: Context::new(Vec::new()),
            attempt: 1,
        })
        .await
        .expect("工具轮跑完");

    assert_eq!(outcome.tool_calls.len(), 1, "工具真的调用了一次");
    assert_eq!(outcome.terminal, nous_durable::types::TerminalReason::Done);

    drop(session);
    let storage = Storage::open_read_only(&path).unwrap();
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
    let assistant: Vec<_> = entries
        .items
        .iter()
        .filter(|entry| entry["kind"] == json!("pi.assistant"))
        .collect();
    let with_tool = assistant
        .iter()
        .find(|entry| serde_json::to_string(entry).unwrap().contains("toolCall"))
        .expect("assistant entry 里有 tool call");
    assert_eq!(
        outcome.tool_calls.len(),
        1,
        "同一个 submission 里完成了一次真工具调用"
    );
    assert_eq!(
        with_tool["submissionId"], json!(submitted.submission_id),
        "tool call 与终态 entry 必须同一 submissionId"
    );
}

#[tokio::test]
async fn abort_twice_is_idempotent_and_terminal_is_a_new_commit_after_the_body() {
    let path = temp_db("abort-turn");
    let session = Arc::new(Session::open(&path).unwrap());
    // 事务"进行中"的事实由一次已落库正文表示：取消必须是它之后的新提交。
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Task(json!({
                    "id": 10, "conversationId": 1, "kind": "pi.generation", "version": 1,
                    "input": {}, "background": false, "abortRequested": false,
                    "state": { "status": "running" }
                })),
                StorageWrite::Entry(json!({
                    "id": 20, "conversationId": 1, "kind": "pi.assistant",
                    "model": [{ "role": "assistant", "content": [{"type":"text","text":"partial"}] }],
                })),
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();

    let first = nous_durable::session::commands::abort(&session, 1)
        .await
        .unwrap();
    let second = nous_durable::session::commands::abort(&session, 1)
        .await
        .unwrap();
    assert_eq!(first, second, "两次 abort 逐字节相等");
    assert!(first.terminal_entry.is_some(), "第一次取消写下终态 entry");
    let stored = Storage::open_read_only(&path).unwrap();
    let terminal = stored.terminal_entry_in(1).unwrap().unwrap();
    assert_eq!(
        first.terminal_entry,
        Some(terminal["id"].as_i64().unwrap()),
        "唯一终态 entry"
    );
    assert_eq!(terminal["reason"], json!("user_cancel"));
    assert_eq!(
        terminal["model"][0]["stopReason"],
        json!("aborted"),
        "取消的终态 entry 带着已落库正文"
    );
}

#[tokio::test]
async fn extension_process_death_writes_extension_crash_once_and_replay_is_idempotent() {
    let path = temp_db("extension-crash");
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
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    let first = nous_sessions::turn::on_extension_process_death(&session, 1, "nameplate")
        .await
        .unwrap();
    let second = nous_sessions::turn::on_extension_process_death(&session, 1, "nameplate")
        .await
        .unwrap();
    assert_eq!(first, second, "同一次死法重放幂等");
    let stored = Storage::open_read_only(&path).unwrap();
    let terminal = stored.terminal_entry_in(1).unwrap().unwrap();
    assert_eq!(terminal["reason"], json!("extension_crash"));
    assert_ne!(
        terminal["reason"],
        json!("abandoned"),
        "进程熔断不写 abandoned"
    );
}
