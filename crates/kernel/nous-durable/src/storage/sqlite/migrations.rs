//! DDL 与迁移链（上游 `storage/sqlite/migrations.ts`，138 行）。
//!
//! ## 版本号是我们自己的
//!
//! 上游 v1 是初版、v2 给 `tasks.status` 加了 `sleeping`。**本库从 1 数起**，v1 直接就是我们
//! 要的形状（含 `sleeping`），所以只有一条迁移 —— 上游那两次重建 `tasks` 表的历史不复制
//! （`docs/storage.md` §3）。
//!
//! ## 唯一的结构改动：`durable_schema.writer`
//!
//! `durable_schema` 由 `nous_chord::sqlite::apply_sqlite_migrations` 建（singleton + version），
//! 认门要的那一列在这里 `ALTER` 上去，然后写成 `'nous'`。**表数仍是 9** —— 加的是列，不是表。
//!
//! 只读打开上游 v2 库时不跑任何迁移（它没有 `writer` 列，版本号也不同），走 `import`。

use nous_chord::sqlite::SqliteMigration;

use super::schema::SqliteSchema;

/// 认门的值。写模式必须等于它（`docs/storage.md` §3）。
pub const WRITER: &str = "nous";

/// 这一版支持的 schema 版本 = 迁移链最后一条。
pub const CURRENT_VERSION: u32 = 1;

/// 给 `schema` 渲染出完整的迁移链。上游 `createSqliteMigrations`。
///
/// 键序与语句顺序**照抄上游 v2 的形状**（v1 的初版 + v2 的 `sleeping`），因为我们发布的 v1
/// 就是它俩的最终结果。改这里的任何一句 = 改表结构 = 先改 `docs/storage.md`。
pub fn migrations(schema: &SqliteSchema) -> Vec<SqliteMigration> {
    let t = |key: &str| schema.table(key).to_string();
    let i = |key: &str| schema.index(key).to_string();
    vec![SqliteMigration::new(
        1,
        [
            format!(
                "CREATE TABLE {} (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    next_id TEXT NOT NULL,
                    next_seq INTEGER NOT NULL
                ) STRICT",
                t("durable_metadata")
            ),
            format!(
                "INSERT INTO {} (singleton, next_id, next_seq) VALUES (1, '2', 1)",
                t("durable_metadata")
            ),
            format!(
                "CREATE TABLE {} (
                    id INTEGER PRIMARY KEY,
                    record_type TEXT NOT NULL CHECK (record_type IN ('conversation', 'entry', 'task', 'submission', 'document'))
                ) STRICT",
                t("record_ids")
            ),
            format!(
                "CREATE TABLE {} (
                    id INTEGER PRIMARY KEY,
                    owner_conversation_id INTEGER,
                    owner_task_id INTEGER,
                    record TEXT NOT NULL CHECK (json_valid(record))
                ) STRICT",
                t("conversations")
            ),
            format!(
                "CREATE INDEX {} ON {} (owner_conversation_id, id)",
                i("conversations_by_owner_conversation"),
                t("conversations")
            ),
            format!(
                "CREATE INDEX {} ON {} (owner_task_id, id)",
                i("conversations_by_owner_task"),
                t("conversations")
            ),
            format!(
                "CREATE TABLE {} (
                    id INTEGER PRIMARY KEY,
                    conversation_id INTEGER NOT NULL,
                    head INTEGER,
                    commit_seq INTEGER NOT NULL,
                    record TEXT NOT NULL CHECK (json_valid(record))
                ) STRICT",
                t("entries")
            ),
            format!(
                "CREATE INDEX {} ON {} (conversation_id, id DESC)",
                i("entries_by_conversation"),
                t("entries")
            ),
            format!(
                "CREATE INDEX {} ON {} (conversation_id, id DESC) WHERE head IS NOT NULL",
                i("entry_heads_by_conversation"),
                t("entries")
            ),
            format!(
                "CREATE TABLE {} (
                    id INTEGER PRIMARY KEY,
                    conversation_id INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'waiting', 'sleeping', 'completing', 'terminal')),
                    abort_requested INTEGER NOT NULL CHECK (abort_requested IN (0, 1)),
                    background INTEGER NOT NULL CHECK (background IN (0, 1)),
                    record TEXT NOT NULL CHECK (json_valid(record))
                ) STRICT",
                t("tasks")
            ),
            format!("CREATE INDEX {} ON {} (status, id)", i("tasks_by_status"), t("tasks")),
            format!(
                "CREATE INDEX {} ON {} (conversation_id, id)",
                i("tasks_by_conversation"),
                t("tasks")
            ),
            format!("CREATE INDEX {} ON {} (kind, id)", i("tasks_by_kind"), t("tasks")),
            format!(
                "CREATE INDEX {} ON {} (abort_requested, id)",
                i("tasks_by_abort_requested"),
                t("tasks")
            ),
            format!(
                "CREATE INDEX {} ON {} (background, id)",
                i("tasks_by_background"),
                t("tasks")
            ),
            format!(
                "CREATE TABLE {} (
                    id INTEGER PRIMARY KEY,
                    conversation_id INTEGER NOT NULL,
                    request_id TEXT,
                    status TEXT NOT NULL CHECK (status IN ('queued', 'placed', 'done', 'unanswered')),
                    record TEXT NOT NULL CHECK (json_valid(record))
                ) STRICT",
                t("submissions")
            ),
            format!(
                "CREATE INDEX {} ON {} (conversation_id, request_id)",
                i("submissions_by_request"),
                t("submissions")
            ),
            format!(
                "CREATE INDEX {} ON {} (conversation_id, id)",
                i("submissions_by_conversation"),
                t("submissions")
            ),
            format!(
                "CREATE INDEX {} ON {} (status, id)",
                i("submissions_by_status"),
                t("submissions")
            ),
            format!(
                "CREATE TABLE {} (
                    id INTEGER PRIMARY KEY,
                    kind TEXT NOT NULL,
                    family INTEGER NOT NULL CHECK (family IN (0, 1)),
                    key_value TEXT NOT NULL,
                    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('session', 'conversation', 'task')),
                    owner_id INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    retired_at INTEGER,
                    record TEXT NOT NULL CHECK (json_valid(record))
                ) STRICT",
                t("documents")
            ),
            format!(
                "CREATE INDEX {} ON {} (kind, scope_kind, owner_id, family, key_value, created_at DESC, retired_at)",
                i("documents_by_address"),
                t("documents")
            ),
            format!(
                "CREATE INDEX {} ON {} (scope_kind, owner_id, id)",
                i("documents_by_scope"),
                t("documents")
            ),
            format!(
                "CREATE INDEX {} ON {} (scope_kind, owner_id, kind, id)",
                i("documents_by_scope_kind"),
                t("documents")
            ),
            format!(
                "CREATE TABLE {} (
                    document_id INTEGER NOT NULL,
                    seq INTEGER NOT NULL,
                    kind TEXT NOT NULL CHECK (kind IN ('base', 'delta')),
                    version INTEGER NOT NULL,
                    content TEXT NOT NULL CHECK (json_valid(content)),
                    PRIMARY KEY (document_id, seq)
                ) STRICT",
                t("document_revisions")
            ),
            format!(
                "CREATE INDEX {} ON {} (document_id, kind, seq DESC)",
                i("document_revisions_by_kind"),
                t("document_revisions")
            ),
            // 认门那一列：`durable_schema` 由 chord 的迁移助手建好，这里补列 + 写值。
            format!("ALTER TABLE {} ADD COLUMN writer TEXT", t("durable_schema")),
            format!(
                "UPDATE {} SET writer = '{WRITER}' WHERE writer IS NULL",
                t("durable_schema")
            ),
        ],
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_chord::sqlite::{apply_sqlite_migrations, LocalDatabase, SqliteExecutor};

    #[test]
    fn the_chain_starts_at_one_and_ends_at_current_version() {
        let chain = migrations(&SqliteSchema::unprefixed());
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].version, 1);
        assert_eq!(chain.last().unwrap().version, CURRENT_VERSION);
        // 25 条建表/建索引 + 2 条补 writer = 27。上游是 30 条，多出来的是 v2 为了给
        // `tasks.status` 加 `sleeping` 而重建整张表的 5 条 —— 我们 v1 直接就是最终形状。
        assert_eq!(chain[0].statements.len(), 27);
    }

    #[test]
    fn applying_the_chain_creates_nine_tables_and_sixteen_indexes_plus_the_writer_column() {
        let schema = SqliteSchema::unprefixed();
        let mut db = LocalDatabase::open_in_memory().unwrap();
        apply_sqlite_migrations(&mut db, &migrations(&schema), schema.table("durable_schema")).unwrap();

        let tables: Vec<String> = db
            .all("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name", &[])
            .unwrap()
            .into_iter()
            .filter_map(|row| row.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        assert_eq!(tables.len(), 9, "{tables:?}");
        for (_, name) in super::super::schema::TABLE_NAMES {
            assert!(tables.contains(&name.to_string()), "{name} 没建出来");
        }

        let indexes: Vec<String> = db
            .all(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                &[],
            )
            .unwrap()
            .into_iter()
            .filter_map(|row| row.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        assert_eq!(indexes.len(), 16, "{indexes:?}");

        // 认门那一列在，值是 nous；版本是 1（不是上游的 2）。
        let row = db
            .get("SELECT version, writer FROM durable_schema WHERE singleton = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(row.get("version").unwrap().as_i64(), Some(1));
        assert_eq!(row.get("writer").unwrap().as_str(), Some(WRITER));

        // `pi.live` 那一列默认值：分配器起始值照上游（next_id '2' / next_seq 1）。
        let metadata = db
            .get("SELECT next_id, next_seq FROM durable_metadata WHERE singleton = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(metadata.get("next_id").unwrap().as_str(), Some("2"));
        assert_eq!(metadata.get("next_seq").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn a_prefixed_schema_renders_prefixed_identifiers() {
        let schema = SqliteSchema::with_prefix("agent_").unwrap();
        let mut db = LocalDatabase::open_in_memory().unwrap();
        apply_sqlite_migrations(&mut db, &migrations(&schema), schema.table("durable_schema")).unwrap();
        let row = db
            .get("SELECT version FROM agent_durable_schema WHERE singleton = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(row.get("version").unwrap().as_i64(), Some(1));
    }
}
