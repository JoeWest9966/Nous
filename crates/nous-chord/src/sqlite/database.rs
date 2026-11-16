//! 可移植的 SQLite 门面。上游 `packages/chord/src/sqlite/database.ts`（41 行）。
//!
//! ## 与上游的两处形状差别（都不改语义）
//!
//! 1. **同步**。上游全是 `Promise`（允许适配器把活干到别的线程）。Rust 侧 `rusqlite` 是同步的，
//!    包一层 async 只会把「阻塞」藏起来；调用方要挪线程就 `spawn_blocking`，意图写在调用点。
//! 2. **行是 [`SqliteRow`]（键 → 值）**。上游是 `get<T extends object>` 的泛型反序列化；
//!    Rust 里由调用方按列名取值再转换 —— 少一层宏，也不会把「列名拼错」推迟到运行期才发现
//!    （`SqliteRow::get_*` 返回 `Option`，配合 `ok_or` 立刻报错）。

use std::collections::BTreeMap;

use super::error::SqliteError;

/// 可绑定 / 可读出的值。上游 `SqliteValue`（`null | number | bigint | string | Uint8Array`）。
#[derive(Debug, Clone, PartialEq)]
pub enum SqliteValue {
    Null,
    /// 整数。上游的 `number` 与 `bigint` 在这里合并 —— SQLite 的 `INTEGER` 就是 i64。
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqliteValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            SqliteValue::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            SqliteValue::Real(value) => Some(*value),
            SqliteValue::Integer(value) => Some(*value as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            SqliteValue::Text(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            SqliteValue::Blob(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, SqliteValue::Null)
    }
}

impl From<i64> for SqliteValue {
    fn from(value: i64) -> Self {
        SqliteValue::Integer(value)
    }
}

impl From<u32> for SqliteValue {
    fn from(value: u32) -> Self {
        SqliteValue::Integer(i64::from(value))
    }
}

impl From<f64> for SqliteValue {
    fn from(value: f64) -> Self {
        SqliteValue::Real(value)
    }
}

impl From<&str> for SqliteValue {
    fn from(value: &str) -> Self {
        SqliteValue::Text(value.to_string())
    }
}

impl From<String> for SqliteValue {
    fn from(value: String) -> Self {
        SqliteValue::Text(value)
    }
}

impl From<Vec<u8>> for SqliteValue {
    fn from(value: Vec<u8>) -> Self {
        SqliteValue::Blob(value)
    }
}

impl<T: Into<SqliteValue>> From<Option<T>> for SqliteValue {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => SqliteValue::Null,
        }
    }
}

/// 一行结果：列名 → 值。列序不保证，按名取（`BTreeMap::get`）。
pub type SqliteRow = BTreeMap<String, SqliteValue>;

/// 数据库与事务句柄共用的操作面。上游 `SqliteExecutor`。
///
/// `run_counted` 对应上游同名方法：返回 `changes()` 数过的行数（索引写、触发器改的行不算），
/// 可能比 `run` 贵 —— **不需要计数就用 `run`**。
pub trait SqliteExecutor {
    /// 执行一段可以有多条语句的 SQL（无绑定）。
    fn exec(&self, sql: &str) -> Result<(), SqliteError>;

    /// 执行一条语句，带位置绑定，不要返回值。
    fn run(&self, sql: &str, params: &[SqliteValue]) -> Result<(), SqliteError>;

    /// 执行一条语句，返回它影响的行数。
    fn run_counted(&self, sql: &str, params: &[SqliteValue]) -> Result<u64, SqliteError>;

    /// 执行一条语句，取第一行。
    fn get(&self, sql: &str, params: &[SqliteValue]) -> Result<Option<SqliteRow>, SqliteError>;

    /// 执行一条语句，取全部行。
    fn all(&self, sql: &str, params: &[SqliteValue]) -> Result<Vec<SqliteRow>, SqliteError>;
}

/// 数据库门面。上游 `SqliteDatabase`。
pub trait SqliteDatabase: SqliteExecutor {
    /// 在一个事务里跑完 `work`：成功提交，失败**回滚**并返回同一个错误。
    ///
    /// 事务里所有操作必须走传进去的句柄（`&dyn SqliteExecutor`）；句柄出了这个函数就作废。
    /// 事务进行中再调数据库本身（包括 `transaction`）会自锁 —— 上游同一约束。
    fn transaction<T, F>(&mut self, work: F) -> Result<T, SqliteError>
    where
        F: FnOnce(&dyn SqliteExecutor) -> Result<T, SqliteError>;

    /// 关库。事务未结束的库不许关。
    fn close(&mut self) -> Result<(), SqliteError>;
}
