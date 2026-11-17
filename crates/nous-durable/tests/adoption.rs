//! 「领养上游 v2 副本」的两条判据：只接受无 `writer` 列且版本 2 的库；领养后可写。

use nous_chord::sqlite::{LocalDatabase, SqliteDatabase};
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
fn as_upstream_v2(path: &std::path::Path) {
    let _ = Storage::open(path).unwrap();
    let mut db = LocalDatabase::open(path).unwrap();
    db.transaction(|tx| -> Result<(), nous_durable::StorageError> {
        tx.exec("ALTER TABLE durable_schema DROP COLUMN writer")?;
        tx.run("UPDATE durable_schema SET version = 2 WHERE singleton = 1", &[])?;
        Ok(())
    })
    .unwrap();
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
    assert_eq!(store.schema_version().unwrap(), 1, "版本号记成我们自己的 1");
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
