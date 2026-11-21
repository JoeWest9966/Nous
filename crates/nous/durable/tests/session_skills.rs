//! #83：会话选中的 skills 落在 conversation 作用域的 pi.agent 文档里。

use nous_durable::session::Session;
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("nous-skill-config-{tag}-{}-{:?}.sqlite", std::process::id(), std::thread::current().id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

#[tokio::test]
async fn created_and_configured_skills_are_read_back_from_the_agent_document() {
    let session = Session::open(&temp_db("read-back")).unwrap();
    let created: Value = session.create_conversation(json!({
        "name": "", "skills": ["alpha", "beta"]
    })).await.unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(
        nous_durable::session::commands::session_skills(&session, id).await.unwrap(),
        vec!["alpha".to_string(), "beta".to_string()],
        "建会话选的技能要读得回来"
    );

    // 配置是相对增删：删 beta 加 gamma，alpha 原位不动。
    let result = nous_durable::session::commands::configure_session(
        &session, id, json!({"skills": {"add": ["gamma"], "remove": ["beta"]}}),
    ).await.unwrap();
    assert_eq!(result.value["skills"], json!(["alpha", "gamma"]), "配置回包是生效顺序");
    assert_eq!(
        nous_durable::session::commands::session_skills(&session, id).await.unwrap(),
        vec!["alpha".to_string(), "gamma".to_string()],
        "pi.agent 文档要记住选中顺序"
    );
    session.close().await.unwrap();
}
