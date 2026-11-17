//! 12a 的判据：**真 fixture 读得进、写得进新库、读回来逐字段不差**，且会话树是 `wire.md` 的形状。
//!
//! `fixtures/agent.sqlite` 是一份真实用户库（1.2 MB，永不入库、永不写回）。它不在时全部跳过并
//! 打印原因 —— 不假绿。

use std::path::{Path, PathBuf};

use nous_chord::sqlite::{SqliteExecutor, SqliteRow, SqliteValue as Cell};
use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::render_sessions;
use nous_durable::storage::{SqliteStore, WRITER};
use nous_durable::StorageError;

fn fixture_path() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    path.exists().then_some(path)
}

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    // SQLite 的 WAL 边车文件也要清掉，否则上一轮的库会被接上。
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

fn cell(row: &SqliteRow, column: &str) -> Cell {
    row.get(column).cloned().unwrap_or(Cell::Null)
}

/// 「哨兵与 NULL 同义」—— 写的一侧用哨兵，旧库用 NULL。判断逻辑在库里那份，这里只是给
/// 判据用；两边必须一致，所以直接调库里的函数。
fn same_cell(table: &str, column: &str, left: &Cell, right: &Cell) -> bool {
    use nous_durable::storage::import::sentinel_equivalent;
    if left == right {
        return true;
    }
    if column == "record" || column == "content" {
        // JSON 文本按值比（键序、空白都可能不同）。
        if let (Some(left), Some(right)) = (left.as_str(), right.as_str()) {
            return serde_json::from_str::<serde_json::Value>(left).ok()
                == serde_json::from_str::<serde_json::Value>(right).ok();
        }
    }
    sentinel_equivalent(table, column, left, right)
}

#[test]
fn the_fixture_reads_with_its_tables_and_counters() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: fixtures/agent.sqlite 不在（它永不入库）");
        return;
    };
    let store = ImportedStore::read(&path).unwrap();
    let counts = store.counts();
    assert_eq!(counts.conversations, 189, "conversations");
    assert_eq!(counts.entries, 209, "entries");
    assert_eq!(counts.tasks, 128, "tasks");
    assert_eq!(counts.submissions, 56, "submissions");
    assert_eq!(counts.documents, 961, "documents");
    assert_eq!(counts.document_revisions, 969, "document_revisions");
    assert_eq!(counts.record_ids, 1609, "record_ids（分配器台账）");
    assert_eq!(store.schema_version, 2, "上游 v2");
    assert_eq!(store.next_id, "1701");
    assert_eq!(store.next_seq, 5074);

    // 硬约束 5：分配的 entry id 比活着的 entry 多（删掉的 35 个仍在台账里）。
    let allocated_entries = store
        .record_ids
        .iter()
        .filter(|row| cell(row, "record_type").as_str() == Some("entry"))
        .count();
    assert_eq!(allocated_entries, 244);
    assert_eq!(allocated_entries - counts.entries, 35);
}

#[test]
fn the_fixture_round_trips_through_a_fresh_database_with_zero_field_diffs() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: fixtures/agent.sqlite 不在（它永不入库）");
        return;
    };
    let imported = ImportedStore::read(&path).unwrap();

    let db_path = temp_db("roundtrip");
    {
        let mut store = SqliteStore::open_writer(&db_path).unwrap();
        imported.write_into(&mut store).unwrap();
    }
    // 重新只读打开（证明落盘的东西能再读出来，而不是内存态）。
    let read_only = SqliteStore::open_read_only(&db_path).unwrap();
    let reread = ImportedStore::read_from(read_only.db(), read_only.schema()).unwrap();
    let _ = std::fs::remove_file(&db_path);

    assert_eq!(reread.counts(), imported.counts());
    assert_eq!((reread.next_id.as_str(), reread.next_seq), (imported.next_id.as_str(), imported.next_seq));

    let tables: [(&str, &Vec<SqliteRow>, &Vec<SqliteRow>); 7] = [
        ("record_ids", &imported.record_ids, &reread.record_ids),
        ("conversations", &imported.conversations, &reread.conversations),
        ("entries", &imported.entries, &reread.entries),
        ("tasks", &imported.tasks, &reread.tasks),
        ("submissions", &imported.submissions, &reread.submissions),
        ("documents", &imported.documents, &reread.documents),
        (
            "document_revisions",
            &imported.document_revisions,
            &reread.document_revisions,
        ),
    ];

    let mut compared_cells = 0usize;
    let mut differences = Vec::new();
    for (table, expected_rows, actual_rows) in tables {
        assert_eq!(expected_rows.len(), actual_rows.len(), "{table} 行数");
        for (index, (expected, actual)) in expected_rows.iter().zip(actual_rows).enumerate() {
            for (column, expected_cell) in expected {
                let actual_cell = cell(actual, column);
                compared_cells += 1;
                if !same_cell(table, column, expected_cell, &actual_cell) {
                    differences.push(format!(
                        "{table}[{index}].{column}: 源 {expected_cell:?} / 读回 {actual_cell:?}"
                    ));
                }
            }
        }
    }
    assert!(differences.is_empty(), "逐字段 diff 必须为 0：{differences:#?}");
    // 七张表加起来接近两万个单元（实测 19,689）。给阈值是为了防止「跳过比对」也算绿。
    assert!(compared_cells > 19_000, "只比了 {compared_cells} 个单元");
}

#[test]
fn the_sessions_render_wire_shaped_items_newest_first() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: fixtures/agent.sqlite 不在（它永不入库）");
        return;
    };
    let store = ImportedStore::read(&path).unwrap();
    let items = render_sessions(&store).unwrap();

    assert_eq!(items.len(), 189);
    // 新的在前。
    assert!(items.windows(2).all(|pair| pair[0].id > pair[1].id), "顺序必须是 id 降序");
    assert_eq!(items.first().unwrap().id, 1694);
    assert_eq!(items.last().unwrap().id, 2);

    let by_id = |id: i64| items.iter().find(|item| item.id == id).unwrap();

    // 名字来自旧引擎的 `qrd3wh.sessions` 文档。
    let session_2 = by_id(2);
    assert_eq!(session_2.name, "真模型");
    assert_eq!(session_2.model.as_deref(), Some("volc-ark/deepseek-v4-flash-260425"));
    assert_eq!(session_2.thinking_level.as_deref(), Some("off"));
    assert_eq!(session_2.parent, None);
    let last = session_2.last.as_ref().unwrap();
    assert_eq!((last.id, last.kind.as_str(), last.text.as_str()), (698, "pi.assistant", ""));

    // 44 是 fork：父是 14、父 entry 24。
    let fork = by_id(44);
    let parent = fork.parent.as_ref().unwrap();
    assert_eq!((parent.id, parent.at), (14, 24));
    assert_eq!(fork.last.as_ref().unwrap().text, "今天是星期三。");

    // 1694 一条 entry 都没有 → `last: null`（不是省略）。
    assert_eq!(by_id(1694).last, None);

    // fixture 的 128 个 task 全是 terminal → 谁都不 busy；归档墓碑本版还没有。
    assert!(items.iter().all(|item| item.busy.is_none()));
    assert!(items.iter().all(|item| item.archived.is_none()));
    // 序列化出来的形状：字段名 camelCase、null 照样在场。
    let json = serde_json::to_value(session_2).unwrap();
    assert!(json.get("thinkingLevel").is_some());
    assert_eq!(json.get("template"), Some(&serde_json::Value::Null));
    assert_eq!(json.get("archived"), Some(&serde_json::Value::Null));
    assert!(json.get("thinking_level").is_none(), "不许留 snake_case");
}

#[test]
fn orphan_names_are_skipped_not_an_error() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: fixtures/agent.sqlite 不在（它永不入库）");
        return;
    };
    let store = ImportedStore::read(&path).unwrap();

    // 旧引擎的名字表有 207 条，会话只有 189 条：18 条名字属于已被撕掉的会话。
    let names_doc = store
        .documents
        .iter()
        .find(|row| cell(row, "kind").as_str() == Some("\"qrd3wh.sessions\""))
        .expect("fixture 里应该有 qrd3wh.sessions 这份文档");
    let doc_id = cell(names_doc, "id").as_i64().unwrap();
    let content = store
        .document_revisions
        .iter()
        .filter(|row| cell(row, "document_id").as_i64() == Some(doc_id))
        .max_by_key(|row| cell(row, "seq").as_i64().unwrap_or(0))
        .map(|row| cell(row, "content").as_str().unwrap().to_string())
        .unwrap();
    let content: serde_json::Value = serde_json::from_str(&content).unwrap();
    let names = content.get("names").and_then(|names| names.as_object()).unwrap();
    assert_eq!(names.len(), 207);
    assert!(names.len() > store.counts().conversations);

    // 拼装时按会话走，孤儿名字既不报错也不进结果。
    let items = render_sessions(&store).unwrap();
    assert_eq!(items.len(), 189);
    assert!(items.iter().all(|item| names.contains_key(&item.id.to_string()) || item.name.is_empty()));
}

#[test]
fn an_upstream_fixture_is_refused_in_write_mode_but_open_read_only() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: fixtures/agent.sqlite 不在（它永不入库）");
        return;
    };
    // 写模式：它是 v2、没有 writer 列 → 明确失败（不是「打开后写坏」）。
    match SqliteStore::open_writer(&path) {
        Err(StorageError::Sqlite(error)) => {
            assert!(
                error.message.contains("newer than supported"),
                "应该是版本过新：{error}"
            );
        }
        Ok(_) => panic!("上游 v2 库不许以写模式打开"),
        Err(other) => panic!("错的失败原因：{other}"),
    }
    // 只读：照读，不跑迁移。
    let store = SqliteStore::open_read_only(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    assert_eq!(store.writer().unwrap(), None, "上游库没有 writer 列");
}

#[test]
fn a_nous_database_carries_its_writer_and_a_foreign_writer_is_refused() {
    let path = temp_db("writer");
    {
        let store = SqliteStore::open_writer(&path).unwrap();
        assert_eq!(store.writer().unwrap().as_deref(), Some(WRITER));
        assert_eq!(store.schema_version().unwrap(), i64::from(nous_durable::storage::CURRENT_VERSION));
        assert_eq!(store.metadata().unwrap(), ("2".to_string(), 1));
    }
    // 篡改 writer：写模式必须拒绝。
    {
        let raw = nous_chord::sqlite::LocalDatabase::open(&path).unwrap();
        raw.run("UPDATE durable_schema SET writer = 'qrd3wh' WHERE singleton = 1", &[])
            .unwrap();
    }
    match SqliteStore::open_writer(&path) {
        Err(StorageError::WriterMismatch { found }) => assert_eq!(found.as_deref(), Some("qrd3wh")),
        other => panic!("认门失败：{:?}", other.err()),
    }
    // 只读照常。
    assert_eq!(
        SqliteStore::open_read_only(&path).unwrap().writer().unwrap().as_deref(),
        Some("qrd3wh")
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_database_without_a_writer_column_is_refused_in_write_mode_but_reads_reasonably() {
    let path = temp_db("no-writer");
    {
        // 只跑 chord 的迁移（没有 writer 列），模拟一份「别人的 v1 库」。
        let schema = nous_durable::storage::SqliteSchema::unprefixed();
        let mut db = nous_chord::sqlite::LocalDatabase::open(&path).unwrap();
        nous_chord::sqlite::apply_sqlite_migrations(
            &mut db,
            &nous_durable::storage::migrations(&schema),
            schema.table("durable_schema"),
        )
        .unwrap();
        db.run("ALTER TABLE durable_schema DROP COLUMN writer", &[]).unwrap();
        assert!(db.get("SELECT version FROM durable_schema", &[]).unwrap().is_some());
    }
    match SqliteStore::open_writer(&path) {
        Err(StorageError::WriterMismatch { found }) => assert_eq!(found, None),
        other => panic!("缺 writer 列的库不许以写模式打开：{:?}", other.err()),
    }
    assert_eq!(SqliteStore::open_read_only(&path).unwrap().writer().unwrap(), None);
    let _ = std::fs::remove_file(&path);
}
