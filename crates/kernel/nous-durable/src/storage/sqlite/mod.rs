//! `storage/sqlite/**`：表名 / DDL / 行编码 / 多行语句 / 记录→行。
//!
//! | 上游 | 本库 | 状态 |
//! |---|---|---|
//! | `sqlite/schema.ts`（63） | `schema.rs` | 原样（表名与索引名一字不改） |
//! | `sqlite/migrations.ts`（138） | `migrations.rs` | 已改（版本从 1 数起、v1 就是最终形状、`+writer` 列） |
//! | `sqlite/rows.ts`（79） | `rows.rs` | 原样 |
//! | `sqlite/statements.ts`（105） | `statements.rs` | 已改（同步、`&dyn SqliteExecutor`） |
//! | `store/tables.ts`（122） | `tables.rs` | 已改（`defineTable` 换成列常量 + 行构造） |
//! | `sqlite/node.ts`（22） / `database.ts`（1） / `index.ts`（8） | — | 已由 `nous-chord::sqlite::LocalDatabase` 覆盖 |
//! | `sqlite/storage.ts`（729） / `sqlite/commit.ts`（486） | — | 12b 的活（提交语义） |
//! | `sqlite/cloudflare.ts`（15） | — | 待提货（Durable Objects 专用，本库用不上） |

pub mod migrations;
pub mod rows;
pub mod schema;
pub mod statements;
pub mod tables;
