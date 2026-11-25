//! #60/#89：模板引擎内修订与写回执。
//!
//! 盘上的 `preset.json` 是内容真相；这里只记「每次写 happened 了什么」：
//! 原文、sha、来路与 requestId。`UNIQUE(section, name, n)` 保证第 n 版只有一份，
//! `request_id` 是幂等锚：同一 requestId 重放回第一次的回执，版本不涨。
//!
//! #89 把这一组 SQL 从 `nous-server` 搬进来：**数据面入口只有 store**，
//! app 层不再直接 `use rusqlite` / `Connection::open`（verify.sh §1.1 / §1.5）。

use rusqlite::{params, OptionalExtension};

use crate::{Result, Store};

/// 一条修订：给 GET /revisions 与 rollback 读回原文。
#[derive(Debug, Clone)]
pub struct PresetRevision {
    pub n: i64,
    pub sha: String,
    pub body: String,
    pub actor: String,
    pub at: i64,
    pub request_id: String,
}

impl Store {
    /// 读回执的正文（JSON 字符串），同一 requestId 重放返回第一次记下的原文。
    pub fn preset_receipt(
        &self,
        section: &str,
        name: &str,
        kind: &str,
        request_id: &str,
    ) -> Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT body FROM preset_write_receipts
                 WHERE section=?1 AND name=?2 AND kind=?3 AND request_id=?4",
                params![section, name, kind, request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// 记一次写回执。`INSERT OR IGNORE`：重放不覆盖第一次的结果。
    pub fn remember_preset_write(
        &self,
        section: &str,
        name: &str,
        kind: &str,
        request_id: &str,
        body: &str,
    ) -> Result<()> {
        self.conn()
            .execute(
                "INSERT OR IGNORE INTO preset_write_receipts
                   (section, name, kind, request_id, body) VALUES (?1,?2,?3,?4,?5)",
                params![section, name, kind, request_id, body],
            )?;
        Ok(())
    }

    /// 这个 preset 的全部修订，按 n 升序。
    pub fn preset_revisions(&self, section: &str, name: &str) -> Result<Vec<PresetRevision>> {
        let mut stmt = self.conn().prepare(
            "SELECT n, sha, body, actor, at, request_id
             FROM presets_revisions WHERE section=?1 AND name=?2 ORDER BY n",
        )?;
        let rows = stmt.query_map(params![section, name], |row| {
            Ok(PresetRevision {
                n: row.get(0)?,
                sha: row.get(1)?,
                body: row.get(2)?,
                actor: row.get(3)?,
                at: row.get(4)?,
                request_id: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// 追加一条修订：n = 当前最大 n + 1。事务与原来 server 里的写法一致——
    /// 算号 + 落行在一个事务里，原子。调用方负责算 sha 与取时间（store 零直接时钟）。
    pub fn append_preset_revision(
        &self,
        section: &str,
        name: &str,
        body: &[u8],
        actor: &str,
        at_ms: i64,
        sha: &str,
        request_id: &str,
    ) -> Result<i64> {
        let tx = self.conn().unchecked_transaction()?;
        let n: i64 = tx.query_row(
            "SELECT COALESCE(MAX(n), 0) + 1 FROM presets_revisions WHERE section=?1 AND name=?2",
            params![section, name],
            |row| row.get(0),
        )?;
        let text = String::from_utf8_lossy(body).into_owned();
        tx.execute(
            "INSERT INTO presets_revisions (section,name,n,sha,body,actor,at,request_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![section, name, n, sha, text, actor, at_ms, request_id],
        )?;
        tx.commit()?;
        Ok(n)
    }
}

/// `GET /v1/stats/db` 的插件库体检：只跑只读 PRAGMA，不另开连接、不写库。
/// #89 收口：`PRAGMA` 属于数据面，app 层不许直接拼这条 SQL。
pub fn quick_check(store: &Store) -> Result<String> {
    store
        .conn()
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(Into::into)
}

/// 给只读报告用的关键表计数（表名由调用方给，都是编译期常量）。
pub fn table_count(store: &Store, table: &str) -> Result<i64> {
    store
        .conn()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_receipt_body_stays_raw_json_text() {
        let store = Store::open_in_memory().expect("open");
        let body = json!({"version": 3}).to_string();
        store
            .remember_preset_write("S", "n", "put", "r", &body)
            .expect("remember");
        assert_eq!(store.preset_receipt("S", "n", "put", "r").expect("receipt"), Some(body));
    }
}
