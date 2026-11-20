//! 泛型文档服务：路由 handler 只认这里的方法，不含任何 `if kind == ...`。
//!
//! `DocumentWriter` 是 documents 对写端的全部需求，serve 侧的实现必须把它接到
//! `nous_durable::session::commands::write_document`（同一写锁 / 同事务 / 同广播）。

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::store::DocumentReader;
use crate::{DocumentKind, ValidationCtx, ValidationReport};

/// 写端接缝。产品词只出现在 `kind` / `section` / `name` 字符串里，不在 trait 名里。
#[async_trait]
pub trait DocumentWriter: Send + Sync {
    /// 建 / 写一份文档。`etag` 是乐观锁（当前实现每次整份 base 覆盖）。
    async fn write(
        &self,
        kind: &str,
        section: &str,
        name: &str,
        fields: Value,
        etag: Option<&str>,
    ) -> Result<String, String>;

    /// 改名字（级联引用由实现统一处理；返回连带改到的名字）。
    async fn rename(&self, kind: &str, section: &str, name: &str, to: &str) -> Result<Vec<String>, String>;

    /// 通用动作（publish / reload / archive / restore / purge / category / order …）。
    /// documents 不解释动作语义，只把它记在文档字段的 `action` 上并经写端落库。
    async fn action(
        &self,
        kind: &str,
        section: &str,
        name: &str,
        action: &str,
        payload: Value,
    ) -> Result<(), String>;
}

/// 一个 kind 的全部行为（列表 / 读 / 校验 / 写 / 历史 / 差异 / pending）。
#[derive(Clone)]
pub struct DocumentService {
    reader: Arc<dyn DocumentReader>,
    writer: Option<Arc<dyn DocumentWriter>>,
    /// 文档根的真实绝对路径。只用于回报 `dir`，不读盘、不 spawn git。
    root: String,
}

impl DocumentService {
    pub fn new(reader: Arc<dyn DocumentReader>, writer: Option<Arc<dyn DocumentWriter>>) -> Self {
        Self {
            reader,
            writer,
            root: std::env::current_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "/".to_string()),
        }
    }

    /// 显式指定文档根（生产由装配方给出）；`dir` 回报的是这个根下的 kind/section。
    pub fn with_root(mut self, root: impl Into<String>) -> Self {
        self.root = root.into();
        self
    }

    /// 列表：`{ kind, section, items[], resolved }`。
    pub fn list<K: DocumentKind>(&self, section: &str) -> Result<Value, String> {
        let names = self.reader.names(K::KIND, section)?;
        let dir = format!("{}/{}/{}", self.root.trim_end_matches('/'), K::KIND, section);
        let mut items = Vec::new();
        for name in &names {
            if let Some((fields, etag)) = self.reader.get(K::KIND, section, name)? {
                // 形状校验：typed 反序列化失败直接报字段；成功但 validate 有 error 就报
                // 第一条 error 的 path（与 `validate` 端点的口径一致）。
                let spec_error = match K::from_value(fields.clone()) {
                    Err(error) => Some(error),
                    Ok(typed) => {
                        let ctx = ValidationCtx { names: names.iter().cloned().collect() };
                        ValidationReport::of::<K>(&typed, &ctx)
                            .errors
                            .first()
                            .map(|issue| format!("{}: {}", issue.path, issue.message))
                    }
                };
                items.push(json!({
                    "name": name, "kind": K::KIND, "section": section,
                    "fields": fields, "etag": etag,
                    // 不 spawn git：无从判「仓里有没有」，诚实回 null，不拿 true 冒充。
                    "tracked": Value::Null, "dir": dir, "specError": spec_error,
                }));
            }
        }
        Ok(json!({
            "kind": K::KIND, "section": section, "items": items,
            "resolved": { "references": {} },
        }))
    }

    /// 读一份：`{ kind, section, name, fields, etag }`。
    pub fn get<K: DocumentKind>(&self, section: &str, name: &str) -> Result<Option<Value>, String> {
        let Some((fields, etag)) = self.reader.get(K::KIND, section, name)? else {
            return Ok(None);
        };
        Ok(Some(json!({
            "kind": K::KIND, "section": section, "name": name,
            "fields": fields, "etag": etag,
        })))
    }

    /// 校验：`{ ok, errors[], warnings[], resolved }`，形状见 `ValidationReport`。
    pub fn validate<K: DocumentKind>(&self, section: &str, name: &str) -> Result<Value, String> {
        let names: BTreeSet<String> = self.reader.names(K::KIND, section)?;
        let ctx = ValidationCtx { names };
        let value = match self.reader.get(K::KIND, section, name)? {
            Some((fields, _)) => {
                let typed = K::from_value(fields).map_err(|error| format!("stored fields: {error}"))?;
                serde_json::to_value(ValidationReport::of::<K>(&typed, &ctx)).unwrap()
            }
            None => {
                // 不存在的名字：给一条明确的错误，不伪造字段结构（`resolved` 仍按形状给空对象）。
                serde_json::to_value(ValidationReport {
                    ok: false,
                    errors: vec![crate::Issue::error("name", format!("document {name} not found"))],
                    warnings: vec![],
                    resolved: json!({ "references": {} }),
                }).unwrap()
            }
        };
        Ok(value)
    }

    /// 历史：`{ kind, section, name, commits[] }`。`hash` 是 revision 的 seq（不是 git sha）。
    pub fn history<K: DocumentKind>(&self, section: &str, name: &str) -> Result<Value, String> {
        let revisions = self.reader.revisions(K::KIND, section, name)?;
        let commits: Vec<Value> = revisions.into_iter().rev().enumerate().map(|(i, (seq, kind))| {
            let hash = format!("r{seq}");
            json!({
                "hash": hash, "short": hash, "at": null, "author": null,
                "message": format!("revision {seq} ({kind})"), "index": i,
            })
        }).collect();
        Ok(json!({ "kind": K::KIND, "section": section, "name": name, "commits": commits }))
    }

    /// 差异：`{ kind, section, name, path, before, after }`（全文，不是 git patch）。
    pub fn diff<K: DocumentKind>(
        &self,
        section: &str,
        name: &str,
        hash: Option<&str>,
        pending: bool,
    ) -> Result<Option<Value>, String> {
        let revisions = self.reader.revisions(K::KIND, section, name)?;
        if revisions.is_empty() {
            return Ok(None);
        }
        let before = if pending {
            // pending = 最新 base 之后叠的 delta；没有 delta 就两边相等。
            revisions.iter().rposition(|(_, kind)| kind == "base").and_then(|idx| revisions.get(idx))
                .map(|(seq, _)| *seq)
        } else {
            let wanted = hash.and_then(|h| h.strip_prefix('r')).and_then(|h| h.parse::<i64>().ok());
            let Some(wanted) = wanted else { return Ok(None) };
            revisions.iter().position(|(seq, _)| *seq == wanted).and_then(|idx| idx.checked_sub(1))
                .and_then(|idx| revisions.get(idx)).map(|(seq, _)| *seq)
        };
        let Some(before_seq) = before else { return Ok(None) };
        let before_value = self.reader.at(K::KIND, section, name, before_seq)?;
        let after_value = self.reader.at(K::KIND, section, name, revisions.last().unwrap().0)?;
        Ok(Some(json!({
            "kind": K::KIND, "section": section, "name": name,
            "path": name, "before": before_value, "after": after_value,
        })))
    }

    /// pending：`{ kind, section, head, dirty, units[] }`。
    pub fn pending<K: DocumentKind>(&self, section: &str, name: &str) -> Result<Value, String> {
        let revisions = self.reader.revisions(K::KIND, section, name)?;
        let last_base = revisions.iter().rposition(|(_, kind)| kind == "base")
            .map(|idx| revisions[idx].0);
        let dirty = last_base.is_some_and(|base| revisions.last().is_some_and(|(seq, _)| *seq > base));
        let units = if dirty {
            vec![json!({
                "key": name, "kind": K::KIND, "name": name, "tracked": true,
                "files": [], "added": 0, "removed": 0, "code": false, "hasDeleted": false,
            })]
        } else {
            vec![]
        };
        Ok(json!({
            "kind": K::KIND, "section": section, "name": name,
            "head": null, "dirty": dirty, "ahead": 0, "behind": 0,
            "remote": null, "units": units, "aheadCommits": [],
        }))
    }

    pub fn writer(&self) -> Option<Arc<dyn DocumentWriter>> {
        self.writer.clone()
    }
}
