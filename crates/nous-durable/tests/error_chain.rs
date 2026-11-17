//! 事务边界的错误**不许被压成字符串**。
//!
//! `nous-chord` 的 `transaction` 把错误类型交给调用方定（`E: From<SqliteError>`），代价是
//! 「转换里偷偷 `to_string()`」这条捷径变得很容易走。两个方向都钉住：
//!
//! 1. 我们自己抛的分类（`Shape`）原样穿过事务边界 —— 不被包成 `Sqlite`、消息一字不改；
//! 2. 后端报的错走到调用方手里时，`source()` 链上还挂着 `SqliteError` 与
//!    `rusqlite::Error`，分类（`Backend`）还在。

use std::error::Error;

use nous_chord::sqlite::{SqliteDatabase, SqliteErrorKind};
use nous_durable::storage::SqliteStore;
use nous_durable::StorageError;

fn fresh(tag: &str) -> (std::path::PathBuf, SqliteStore) {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-errchain-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let store = SqliteStore::open_writer(&path).unwrap();
    (path, store)
}

#[test]
fn a_shape_error_crosses_the_transaction_boundary_unchanged() {
    let (path, mut store) = fresh("shape");
    let error = store
        .db_mut()
        .transaction(|_tx| -> Result<(), StorageError> {
            Err(StorageError::Shape("entry 12: kind 不在文档的取值集合里".to_string()))
        })
        .unwrap_err();
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        error,
        StorageError::Shape("entry 12: kind 不在文档的取值集合里".to_string()),
        "事务不许把分类改写成 Sqlite(..)，也不许加前后缀"
    );
    assert!(error.source().is_none(), "Shape 没有底层原因，别凭空造一条");
}

#[test]
fn a_backend_error_keeps_its_rusqlite_cause_in_the_chain() {
    let (path, mut store) = fresh("backend");
    let error = store
        .db_mut()
        .transaction(|tx| -> Result<(), StorageError> {
            // `record_ids.record_type` 是 NOT NULL：撞约束就是一条真后端错误。
            tx.run("INSERT INTO record_ids (id, record_type) VALUES (1, NULL)", &[])?;
            Ok(())
        })
        .unwrap_err();
    let _ = std::fs::remove_file(&path);

    // 第一层：分类。
    let StorageError::Sqlite(sqlite) = &error else {
        panic!("后端错误应该落成 Sqlite(..)，实际 {error:?}");
    };
    assert_eq!(sqlite.kind, SqliteErrorKind::Backend);
    assert!(sqlite.message.contains("NOT NULL"), "{}", sqlite.message);

    // 第二层：`StorageError::source()` 交出 `SqliteError`。
    let first = error.source().expect("StorageError::Sqlite 必须交出 source");
    assert!(first.to_string().starts_with("sqlite "), "{first}");

    // 第三层：`SqliteError::source()` 交出 `rusqlite::Error` —— 想按错误码分流的人从这里拿。
    let second = first.source().expect("后端错误的 source 不许是空的");
    let text = second.to_string();
    assert!(
        text.to_lowercase().contains("not null") || text.contains("constraint"),
        "source 链底应该是 rusqlite 的原话：{text}"
    );
    assert_eq!(sqlite.cause().map(|cause| cause.to_string()), Some(second.to_string()));

    // 反过来：我们自己判定的错误（版本过新 / 前缀不合法）没有底层原因，`cause()` 是 `None`。
    let ours = nous_chord::sqlite::SqliteError::new(SqliteErrorKind::SchemaTooNew, "v9 > v1");
    assert!(ours.cause().is_none());
    assert!(Error::source(&ours).is_none());
}
