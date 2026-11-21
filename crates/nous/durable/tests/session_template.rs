//! #72：会话 template 落进 session 作用域单例文档，重启后仍读得出。

use nous_durable::session::Session;
use nous_durable::Storage;
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("nous-template-{tag}-{}.sqlite", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

#[tokio::test]
async fn a_created_session_remembers_its_template_after_reopen() {
    let path = temp_db("remember");
    let session = Session::open(&path).unwrap();
    let created: Value = session.create_conversation(json!({
        "name": "tpl-session", "template": "alpha"
    })).await.unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(nous_durable::session::commands::session_template(&session, id).await.unwrap(), Some("alpha".to_string()));
    session.close().await.unwrap();

    // 只读重开：值在库里，不在进程内存里。
    let storage = Storage::open_read_only(&path).unwrap();
    let rows = storage.scan_entries(&nous_durable::EntryQuery { conversation_id: id, min_entry_id: None, max_entry_id: None, order: None }, 10, None).unwrap().unwrap();
    assert_eq!(rows.items.len(), 0, "template 不该写成 entry");
    let imported = nous_durable::storage::import::ImportedStore::read(&path).unwrap();
    let items = nous_durable::storage::sessions::render_sessions(&imported).unwrap();
    assert_eq!(items[0].template.as_deref(), Some("alpha"));
}
