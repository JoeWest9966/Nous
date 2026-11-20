//! 判据 2（独立迁移链）+ `open` 自己的 pragma + `backup_to` / `schema_version` 两个薄接口。
//!
//! 这一份测试**不碰** `durable.db`：它只证明「nous.db 自己建得起来，版本链自己数」。
//! 反向（删掉 nous.db，durable 照常打开）由 durable 自己的套件守着 —— 两个 crate 互不依赖。

use std::path::PathBuf;

use nous_store::{Inserted, NewTrigger, Store, StoreError, CURRENT_VERSION, VERSION_TABLE};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("临时目录")
}

fn nous_db(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("nous.db")
}

fn tables(store: &Store) -> Vec<String> {
    let mut stmt = store
        .conn()
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .expect("prepare");
    let rows = stmt.query_map([], |row| row.get::<_, String>(0)).expect("query");
    rows.map(|row| row.expect("row")).collect()
}

fn pragma_i64(store: &Store, pragma: &str) -> i64 {
    store
        .conn()
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .expect("pragma")
}

fn pragma_text(store: &Store, pragma: &str) -> String {
    store
        .conn()
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .expect("pragma")
}

#[test]
fn a_fresh_file_lands_on_version_1_with_exactly_the_two_tables() {
    let dir = temp();
    let store = Store::open(nous_db(&dir)).expect("open");

    assert_eq!(CURRENT_VERSION, 2, "#60 加了 presets_revisions：第二版；升版本是改表结构");
    assert_eq!(store.schema_version().expect("schema_version"), CURRENT_VERSION);
    // 两张表 + 我们自己的版本表。多一张就是有人偷偷加了列/表而没走迁移。
    assert_eq!(tables(&store), vec!["fires", "presets_revisions", "schema_version", "triggers"]);
}

#[test]
fn the_version_table_is_ours_not_durables() {
    // 判据 2 的负向哨兵：store 复用 durable 的迁移版本表 → 这条红。
    let dir = temp();
    let store = Store::open(nous_db(&dir)).expect("open");
    assert_eq!(VERSION_TABLE, "schema_version");
    assert_ne!(VERSION_TABLE, "durable_schema", "durable 的版本表叫 durable_schema");
    assert!(tables(&store).contains(&"schema_version".to_string()));
    assert!(!tables(&store).contains(&"durable_schema".to_string()));
}

#[test]
fn a_database_that_never_saw_durable_still_opens_and_migrates() {
    // 判据 2 的前半：目录里没有 durable.db，只有 nous.db。
    let dir = temp();
    assert!(!dir.path().join("durable.db").exists());

    let store = Store::open(nous_db(&dir)).expect("open");
    assert_eq!(store.schema_version().expect("version"), CURRENT_VERSION);
    assert!(!dir.path().join("durable.db").exists(), "store 不许顺手建 durable 的库");
    assert!(nous_db(&dir).exists());
}

#[test]
fn a_durable_side_version_table_is_left_alone_and_never_reads_as_ours() {
    // 同一份文件里躺着 durable 形状的版本表（`durable_schema` = 2 + `writer` 列）：
    // 那是别的库的事。store 不许把它当自己的版本读 —— 读错了就会抛 `SchemaTooNew`。
    let dir = temp();
    let path = nous_db(&dir);
    {
        let conn = rusqlite::Connection::open(&path).expect("seed");
        conn.execute_batch(
            "CREATE TABLE durable_schema (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 version   INTEGER NOT NULL,
                 writer    TEXT NOT NULL
             ) STRICT;
             INSERT INTO durable_schema (singleton, version, writer) VALUES (1, 2, 'nous');",
        )
        .expect("seed");
    }

    let store = Store::open(&path).expect("durable 的版本表不该拦我们");
    assert_eq!(store.schema_version().expect("version"), CURRENT_VERSION);
    let durable_version: i64 = store
        .conn()
        .query_row("SELECT version FROM durable_schema WHERE singleton = 1", [], |row| row.get(0))
        .expect("durable_schema 原样还在");
    assert_eq!(durable_version, 2, "别人的版本号一个字节都不许动");
}

#[test]
fn deleting_one_file_leaves_the_other_library_alone() {
    // 判据 2 的两半放在同一个目录里看：两库是两个文件，删哪一个都不该动到另一个。
    // （真 `nous-durable` 不在这里打开 —— 本 crate 依赖它就把判据 1 破了。所以「durable 形状」
    //   用它的版本表 + 一张真表模拟：我们证明的是**文件与版本表**互不相干。）
    let dir = temp();
    let nous = nous_db(&dir);
    let durable = dir.path().join("durable.db");
    {
        let conn = rusqlite::Connection::open(&durable).expect("durable 形状");
        conn.execute_batch(
            "CREATE TABLE durable_schema (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 version   INTEGER NOT NULL,
                 writer    TEXT NOT NULL
             ) STRICT;
             INSERT INTO durable_schema (singleton, version, writer) VALUES (1, 2, 'nous');
             CREATE TABLE entries (id INTEGER PRIMARY KEY, record TEXT NOT NULL);",
        )
        .expect("durable 形状");

        let store = Store::open(&nous).expect("store 建自己的库");
        store
            .create_trigger(NewTrigger { code: "nightly_digest", name: "夜报", spec: "{}" }, 1_000)
            .expect("create");
    }

    // 删掉 nous.db（连 WAL 边车）：durable 那边一个字节没动。
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", nous.display()));
    }
    {
        let conn = rusqlite::Connection::open(&durable).expect("durable 照常打开");
        let version: i64 = conn
            .query_row("SELECT version FROM durable_schema WHERE singleton = 1", [], |row| row.get(0))
            .expect("durable 的版本表还在");
        assert_eq!(version, 2, "store 的迁移没碰过 durable 的版本号");
    }

    // nous.db 不见了 → 重开到 CURRENT_VERSION；durable.db 的名字与内容都不受影响。
    let store = Store::open(&nous).expect("重新建库");
    assert_eq!(store.schema_version().expect("version"), CURRENT_VERSION);
    assert!(store.triggers().expect("triggers").is_empty(), "删掉的是库，不是「变成了永久墓碑」");
    let conn = rusqlite::Connection::open(&durable).expect("durable 仍在");
    let left: i64 = conn
        .query_row("SELECT count(*) FROM entries", [], |row| row.get(0))
        .expect("durable 的表还在");
    assert_eq!(left, 0);
}

#[test]
fn an_in_memory_store_runs_the_same_migrations_and_constraints() {
    // platform 层的「无 IO 替身」：同一份代码、不落盘。判据里需要「不碰盘」的地方用它。
    let store = Store::open_in_memory().expect("in-memory");
    assert_eq!(store.schema_version().expect("version"), CURRENT_VERSION);
    let created = store
        .create_trigger(NewTrigger { code: "nightly_digest", name: "夜报", spec: "{}" }, 1_000)
        .expect("create");
    assert_eq!(created.code, "nightly_digest");
    // 约束也在（不是「内存库就没规矩了」）。
    store
        .conn()
        .execute("UPDATE triggers SET code = 'other_code' WHERE code = 'nightly_digest'", [])
        .expect_err("内存库里 code 一样不可变");
}

#[test]
fn a_newer_schema_refuses_to_open() {
    let dir = temp();
    let path = nous_db(&dir);
    Store::open(&path).expect("首次建库");
    {
        let conn = rusqlite::Connection::open(&path).expect("seed");
        conn.execute(
            &format!("UPDATE {VERSION_TABLE} SET version = ?1 WHERE singleton = 1"),
            [CURRENT_VERSION + 1],
        )
        .expect("版本调高");
    }

    match Store::open(&path) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, CURRENT_VERSION + 1);
            assert_eq!(supported, CURRENT_VERSION);
        }
        other => panic!("库比代码新时必须拒绝打开，实得 {other:?}"),
    }
}

#[test]
fn reopening_is_idempotent() {
    let dir = temp();
    let path = nous_db(&dir);
    {
        let store = Store::open(&path).expect("open");
        store
            .create_trigger(NewTrigger { code: "nightly_digest", name: "夜报", spec: "{}" }, 1_000)
            .expect("create");
    }
    let again = Store::open(&path).expect("reopen");
    assert_eq!(again.schema_version().expect("version"), CURRENT_VERSION);
    assert_eq!(again.triggers().expect("triggers").len(), 1, "重开不许重跑迁移/重插");
}

#[test]
fn open_sets_wal_busy_timeout_and_normal_sync() {
    let dir = temp();
    let store = Store::open(nous_db(&dir)).expect("open");
    assert_eq!(pragma_text(&store, "journal_mode").to_lowercase(), "wal");
    assert_eq!(pragma_i64(&store, "busy_timeout"), 5_000);
    // 1 = NORMAL（0 = OFF，2 = FULL，3 = EXTRA）。掉电丢最近若干提交，库不会坏。
    assert_eq!(pragma_i64(&store, "synchronous"), 1);
}

#[test]
fn backup_to_writes_a_readable_copy_and_never_overwrites() {
    let dir = temp();
    let store = Store::open(nous_db(&dir)).expect("open");
    store
        .create_trigger(
            NewTrigger { code: "nightly_digest", name: "夜报", spec: r#"{"cron":"0 3 * * *"}"# },
            1_700_000_000_000,
        )
        .expect("create");
    assert_eq!(
        store.record_fire("nightly_digest", 1_700_000_060_000, Some("首轮")).expect("fire"),
        Inserted::New
    );

    let copy = dir.path().join("nous-backup.db");
    store.backup_to(&copy).expect("backup");

    let copied = Store::open(&copy).expect("备份是可读的库");
    assert_eq!(copied.schema_version().expect("version"), CURRENT_VERSION);
    assert_eq!(copied.trigger("nightly_digest").expect("trigger").expect("有").name, "夜报");
    let fires = copied.fires().expect("fires");
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].note.as_deref(), Some("首轮"));

    // 备份不许静默盖掉上一份（覆盖 = 上一份备份没了，也看不出来）。
    match store.backup_to(&copy) {
        Err(StoreError::BackupTargetExists { .. }) => {}
        other => panic!("目标已存在时 backup_to 必须报错，实得 {other:?}"),
    }
}
