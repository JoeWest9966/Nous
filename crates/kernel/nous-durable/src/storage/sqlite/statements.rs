//! 一次提交的多行语句：**行数再多，语句数也极少**。上游 `storage/sqlite/statements.ts`（105 行）。
//!
//! 三条来自上游的硬约束，抄在这里的原因是我们也会撞上：
//!
//! 1. **绑定参数的条数有上限**（Durable Objects 100 个；本机 SQLite 默认 32,766，但别赌）。
//!    所以小行走「一条 JSON 数组参数 + `json_each(?)`」：一行一格，SQL 文本不随行数变。
//! 2. **大行一次绑定列**，用 1 / 2 / 4 … 行一段的多行 `VALUES`：`record` 已经是 JSON 文本，
//!    再套一层 JSON 数组就是二次转义，8 KB 以上不划算。
//! 3. **单个参数超过 2 MB 会被拒**，所以数组本身也按 512 K 字符切段。
//!
//! 判据在 `tests/storage_roundtrip.rs`：189 条会话 + 209 条 entry + 961 份文档全部走这几条路径，
//! 且一次提交的语句条数远小于行数（`insert_rows_counts_more_rows_than_statements`）。

use nous_chord::sqlite::{SqliteExecutor, SqliteRow, SqliteValue as Cell};

use crate::error::Result;

/// 一行的字符数（UTF-16 码元，和上游 `String.length` 同一个口径）超过它就改走绑定路径。
const LARGE_ROW_CHARACTERS: usize = 8 * 1024;

/// 单条语句最多绑定多少个参数。
const MAX_PARAMETERS: usize = 100;

/// 单个数组参数的字符上限（512 Ki 码元 ≈ 1.5 MB UTF-8）。
const MAX_ARRAY_CHARACTERS: usize = 512 * 1024;

/// 把一段 `sql` 里的那一个 `?` 当作 JSON 数组参数，逐段执行并收集行。
///
/// `sql` 的写法约定：参数在 `json_each(?)` 里出现，例如
/// `SELECT * FROM entries WHERE id IN (SELECT value FROM json_each(?))`。
pub fn read_rows(executor: &dyn SqliteExecutor, sql: &str, values: &[Cell]) -> Result<Vec<SqliteRow>> {
    read_rows_parts(executor, sql, values.iter().map(|cell| cell_json(cell).to_string()).collect())
}

/// 同 [`read_rows`]，但每一段自己就是一段 JSON —— 用于「数组里装数组」的查询（地址元组）。
pub fn read_rows_parts(
    executor: &dyn SqliteExecutor,
    sql: &str,
    parts: Vec<String>,
) -> Result<Vec<SqliteRow>> {
    let mut rows = Vec::new();
    for array in json_arrays(parts)? {
        rows.extend(executor.all(sql, &[array.into()])?);
    }
    Ok(rows)
}

/// 同 [`read_rows`]，用于删除类语句。
pub fn run_for_each(executor: &dyn SqliteExecutor, sql: &str, values: &[Cell]) -> Result<()> {
    for array in json_arrays(values.iter().map(|cell| cell_json(cell).to_string()).collect())? {
        executor.run(sql, &[array.into()])?;
    }
    Ok(())
}

/// 把 `rows`（每行一格、按 `columns` 顺序）写进 `table`，`conflict` 是 upsert 的尾巴。
pub fn insert_rows(
    executor: &dyn SqliteExecutor,
    table: &str,
    columns: &[&str],
    rows: &[Vec<Cell>],
    conflict: &str,
) -> Result<()> {
    let mut small: Vec<Vec<Cell>> = Vec::new();
    let mut large: Vec<Vec<Cell>> = Vec::new();
    for row in rows {
        let text: usize = row
            .iter()
            .map(|cell| match cell {
                Cell::Text(value) => value.encode_utf16().count(),
                // Blob 进不了 JSON 数组，一律走绑定路径。
                Cell::Blob(value) => LARGE_ROW_CHARACTERS + value.len(),
                _ => 0,
            })
            .sum();
        if text > LARGE_ROW_CHARACTERS {
            large.push(row.clone());
        } else {
            small.push(row.clone());
        }
    }

    let names = columns.join(", ");
    if !small.is_empty() {
        let cells = (0..columns.len())
            .map(|index| format!("json_extract(item.value, '$[{index}]')"))
            .collect::<Vec<_>>()
            .join(", ");
        // `WHERE true` 让 SQLite 不把 upsert 的 `ON` 读成 `SELECT` 的联结条件。
        let sql = format!(
            "INSERT INTO {table} ({names}) SELECT {cells} FROM json_each(?) AS item WHERE true {conflict}"
        );
        let parts: Vec<String> = small
            .iter()
            .map(|row| serde_json::to_string(&row.iter().map(cell_json).collect::<Vec<_>>()))
            .collect::<std::result::Result<_, _>>()
            .map_err(crate::error::StorageError::from)?;
        for array in json_arrays(parts)? {
            executor.run(&sql, &[array.into()])?;
        }
    }

    insert_bound(executor, &format!("INSERT INTO {table} ({names}) VALUES"), columns.len(), &large, conflict)
}

/// 大行路径：`width` 格一条，1 / 2 / 4 … 行一段，不超过 [`MAX_PARAMETERS`] 个参数。
fn insert_bound(
    executor: &dyn SqliteExecutor,
    head: &str,
    width: usize,
    rows: &[Vec<Cell>],
    conflict: &str,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let row = format!("({})", vec!["?"; width].join(", "));
    let mut size = 1usize;
    while size * 2 * width <= MAX_PARAMETERS {
        size *= 2;
    }
    let mut start = 0usize;
    while start < rows.len() {
        while size > rows.len() - start {
            size /= 2;
        }
        let chunk = &rows[start..start + size];
        let sql = format!("{head} {} {conflict}", vec![row.as_str(); size].join(", "));
        let params: Vec<Cell> = chunk.iter().flatten().cloned().collect();
        executor.run(&sql, &params)?;
        start += size;
    }
    Ok(())
}

/// 一个单元值的 JSON 表示（`json_each` 路径只走这三类）。
fn cell_json(cell: &Cell) -> serde_json::Value {
    match cell {
        Cell::Null => serde_json::Value::Null,
        Cell::Integer(value) => (*value).into(),
        Cell::Real(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Cell::Text(value) => serde_json::Value::String(value.clone()),
        // Blob 不该出现在小行里（`insert_rows` 已经把它归到绑定路径）。
        Cell::Blob(value) => serde_json::Value::String(format!("<blob:{}>", value.len())),
    }
}

/// JSON 值（已编码的字符串）按 [`MAX_ARRAY_CHARACTERS`] 切成若干数组文本。上游 `jsonArrays`。
fn json_arrays(parts: Vec<String>) -> Result<Vec<String>> {
    let mut arrays = Vec::new();
    let mut chunk: Vec<String> = Vec::new();
    // 数组文本 = 两个方括号 + 各段 + 段间逗号。
    let mut length = 2usize;
    for part in parts {
        let part_length = part.encode_utf16().count();
        if !chunk.is_empty() && length + 1 + part_length > MAX_ARRAY_CHARACTERS {
            arrays.push(format!("[{}]", chunk.join(",")));
            chunk.clear();
            length = 2;
        }
        length += (if chunk.is_empty() { 0 } else { 1 }) + part_length;
        chunk.push(part);
    }
    if !chunk.is_empty() {
        arrays.push(format!("[{}]", chunk.join(",")));
    }
    Ok(arrays)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::migrations::migrations;
    use crate::storage::sqlite::schema::SqliteSchema;
    use nous_chord::sqlite::{apply_sqlite_migrations, LocalDatabase};

    /// 数语句条数的执行器：证明「行多、语句少」。
    struct Counting<'a> {
        inner: &'a dyn SqliteExecutor,
        counts: std::cell::Cell<usize>,
    }

    impl SqliteExecutor for Counting<'_> {
        fn exec(&self, sql: &str) -> std::result::Result<(), nous_chord::sqlite::SqliteError> {
            self.counts.set(self.counts.get() + 1);
            self.inner.exec(sql)
        }
        fn run(&self, sql: &str, params: &[Cell]) -> std::result::Result<(), nous_chord::sqlite::SqliteError> {
            self.counts.set(self.counts.get() + 1);
            self.inner.run(sql, params)
        }
        fn run_counted(&self, sql: &str, params: &[Cell]) -> std::result::Result<u64, nous_chord::sqlite::SqliteError> {
            self.counts.set(self.counts.get() + 1);
            self.inner.run_counted(sql, params)
        }
        fn get(&self, sql: &str, params: &[Cell]) -> std::result::Result<Option<SqliteRow>, nous_chord::sqlite::SqliteError> {
            self.counts.set(self.counts.get() + 1);
            self.inner.get(sql, params)
        }
        fn all(&self, sql: &str, params: &[Cell]) -> std::result::Result<Vec<SqliteRow>, nous_chord::sqlite::SqliteError> {
            self.counts.set(self.counts.get() + 1);
            self.inner.all(sql, params)
        }
    }

    fn fresh() -> LocalDatabase {
        let schema = SqliteSchema::unprefixed();
        let mut db = LocalDatabase::open_in_memory().unwrap();
        apply_sqlite_migrations(&mut db, &migrations(&schema), schema.table("durable_schema")).unwrap();
        db
    }

    #[test]
    fn insert_rows_counts_more_rows_than_statements() {
        let db = fresh();
        let counter = Counting { inner: &db, counts: std::cell::Cell::new(0) };
        let rows: Vec<Vec<Cell>> = (1..=500)
            .map(|id| vec![Cell::Integer(id), Cell::Text("entry".to_string())])
            .collect();
        insert_rows(&counter, "record_ids", &["id", "record_type"], &rows, "").unwrap();
        let statements = counter.counts.get();
        assert_eq!(statements, 1, "500 行应该是一条语句（json_each），实际 {statements}");
        let count = db.get("SELECT COUNT(*) AS n FROM record_ids", &[]).unwrap().unwrap();
        assert_eq!(count.get("n").unwrap().as_i64(), Some(500));

        // 大行（> 8 KB）走绑定路径：仍然远少于行数。
        db.exec("CREATE TABLE big (id INTEGER PRIMARY KEY, data TEXT NOT NULL) STRICT").unwrap();
        let large: Vec<Vec<Cell>> = (1..=20)
            .map(|id| vec![Cell::Integer(id), Cell::Text("x".repeat(9_000))])
            .collect();
        let counter = Counting { inner: &db, counts: std::cell::Cell::new(0) };
        insert_rows(&counter, "big", &["id", "data"], &large, "").unwrap();
        assert!(counter.counts.get() <= 2, "20 个大行最多两条语句，实际 {}", counter.counts.get());
        let count = db.get("SELECT COUNT(*) AS n FROM big", &[]).unwrap().unwrap();
        assert_eq!(count.get("n").unwrap().as_i64(), Some(20));
    }

    #[test]
    fn read_rows_binds_one_json_array_per_chunk() {
        let db = fresh();
        insert_rows(
            &db,
            "record_ids",
            &["id", "record_type"],
            &(1..=5).map(|id| vec![Cell::Integer(id), Cell::Text("entry".into())]).collect::<Vec<_>>(),
            "",
        )
        .unwrap();
        let rows = read_rows(
            &db,
            "SELECT id FROM record_ids WHERE id IN (SELECT value FROM json_each(?)) ORDER BY id",
            &[Cell::Integer(1), Cell::Integer(3), Cell::Integer(5)],
        )
        .unwrap();
        assert_eq!(rows.len(), 3);

        // 空值列表 → 一条语句都不发（`json_arrays` 返回空）。
        let none = read_rows(&db, "SELECT id FROM record_ids WHERE id IN (SELECT value FROM json_each(?))", &[])
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn an_upsert_conflict_clause_keeps_the_reader_unambiguous() {
        let db = fresh();
        let rows = vec![vec![Cell::Integer(1), Cell::Text("conversation".into())]];
        insert_rows(&db, "record_ids", &["id", "record_type"], &rows, "").unwrap();
        insert_rows(
            &db,
            "record_ids",
            &["id", "record_type"],
            &rows,
            "ON CONFLICT (id) DO UPDATE SET record_type = excluded.record_type",
        )
        .unwrap();
        let count = db.get("SELECT COUNT(*) AS n FROM record_ids", &[]).unwrap().unwrap();
        assert_eq!(count.get("n").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn blobs_never_travel_through_the_json_array_path() {
        let db = fresh();
        let rows = vec![vec![Cell::Integer(1), Cell::Blob(vec![0, 1, 2])]];
        // `record_type` 是 TEXT 列，blob 会因为类型不符被拒 —— 我们要的是「不 panic、明确失败」。
        assert!(insert_rows(&db, "record_ids", &["id", "record_type"], &rows, "").is_err());
        // 换成真正的 BLOB 列则能写进去。
        db.exec("CREATE TABLE b (id INTEGER PRIMARY KEY, data BLOB) STRICT").unwrap();
        insert_rows(&db, "b", &["id", "data"], &rows, "").unwrap();
        let row = db.get("SELECT data FROM b WHERE id = 1", &[]).unwrap().unwrap();
        assert_eq!(row.get("data").unwrap().as_blob(), Some(&[0u8, 1, 2][..]));
    }
}
