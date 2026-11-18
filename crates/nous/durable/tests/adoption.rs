//! 「领养上游 v2 副本」的两条判据：只接受无 `writer` 列且版本 2 的库；领养后可写。

use nous_chord::sqlite::{LocalDatabase, SqliteDatabase, SqliteExecutor};
use nous_durable::storage::adopt_legacy_copy;
use nous_durable::Storage;

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-adoption-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

/// 造一份「上游 v2 形状」的库：本库 v1 → 去掉 `writer` 列、版本号写成 2。
///
/// **还要删掉 `#38` 的终态索引**：那是本库 v2 自己加的，上游 v2 没有。不删的话这份夹具
/// 就自带索引，测试永远看不到「领养之后索引缺失」这个真问题（真库副本实测缺，见 #38 收尾）。
fn as_upstream_v2(path: &std::path::Path) {
    let _ = Storage::open(path).unwrap();
    let mut db = LocalDatabase::open(path).unwrap();
    db.transaction(|tx| -> Result<(), nous_durable::StorageError> {
        tx.exec("ALTER TABLE durable_schema DROP COLUMN writer")?;
        tx.run("UPDATE durable_schema SET version = 2 WHERE singleton = 1", &[])?;
        Ok(())
    })
    .unwrap();
    db.execute_batch("DROP INDEX IF EXISTS entry_terminals_by_conversation")
        .unwrap();
}

/// 终态索引在不在（`terminal_entry_in` 快不快全靠它）。
fn terminal_index_exists(path: &std::path::Path) -> bool {
    LocalDatabase::open(path)
        .unwrap()
        .all(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'entry_terminals_by_conversation'",
            &[],
        )
        .unwrap()
        .len()
        == 1
}

#[test]
fn adopting_a_v2_copy_renumbers_and_unlocks_the_writer_gate() {
    let path = temp_db("ok");
    as_upstream_v2(&path);
    // 认门之前：写模式明确失败。
    assert!(
        Storage::open(&path).is_err(),
        "v2 库在写模式下必须被拒（§12.3）"
    );

    adopt_legacy_copy(&path).expect("无 writer 列 + 版本 2 → 可以领养");
    let storage = Storage::open(&path).expect("领养后可写");
    let store = storage.store();
    assert_eq!(
        store.schema_version().unwrap(),
        2,
        "记成当前版本 2（v2 的终态索引已随库存在，写回 1 会重跑建索引并撞名）"
    );
    assert_eq!(store.writer().unwrap().as_deref(), Some("nous"));
    drop(storage);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn adoption_refuses_a_library_that_already_has_a_writer() {
    let path = temp_db("has-writer");
    let _ = Storage::open(&path).unwrap(); // 本库自己的库：有 writer='nous'
    let error = adopt_legacy_copy(&path).expect_err("已有认门值就不许再领养");
    assert!(
        error.to_string().contains("writer"),
        "错误要说清是认门列的问题，实测 {error}"
    );
    let _ = std::fs::remove_file(&path);
}

/// 领养必须把**终态索引**补上：领养只写 `version = 1`，下一次写模式打开重跑 v2
/// （`CREATE INDEX IF NOT EXISTS`）把索引建出来，版本站到 2。
///
/// 不补的后果不是报错，是**静默退化**：`terminal_entry_in` 掉回
/// `entries_by_conversation` 索引 + 排序，谁都不会红（2026-11-18 在真库副本上实测）。
#[test]
fn adopting_backfills_the_terminal_index_on_the_next_open() {
    let path = temp_db("terminal-index");
    as_upstream_v2(&path);
    assert!(
        !terminal_index_exists(&path),
        "夹具造错了：上游 v2 不该带本库的终态索引"
    );

    adopt_legacy_copy(&path).unwrap();
    assert!(
        !terminal_index_exists(&path),
        "领养只写 writer 与版本号，不该顺手建索引（建索引是迁移的活）"
    );

    // 下一次写模式打开 = 跑迁移 = 索引补上。
    let storage = Storage::open(&path).unwrap();
    assert!(
        terminal_index_exists(&path),
        "领养之后必须补上终态索引，否则 terminal_entry_in 静默退化"
    );
    drop(storage);

    let version: Vec<_> = LocalDatabase::open(&path)
        .unwrap()
        .all("SELECT version FROM durable_schema WHERE singleton = 1", &[])
        .unwrap();
    let version = version[0].get("version").unwrap().as_i64().unwrap();
    assert_eq!(version, 2, "迁移跑完版本必须是 2");
}
