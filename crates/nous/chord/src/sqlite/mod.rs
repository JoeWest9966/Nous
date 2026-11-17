//! `packages/chord/src/sqlite/**`：可移植门面 + 迁移 + 前缀校验 + 本地适配器。
//!
//! ## 上游五个文件 → 本库五个
//!
//! | 上游 | 本库 | 状态 |
//! |---|---|---|
//! | `database.ts` | `database.rs` | 已改（去 `Promise`：同步；行是键值表而非泛型反序列化） |
//! | `schema.ts` | `schema.rs` | 原样（两段正则语义一字不改） |
//! | `migrations.ts` | `migrations.rs` | 已改（同语义，`Result` 代替异常） |
//! | `node.ts` | `local.rs` | 已改（`node:sqlite` → `rusqlite`；`BEGIN IMMEDIATE` 写进事务实现） |
//! | `cloudflare.ts` | — | 待提货（Cloudflare Durable Objects 专用；本库是单进程 SQLite，见 `SOURCES.md`） |
//!
//! 判据：能 `mode=ro` 打开 `fixtures/agent.sqlite` 并列出 9 张表
//! （`local.rs::the_fixture_opens_read_only_and_lists_nine_tables`）。

pub mod database;
pub mod error;
pub mod local;
pub mod migrations;
pub mod schema;

pub use database::{SqliteDatabase, SqliteExecutor, SqliteRow, SqliteValue};
pub use error::{SqliteError, SqliteErrorKind};
pub use local::LocalDatabase;
pub use migrations::{apply_sqlite_migrations, SqliteMigration};
pub use schema::validate_schema_prefix;
