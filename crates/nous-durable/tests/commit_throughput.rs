//! `#14` 的前置判据：**immediate 提交顶不顶得掉 deferred**（用户 2026-11-17 定的门）。
//!
//! 背景：上游 `live.ts:236` 的 `PROGRESS_COMMIT = { kind: "deferred" }` —— 流式 partial 先攒着，
//! 攒到下一次真提交或 `persistWithinMs` 才落盘；而 `#13` 的 `Session::commit` 明确拒绝 deferred。
//! 若 harness 只做 immediate，按 16ms 合帧就是 ~60 次提交/秒，每次一个事务。
//!
//! 这条测试量的就是「1,000 次提交（模拟 16 秒流式输出）本机总耗时」：
//!
//! - **≤ 2s** → deferred 永久砍掉，(a) 关票不做，不留「以后再说」；
//! - **> 2s** → 那才是开 (a) 的证据，到那时 `#13` 的关闭语义一起改。
//!
//! 不放进 `verify.sh`：时间数字在机器上有噪声，进去只会变成假红。手工跑：
//!
//! ```text
//! cargo test --release -p nous-durable --test commit_throughput -- --ignored --nocapture
//! ```
//!
//! 每次提交的形状照真实 partial：往 `pi.live` 的 `generation.message.content[0].text` 追加一段。
//! 走 `Session::commit`（harness 与 `#22` 真正会用的入口），不是直接摸 `Storage` —— 后者省掉
//! 锁与发布，量出来的数字不代表线上。

use std::time::Instant;

use nous_chord::sqlite::SqliteExecutor;
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{DocumentContent, DocumentPoint, Storage, StorageWrite};
use serde_json::json;

const COMMITS: usize = 1_000;
/// 1,000 次提交 ≈ 16 秒的 16ms 合帧（62.5 次/秒）。
const BUDGET_MS: u128 = 2_000;
/// 每次追加的字节数，跟真实 token 流的分片同量级。
const CHUNK: usize = 32;

/// 基准库要落在**真文件系统**上：`std::env::temp_dir()` 在不少机器上是 tmpfs，那里 `fsync` 是
/// 空操作 —— 量出来的「快」是内存的快，不是 sqlite 的快。默认 `/var/tmp`（ext4），可用
/// `NOUS_BENCH_DIR` 指到别处。
fn bench_dir() -> std::path::PathBuf {
    let dir = std::env::var_os("NOUS_BENCH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"));
    if dir.is_dir() {
        dir
    } else {
        std::env::temp_dir()
    }
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = bench_dir().join(format!(
        "nous-bench-commit-{tag}-{}.sqlite",
        std::process::id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// 一条 partial 提交：只追加一段文本（`a` = 唯一不重写整串的 op）。
fn append(chunk: &str) -> StorageWrite {
    StorageWrite::DocumentChange {
        id: 3,
        content: DocumentContent::Delta {
            version: 1,
            ops: json!([["a", ["generation", "message", "content", 0, "text"], chunk]]),
        },
    }
}

/// 基准的前提条件也是判据：`journal_mode=WAL`、`synchronous=NORMAL`。
///
/// 这两条不是「顺手设的」，是 11,200 ms → 1,890 ms 那个差距的原因：WAL 下的 NORMAL 让提交
/// 不 fsync。设错了却照跑基准，量出来的是另一个系统的数字（`docs/bench-commit.md`）。
#[test]
fn the_writer_connection_runs_in_wal_with_normal_synchronous() {
    let path = temp_db("pragmas");
    let storage = Storage::open(&path).expect("打开");
    let db = storage.store().db();
    assert_eq!(pragma_of(db, "journal_mode"), "wal");
    assert_eq!(pragma_of(db, "synchronous"), "1", "NORMAL == 1");
}

/// 读**写连接自己**的 pragma（`synchronous` 是连接级设置，只读连接看到的是默认值）。
fn pragma_of(db: &nous_chord::sqlite::local::LocalDatabase, name: &str) -> String {
    let row = db
        .get(&format!("PRAGMA {name}"), &[])
        .expect("pragma 查询")
        .expect("pragma 有一行");
    let value = row.get(name).expect("pragma 有同名列");
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|n| n.to_string()))
        .expect("pragma 是标量")
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "benchmark: 手工跑，数字进 docs/bench-commit.md"]
async fn one_thousand_immediate_partial_commits_stay_within_the_budget() {
    let path = temp_db("partial");
    println!("… 基准库: {}", path.display());
    let session = Session::open(&path).expect("打开");

    // 造一份 pi.live 形状的文档：一次 base，之后 1,000 条 delta。
    let base = session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::DocumentCreate {
                    id: 3,
                    record: json!({
                        "id": 3,
                        "kind": "pi.live",
                        "scope": { "kind": "conversation", "conversationId": 2 },
                        "history": "latest",
                        "fork": "initial",
                    }),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({
                            "generation": {
                                "attempt": 1,
                                "message": { "content": [{ "type": "text", "text": "" }] },
                            },
                        }),
                    },
                },
            ],
            CommitOptions::default(),
        )
        .await
        .expect("base 提交");
    // `stored` 数的是文档修订条数（`#13` 起就如此），不是「写了几条」—— 会话行不算。
    assert_eq!(base.stored, 1, "base 被拒了：{:?}", base.refused);

    let started = Instant::now();
    for i in 0..COMMITS {
        let chunk = format!("chunk{i:05}{}", "x".repeat(CHUNK - 10));
        let outcome = session
            .commit(vec![append(&chunk)], CommitOptions::default())
            .await
            .expect("partial 提交");
        assert_eq!(outcome.stored, 1, "第 {i} 条被拒：{:?}", outcome.refused);
    }
    let elapsed = started.elapsed();

    // 确认库真的长了 —— 否则量的是一个空转的循环。
    let reader = Storage::open_read_only(&path).expect("独立只读连接");
    let stored = reader
        .materialize(3, DocumentPoint::Current)
        .expect("materialize")
        .expect("文档在");
    let text_len = stored.value["generation"]["message"]["content"][0]["text"]
        .as_str()
        .expect("text 是字符串")
        .len();
    assert_eq!(text_len, COMMITS * CHUNK, "每次都追加成功了才算数");

    let ms = elapsed.as_millis();
    println!("… 1,000 次 immediate partial 提交: {ms} ms（预算 {BUDGET_MS} ms，正文 {text_len} 字节）");
    // 前提条件在写连接上看（`synchronous` 是连接级设置）：另开一个写连接，走同一段打开代码。
    let writer = Storage::open(&path).expect("第二个写连接");
    println!(
        "… journal_mode = {}, synchronous = {}",
        pragma_of(writer.store().db(), "journal_mode"),
        pragma_of(writer.store().db(), "synchronous")
    );

    assert!(
        ms <= BUDGET_MS,
        "{ms} ms 超过 {BUDGET_MS} ms 预算 —— 这就是开 deferred（(a)）的证据。\
         别放宽这条测试：把数字和依据写进 docs/bench-commit.md，再动 #13 的关闭语义"
    );
}

/// 对照组：同样的 1,000 次提交，但每 100 次写一次 base（模拟 `checkpointWhen` 空闲时打新 base）。
///
/// 这条存在的唯一理由是量出上一条的超线性：delta 提交要把 `materialize(Current)` 读回来
/// 当 publication 的 `value`，那条路会重放整条修订链 —— 链越长每次越贵。所以同等工作量下，
/// 「链一直长」和「链被 base 切断」差一个数量级。两个数字一起才说明代价在哪。
#[tokio::test(flavor = "current_thread")]
#[ignore = "benchmark: 手工跑，和上一条对照，数字进 docs/bench-commit.md"]
async fn one_thousand_commits_with_a_checkpoint_every_hundred() {
    const CHECKPOINT_EVERY: usize = 100;
    let path = temp_db("checkpointed");
    println!("… 基准库: {}", path.display());
    let session = Session::open(&path).expect("打开");
    let mut version = 1i64;
    let mut text = String::new();

    let base = |version: i64, text: &str| StorageWrite::DocumentCreate {
        id: 3,
        record: json!({
            "id": 3,
            "kind": "pi.live",
            "scope": { "kind": "conversation", "conversationId": 2 },
            "history": "latest",
            "fork": "initial",
        }),
        content: DocumentContent::Base {
            version,
            value: json!({
                "generation": { "attempt": 1, "message": { "content": [{ "type": "text", "text": text }] } },
            }),
        },
    };

    session
        .commit(
            vec![StorageWrite::Conversation(json!({ "id": 2 })), base(version, &text)],
            CommitOptions::default(),
        )
        .await
        .expect("base 提交");

    let started = Instant::now();
    for i in 0..COMMITS {
        let chunk = format!("chunk{i:05}{}", "x".repeat(CHUNK - 10));
        if i > 0 && i % CHECKPOINT_EVERY == 0 {
            version += 1;
            session
                .commit(
                    vec![StorageWrite::DocumentChange {
                        id: 3,
                        content: DocumentContent::Base {
                            version,
                            value: json!({
                                "generation": { "attempt": 1, "message": { "content": [{ "type": "text", "text": text }] } },
                            }),
                        },
                    }],
                    CommitOptions::default(),
                )
                .await
                .expect("checkpoint");
        }
        session
            .commit(
                vec![StorageWrite::DocumentChange {
                    id: 3,
                    content: DocumentContent::Delta {
                        version,
                        ops: json!([["a", ["generation", "message", "content", 0, "text"], chunk]]),
                    },
                }],
                CommitOptions::default(),
            )
            .await
            .expect("partial");
        text.push_str(&chunk);
    }
    let ms = started.elapsed().as_millis();
    println!("… 1,000 次提交（每 100 次打一次 base）: {ms} ms / 正文 {} 字节", text.len());

    let reader = Storage::open_read_only(&path).expect("只读连接");
    let stored = reader
        .materialize(3, DocumentPoint::Current)
        .expect("materialize")
        .expect("文档在");
    assert_eq!(
        stored.value["generation"]["message"]["content"][0]["text"]
            .as_str()
            .expect("text"),
        text
    );
}
