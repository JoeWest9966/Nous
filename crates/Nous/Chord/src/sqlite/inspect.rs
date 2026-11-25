//! 只读巡检：`PRAGMA` 与 sqlite3 运维命令的**唯一出处**。
//!
//! 判据 5（数据面入口可数）只认内核层（wire / chord / durable / ai-types）与标了
//! `sqlite-entry` 的包。`PRAGMA` 这类数据面方言因此只能写在那些 crate 里；`stats`
//! 这类**只读消费者**来这里取，不在自己家里拼 SQL。
//!
//! 规矩：`name` 只接收本文件认识的字面量（见 [`Pragma`]），不接受调用方随便传字符串 ——
//! 巡检面不该成为拼 SQL 的后门。

use std::path::Path;

use super::{SqliteExecutor, SqliteRow, SqliteValue};

/// 巡检会用到的 `PRAGMA` 名字。**白名单**：不在表里的一律不给跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pragma {
    /// 页大小（字节）。
    PageSize,
    /// 页数。
    PageCount,
    /// 空闲页数。
    FreelistCount,
    /// `wal` / `delete` / `truncate`。
    JournalMode,
    /// `ok` / 若干条坏块描述。
    IntegrityCheck,
    /// `checkpoint` 三列（busy / log / checkpointed）。
    WalCheckpoint,
}

impl Pragma {
    /// SQL 里那个名字。只有这张表里的值会进 SQL 文本。
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::PageSize => "page_size",
            Self::PageCount => "page_count",
            Self::FreelistCount => "freelist_count",
            Self::JournalMode => "journal_mode",
            Self::IntegrityCheck => "integrity_check",
            Self::WalCheckpoint => "wal_checkpoint(TRUNCATE)",
        }
    }
}

/// 跑一条巡检 `PRAGMA`，回全部行（行里是键值表，见 [`SqliteRow`]）。
pub fn pragma(db: &dyn SqliteExecutor, which: Pragma) -> Vec<SqliteRow> {
    db.all(&format!("PRAGMA {}", which.as_sql()), &[]).unwrap_or_default()
}

/// `PRAGMA` 的第一个整数（缺席 / 非整数 → 0）。只用于读数聚合，缺值当零是故意的。
pub fn pragma_i64(db: &dyn SqliteExecutor, which: Pragma) -> i64 {
    pragma(db, which)
        .first()
        .and_then(|row| row.values().next().and_then(SqliteValue::as_i64))
        .unwrap_or(0)
}

/// `PRAGMA` 的第一个文本（`journal_mode` 那种）。
pub fn pragma_text(db: &dyn SqliteExecutor, which: Pragma) -> String {
    pragma(db, which)
        .first()
        .and_then(|row| row.values().next().and_then(SqliteValue::as_str).map(str::to_owned))
        .unwrap_or_default()
}

/// `PRAGMA integrity_check` 的逐条结论（第一列文本）。空 = 一条也没跑出来。
pub fn integrity_messages(db: &dyn SqliteExecutor) -> Vec<String> {
    pragma(db, Pragma::IntegrityCheck)
        .iter()
        .filter_map(|row| {
            row.values().next().map(|value| match value {
                SqliteValue::Text(text) => text.clone(),
                other => format!("{other:?}"),
            })
        })
        .collect()
}

/// 给运维看的命令行：把 WAL 收回去（**只给文本，不执行**）。
pub fn advice_wal_checkpoint(path: &Path) -> String {
    format!(
        "sqlite3 \"{}\" \"PRAGMA {};\"",
        path.display(),
        Pragma::WalCheckpoint.as_sql()
    )
}

/// 给运维看的命令行：跑一遍整库校验（**只给文本，不执行**）。
pub fn advice_integrity_check(path: &Path) -> String {
    format!(
        "sqlite3 \"{}\" \"PRAGMA {};\"",
        path.display(),
        Pragma::IntegrityCheck.as_sql()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::LocalDatabase;
    use std::path::PathBuf;

    fn fixture() -> Option<PathBuf> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/agent.sqlite");
        path.exists().then_some(path)
    }

    /// 白名单就是全部：SQL 里出现的那几个名字只来自这张表。
    #[test]
    fn the_whitelist_covers_exactly_what_we_run() {
        let names: Vec<&str> = [
            Pragma::PageSize,
            Pragma::PageCount,
            Pragma::FreelistCount,
            Pragma::JournalMode,
            Pragma::IntegrityCheck,
            Pragma::WalCheckpoint,
        ]
        .iter()
        .map(|pragma| pragma.as_sql())
        .collect();
        assert_eq!(
            names,
            vec!["page_size", "page_count", "freelist_count", "journal_mode", "integrity_check", "wal_checkpoint(TRUNCATE)"]
        );
        assert!(names.iter().all(|name| !name.contains(';')), "名字里不许带分号：那是拼 SQL 的后门");
    }

    /// 真库上读得出来：页大小、页数、journal_mode、integrity_check。
    #[test]
    fn pragmas_read_a_real_database() {
        let Some(path) = fixture() else { return };
        let db = LocalDatabase::open_read_only(&path).unwrap();

        assert!(pragma_i64(&db, Pragma::PageSize) > 0, "页大小要读得出来");
        assert!(pragma_i64(&db, Pragma::PageCount) > 0, "页数要读得出来");
        assert_eq!(pragma_text(&db, Pragma::JournalMode), "wal");
        assert_eq!(integrity_messages(&db), vec!["ok".to_string()]);
    }

    /// 运维命令只给文本：路径被引号包住，SQL 语句来自白名单。
    #[test]
    fn advice_texts_quote_the_path_and_carry_no_extra_statements() {
        let path = Path::new("/var/lib/nous/nous.db");
        let wal = advice_wal_checkpoint(path);
        let check = advice_integrity_check(path);
        assert_eq!(
            wal,
            "sqlite3 \"/var/lib/nous/nous.db\" \"PRAGMA wal_checkpoint(TRUNCATE);\""
        );
        assert_eq!(check, "sqlite3 \"/var/lib/nous/nous.db\" \"PRAGMA integrity_check;\"");
    }
}
