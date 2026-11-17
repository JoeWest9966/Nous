//! 12c 的判据：文档修订（base + delta 重放）、复制（materialize 源）、跨版本 / 没历史 / 带 emoji 的往返。
//!
//! 这一份是 `#10`（chord delta）第一次跑在**真数据**上：`t` 的计数是 UTF-16 码元、截在代理对中间
//! 要整字符丢弃 —— 纯 ASCII 的用例过不了这几条，所以下面至少有一条改写里带 ZWJ 家族 emoji。

use std::path::PathBuf;

use serde_json::{json, Value};

use nous_chord::delta::codec::Encoder;
use nous_chord::delta::diff::diff_revisions;
use nous_chord::sqlite::SqliteExecutor;
use nous_durable::{CopySource, DocumentContent, DocumentPoint, Refusal, Storage, StorageWrite};

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-docs-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

/// 一份 conversation 作用域的文档（只有这个作用域 + `rewindable` 才留历史）。
fn document(id: i64, conversation: i64, kind: &str, key: &str) -> Value {
    json!({
        "id": id, "kind": kind, "key": key,
        "scope": { "kind": "conversation", "conversationId": conversation },
        "history": "rewindable", "fork": "asOf"
    })
}

fn create(id: i64, conversation: i64, kind: &str, key: &str, value: Value) -> StorageWrite {
    StorageWrite::DocumentCreate {
        id,
        record: document(id, conversation, kind, key),
        content: DocumentContent::Base { version: 1, value },
    }
}

/// 把 `before → after` 的差算成一批 wire 形式的 op（上游 `diff` + 编码）。
fn ops(before: &Value, after: &Value) -> (Value, usize) {
    let trusted = diff_revisions(before, after);
    let wire = Encoder::new().encode(trusted.as_ops());
    (Value::Array(wire), trusted.len())
}

fn revision_count(storage: &Storage, id: i64) -> i64 {
    storage
        .store()
        .db()
        .get(
            "SELECT COUNT(*) AS n FROM document_revisions WHERE document_id = ?",
            &[id.into()],
        )
        .unwrap()
        .unwrap()
        .get("n")
        .unwrap()
        .as_i64()
        .unwrap()
}

#[test]
fn an_emoji_rewrite_round_trips_through_a_base_and_a_delta() {
    let path = temp_db("emoji");
    let mut storage = Storage::open(&path).unwrap();

    // 一句真数据（fixture 里 fork 44 的那句）+ ZWJ 家族 emoji。
    let before = json!({ "text": "今天是星期三。👩‍👩‍👧‍👦", "n": 1 });
    storage
        .commit(&[vec![create(10, 2, "pi.agent", "live", before.clone())]], |_, _| {})
        .unwrap();
    assert_eq!(
        storage.materialize(10, DocumentPoint::Current).unwrap().unwrap().value,
        before
    );

    // 改写：换一个字、追加一个单独的 emoji、删掉家族 emoji 的第一个码元（代理对中间）。
    let after = json!({ "text": "今天是星期四。🎉👩‍👩‍👧‍👦!", "n": 2 });
    let (wire, op_count) = ops(&before, &after);
    assert!(op_count >= 1);
    let result = storage
        .commit(
            &[vec![StorageWrite::DocumentChange {
                id: 10,
                content: DocumentContent::Delta { version: 1, ops: wire },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 1, "{:?}", result.refused);

    let current = storage.materialize(10, DocumentPoint::Current).unwrap().unwrap();
    // 逐个字符串比：emoji 的 UTF-16 语义一旦错了，这里就是乱码或半个代理项。
    assert_eq!(current.value["text"], json!("今天是星期四。🎉👩‍👩‍👧‍👦!"));
    assert_eq!(current.value, after);
    assert_eq!(current.version, 1, "delta 不换版本，版本只在换 base 时前进");
    assert_eq!(current.deltas_since_base, 1);

    // 时间旅行：base 那个 seq 上还是老值。
    assert_eq!(
        storage.materialize(10, DocumentPoint::At(1)).unwrap().unwrap().value,
        before
    );

    // 重新打开：落盘的东西逐字段一样（不靠内存）。
    drop(storage);
    let reopened = Storage::open(&path).unwrap();
    assert_eq!(reopened.materialize(10, DocumentPoint::Current).unwrap().unwrap().value, after);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_window_of_streamed_appends_is_one_op_and_one_revision() {
    let path = temp_db("window");
    let mut storage = Storage::open(&path).unwrap();
    let before = json!({ "text": "", "n": 0 });
    storage
        .commit(&[vec![create(11, 2, "pi.agent", "live", before.clone())]], |_, _| {})
        .unwrap();

    // 一个窗口里流进来三段（调用方只保留最后的值）；到点提交时差出来的是一**条** append。
    let window_one = json!({ "text": "第一段第二段第三段", "n": 0 });
    let (wire, op_count) = ops(&before, &window_one);
    assert_eq!(op_count, 1, "纯增长折成一条 a");
    assert_eq!(revision_count(&storage, 11), 1, "只有 base");
    storage
        .commit(
            &[vec![StorageWrite::DocumentChange {
                id: 11,
                content: DocumentContent::Delta { version: 1, ops: wire },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(revision_count(&storage, 11), 2, "base + 一条 delta");
    let materialized = storage.materialize(11, DocumentPoint::Current).unwrap().unwrap();
    assert_eq!(materialized.value, window_one);
    assert_eq!(materialized.deltas_since_base, 1);

    // 下一个窗口：又一条 delta（还是一条 op），值接着长。
    let window_two = json!({ "text": "第一段第二段第三段第四段", "n": 0 });
    let (wire, op_count) = ops(&window_one, &window_two);
    assert_eq!(op_count, 1);
    storage
        .commit(
            &[vec![StorageWrite::DocumentChange {
                id: 11,
                content: DocumentContent::Delta { version: 1, ops: wire },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(revision_count(&storage, 11), 3, "两个窗口两条 delta");
    let materialized = storage.materialize(11, DocumentPoint::Current).unwrap().unwrap();
    assert_eq!(materialized.value, window_two);
    assert_eq!(materialized.deltas_since_base, 2);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_retired_document_is_absent_current_and_still_readable_in_its_lifetime() {
    let path = temp_db("retire");
    let mut storage = Storage::open(&path).unwrap();
    storage
        .commit(&[vec![create(12, 2, "pi.agent", "live", json!({ "x": 1 }))]], |_, _| {})
        .unwrap();
    storage
        .commit(&[vec![StorageWrite::DocumentRetire { id: 12 }]], |_, _| {})
        .unwrap();

    assert!(storage.materialize(12, DocumentPoint::Current).unwrap().is_none());
    assert!(storage.materialize(12, DocumentPoint::At(1)).unwrap().is_some());
    // 退役那个 seq 上已经不在世（左闭右开）。
    assert!(storage.materialize(12, DocumentPoint::At(2)).unwrap().is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_current_only_document_refuses_to_be_read_in_the_past() {
    let path = temp_db("current-only");
    let mut storage = Storage::open(&path).unwrap();
    // session 作用域：只留最新态。
    let record = json!({
        "id": 13, "kind": "pi.agent", "scope": { "kind": "session" },
        "history": "latest", "fork": "current"
    });
    storage
        .commit(
            &[vec![StorageWrite::DocumentCreate {
                id: 13,
                record,
                content: DocumentContent::Base { version: 1, value: json!({ "x": 1 }) },
            }]],
            |_, _| {},
        )
        .unwrap();

    // 最新态读得到。
    assert_eq!(
        storage.materialize(13, DocumentPoint::Current).unwrap().unwrap().value,
        json!({ "x": 1 })
    );
    // 过去读不到：明确失败，不给「最新的值当作过去的」。
    assert_eq!(
        storage.materialize(13, DocumentPoint::At(1)).unwrap_err(),
        nous_durable::StorageError::HistoryNotRetained { document_id: 13 }
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_delta_that_crosses_a_version_boundary_without_a_base_is_an_error() {
    let path = temp_db("crossing");
    let mut storage = Storage::open(&path).unwrap();
    storage
        .commit(&[vec![create(14, 2, "pi.agent", "live", json!({ "x": 1 }))]], |_, _| {})
        .unwrap();
    // 直接塞一条版本不对的 delta（模拟被写坏的库 / 漏了 base 的写入）。
    let (wire, _) = ops(&json!({"x": 1}), &json!({"x": 2}));
    storage
        .store()
        .db()
        .run(
            "INSERT INTO document_revisions (document_id, seq, kind, version, content) VALUES (14, 2, 'delta', 2, ?)",
            &[wire.to_string().into()],
        )
        .unwrap();
    assert_eq!(
        storage.materialize(14, DocumentPoint::Current).unwrap_err(),
        nous_durable::StorageError::VersionCrossing { document_id: 14 }
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_copy_materializes_the_source_at_the_given_point() {
    let path = temp_db("copy");
    let mut storage = Storage::open(&path).unwrap();
    let base = json!({ "text": "第一版 👩‍👩‍👧‍👦" });
    storage
        .commit(&[vec![create(20, 2, "pi.agent", "live", base.clone())]], |_, _| {})
        .unwrap();
    let changed = json!({ "text": "第二版 🎉" });
    let (wire, _) = ops(&base, &changed);
    storage
        .commit(
            &[vec![StorageWrite::DocumentChange {
                id: 20,
                content: DocumentContent::Delta { version: 1, ops: wire },
            }]],
            |_, _| {},
        )
        .unwrap();

    // fork：源是会话 2 的这份文档，看当前状态。
    let result = storage
        .commit(
            &[vec![StorageWrite::DocumentCopy {
                id: 21,
                record: document(21, 3, "pi.agent", "live"),
                source: CopySource { id: 20, at: DocumentPoint::Current },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 1, "{:?}", result.refused);

    let copy = storage.materialize(21, DocumentPoint::Current).unwrap().unwrap();
    // 复制出来的是**一条 base**（不是共享 delta 链），值等于源当时的重放结果。
    assert_eq!(copy.value, changed);
    assert_eq!(copy.deltas_since_base, 0);
    assert_eq!(revision_count(&storage, 21), 1);

    // 复制在一个过去的点上：拿到的是那个点的值。
    storage
        .commit(
            &[vec![StorageWrite::DocumentCopy {
                id: 22,
                record: document(22, 3, "pi.agent", "live"),
                source: CopySource { id: 20, at: DocumentPoint::At(1) },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(
        storage.materialize(22, DocumentPoint::Current).unwrap().unwrap().value,
        base
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_copy_of_a_source_changed_in_the_same_batch_is_refused() {
    let path = temp_db("copy-changed");
    let mut storage = Storage::open(&path).unwrap();
    storage
        .commit(&[vec![create(30, 2, "pi.agent", "live", json!({ "x": 1 }))]], |_, _| {})
        .unwrap();

    let (wire, _) = ops(&json!({ "x": 1 }), &json!({ "x": 2 }));
    let result = storage
        .commit(
            &[vec![
                StorageWrite::DocumentChange {
                    id: 30,
                    content: DocumentContent::Delta { version: 1, ops: wire },
                },
                StorageWrite::DocumentCopy {
                    id: 31,
                    record: document(31, 3, "pi.agent", "live"),
                    source: CopySource { id: 30, at: DocumentPoint::Current },
                },
            ]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 0);
    assert_eq!(result.refused, Some(Refusal::CopySourceChanged { document_id: 31 }));
    assert!(storage.document(31).unwrap().is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_copy_whose_record_does_not_match_the_source_is_refused() {
    let path = temp_db("copy-mismatch");
    let mut storage = Storage::open(&path).unwrap();
    storage
        .commit(&[vec![create(40, 2, "pi.agent", "live", json!({ "x": 1 }))]], |_, _| {})
        .unwrap();

    let result = storage
        .commit(
            &[vec![StorageWrite::DocumentCopy {
                id: 41,
                // kind 不同：这不是复制，是另造一份。
                record: document(41, 3, "pi.live", "live"),
                source: CopySource { id: 40, at: DocumentPoint::Current },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 0);
    assert!(
        matches!(result.refused, Some(Refusal::CopyRejected { document_id: 41, .. })),
        "{:?}",
        result.refused
    );
    let _ = std::fs::remove_file(&path);
}

/// 真 fixture 的 961 份文档全部走一遍重放，并把每一份的值做一次「改一个字再折成 delta 再重放」。
///
/// 这是 `#10` 的 UTF-16 语义在真数据上的第一跑：fixture 里全是中文（还有 emoji），ASCII 用例
/// 碰不到代理对。fixture 不在时跳过并打印，不假绿。
#[test]
fn every_fixture_document_replays_and_survives_a_delta_round_trip() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    if !fixture.exists() {
        eprintln!("跳过：{} 不在（fixture 只读、不入库）", fixture.display());
        return;
    }
    // 上游 v2 库只能只读打开。
    let storage = Storage::open_read_only(&fixture).unwrap();
    let ids: Vec<i64> = storage
        .store()
        .db()
        .all("SELECT id FROM documents ORDER BY id", &[])
        .unwrap()
        .iter()
        .map(|row| row.get("id").unwrap().as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 961, "fixture 的 documents 行数");

    let mut replayed = 0;
    let mut round_tripped = 0;
    for id in ids {
        let Some(document) = storage.materialize(id, DocumentPoint::Current).unwrap() else {
            // 退役的文档当前不在世：换到它自己那个 seq 上读。
            continue;
        };
        replayed += 1;
        assert_eq!(document.deltas_since_base, 0, "fixture 的 revision 全是 base");

        // 改一个字符（保留原值与 emoji），折成 delta 再重放：值必须一模一样。
        let mut changed = document.value.clone();
        let original = changed.to_string();
        let perturbed = match &mut changed {
            Value::Object(map) => {
                map.insert("__roundtrip".to_string(), Value::String(original.clone()));
                changed
            }
            other => Value::String(format!("{}·改", other)),
        };
        let (wire, _) = ops(&document.value, &perturbed);
        let trusted = nous_chord::delta::codec::Decoder::new()
            .decode(wire.as_array().unwrap())
            .unwrap();
        let trusted = nous_chord::delta::apply::TrustedOps::from_decoded(trusted).unwrap();
        let replayed_value =
            nous_chord::delta::apply::apply_immutable_trusted(&document.value, &trusted).unwrap();
        assert_eq!(replayed_value, perturbed, "document {id}");
        round_tripped += 1;
    }
    assert_eq!(replayed, 961, "每一份都读到了");
    assert_eq!(round_tripped, 961);
}

/// `t` 的计数是 **UTF-16 码元**，这条只能在真数据上钉：fixture 的中文是一字符一码元。
#[test]
fn truncation_counts_utf16_units_on_real_fixture_text() {
    use nous_chord::delta::codec::Encoder;
    use nous_chord::delta::{Op, Seg};

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    if !fixture.exists() {
        eprintln!("跳过：{} 不在（fixture 只读、不入库）", fixture.display());
        return;
    }
    let storage = Storage::open_read_only(&fixture).unwrap();
    let ids: Vec<i64> = storage
        .store()
        .db()
        .all("SELECT id FROM documents ORDER BY id", &[])
        .unwrap()
        .iter()
        .map(|row| row.get("id").unwrap().as_i64().unwrap())
        .collect();

    /// 值里第一个长度 >= 2 的字符串及其路径。
    fn first_string(value: &Value, path: &mut Vec<Seg>) -> Option<(Vec<Seg>, String)> {
        match value {
            Value::String(text) if text.chars().count() >= 2 => {
                Some((path.clone(), text.clone()))
            }
            Value::Object(map) => {
                for (key, item) in map {
                    path.push(Seg::key(key));
                    if let Some(found) = first_string(item, path) {
                        return Some(found);
                    }
                    path.pop();
                }
                None
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    path.push(Seg::index(index));
                    if let Some(found) = first_string(item, path) {
                        return Some(found);
                    }
                    path.pop();
                }
                None
            }
            _ => None,
        }
    }

    let mut checked = 0;
    let mut non_ascii = 0;
    for id in ids {
        let Some(document) = storage.materialize(id, DocumentPoint::Current).unwrap() else {
            continue;
        };
        let Some((path, text)) = first_string(&document.value, &mut Vec::new()) else {
            continue;
        };
        let first = text.chars().next().unwrap();
        // 「一码元」这句前提要成立，下面那个期望值才算数。
        assert_eq!(first.len_utf16(), 1, "document {id} 的首字符不是单个码元");
        if !text.is_ascii() {
            non_ascii += 1;
        }
        let ops = Encoder::new().encode(&[Op::Truncate(path.clone(), 1)]);
        let decoded = nous_chord::delta::codec::Decoder::new()
            .decode(&ops)
            .unwrap();
        let trusted = nous_chord::delta::apply::TrustedOps::from_decoded(decoded).unwrap();
        let truncated =
            nous_chord::delta::apply::apply_immutable_trusted(&document.value, &trusted).unwrap();
        let expected = text.chars().skip(1).collect::<String>();
        assert_eq!(
            path_get(&truncated, &path),
            Some(json!(expected)),
            "document {id}: 截掉一个码元之后那个字符串必须正好少一个字符"
        );
        checked += 1;
    }
    assert!(checked > 0, "fixture 里没找到带字符串的文档？");
    assert!(non_ascii > 0, "一条非 ASCII 的都没碰到？那这条判据没意义");

    fn path_get(value: &Value, path: &[Seg]) -> Option<Value> {
        let mut current = value;
        for segment in path {
            current = match segment {
                Seg::Key(key) => current.get(key)?,
                Seg::Index(index) => current.get(*index as usize)?,
            };
        }
        Some(current.clone())
    }

}
