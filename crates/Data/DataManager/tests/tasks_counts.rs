mod common;

use std::collections::BTreeSet;

use common::Db;
use serde_json::json;

#[test]
fn by_state_running_equals_a_direct_sql_count_on_the_same_db() {
    let db = Db::new("tasks");
    for (i, status) in [
        "running", "running", "pending", "waiting", "sleeping", "completing", "terminal",
    ].iter().enumerate() {
        db.task(i as i64 + 1, 10, status);
    }

    // 测试直接开同一个库：SELECT count(*) ... status='running'（不靠 stats 自证）。
    let conn = rusqlite::Connection::open_with_flags(
        &db.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).unwrap();
    let direct: i64 = conn.query_row(
        "SELECT count(*) FROM tasks WHERE status = 'running'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(direct, 2);

    let report = data_data_manager::tasks(&db.path).unwrap();
    let by_state = &report["byState"];
    let states: BTreeSet<&str> = by_state.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(states, BTreeSet::from([
        "pending", "running", "waiting", "sleeping", "completing", "terminal",
    ]), "六态键全集固定");
    assert_eq!(by_state["running"], json!(direct), "running 与直连 SQL 相等");
    assert_eq!(report["active"], json!(6), "active = 非 terminal 之和");
    std::fs::remove_file(&db.path).ok();
}

#[test]
fn empty_db_reports_all_zero_states_and_no_active_tasks() {
    let db = Db::new("tasks-empty");
    let report = data_data_manager::tasks(&db.path).unwrap();
    assert_eq!(report["active"], json!(0));
    for state in ["pending", "running", "waiting", "sleeping", "completing", "terminal"] {
        assert_eq!(report["byState"][state], json!(0), "{state} 空库为 0");
    }
    std::fs::remove_file(&db.path).ok();
}
