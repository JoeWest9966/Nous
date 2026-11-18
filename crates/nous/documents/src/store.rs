//! 只读文档存储接缝。真正的 SQL 读在 server 侧已有的只读源上做；
//! 这里只声明 documents 需要的接口，不绑定第二个可写 `Storage`。

use std::collections::BTreeSet;

use serde_json::Value;

use crate::DocumentKind;

/// 一条已存文档的记录（读侧投影）。
#[derive(Debug, Clone, PartialEq)]
pub struct Stored<K: DocumentKind> {
    pub name: String,
    pub fields: K,
    pub etag: String,
}

/// documents 对读端的全部需求。server 用已有的只读连接实现它。
pub trait DocumentReader: Send + Sync {
    /// 某 kind + section 下的全部文档名。
    fn names(&self, kind: &'static str, section: &str) -> Result<BTreeSet<String>, String>;

    /// 读一份文档的 typed 字段 + etag；不存在给 `None`。
    fn get(&self, kind: &'static str, section: &str, name: &str) -> Result<Option<(Value, String)>, String>;

    /// 一份文档的修订序列（`seq` 升序），每项带 `(seq, "base" | "delta")`。
    fn revisions(&self, kind: &'static str, section: &str, name: &str)
        -> Result<Vec<(i64, String)>, String>;

    /// 某个 seq 上的全文值；没有就是 `None`。
    fn at(&self, kind: &'static str, section: &str, name: &str, seq: i64) -> Result<Option<Value>, String>;
}
