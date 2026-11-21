//! #89：presets 的 SQL 归位——server 不再直接开库，回执 / 修订全走 Store 的 API。
//!
//! 判据守住三件事：回执幂等（同 requestId 不涨版本）、修订从 1 连续、文件库与
//! `nous.db` 的既有表（triggers/fires）互不干扰。

use std::path::PathBuf;

use nous_store::Store;

fn db(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(format!("nous-{tag}.db"));
    (dir, path)
}

#[test]
fn preset_receipts_are_idempotent_and_revisions_grow_one_by_one() {
    let (_dir, path) = db("presets-api");
    let store = Store::open(&path).expect("open");

    assert_eq!(store.preset_receipt("Session-Template", "alpha", "put", "req-1").expect("receipt"), None);
    let first = store
        .append_preset_revision("Session-Template", "alpha", b"{\"name\":\"alpha\"}\n", "local", 1000, "sha1", "req-1")
        .expect("append");
    assert_eq!(first, 1);
    store
        .remember_preset_write("Session-Template", "alpha", "put", "req-1", r#"{"version":1}"#)
        .expect("remember");

    let receipt = store
        .preset_receipt("Session-Template", "alpha", "put", "req-1")
        .expect("receipt")
        .expect("receipt exists");
    assert_eq!(receipt, r#"{"version":1}"#);

    // 同一 requestId 重放：回执不覆盖，版本也不涨（server 靠这个返回第一次的回执）。
    store
        .remember_preset_write("Session-Template", "alpha", "put", "req-1", r#"{"version":99}"#)
        .expect("remember replay");
    assert_eq!(
        store.preset_receipt("Session-Template", "alpha", "put", "req-1").expect("receipt"),
        Some(r#"{"version":1}"#.to_string())
    );

    let second = store
        .append_preset_revision("Session-Template", "alpha", b"{\"name\":\"alpha\",\"v\":2}\n", "local", 2000, "sha2", "req-2")
        .expect("append");
    assert_eq!(second, 2);
    let revisions = store.preset_revisions("Session-Template", "alpha").expect("revisions");
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].n, 1);
    assert_eq!(revisions[1].n, 2);
    assert_eq!(revisions[1].request_id, "req-2");
    assert_eq!(revisions[0].body, "{\"name\":\"alpha\"}\n");
}

#[test]
fn preset_revisions_are_scoped_by_section_and_name() {
    let (_dir, path) = db("presets-scope");
    let store = Store::open(&path).expect("open");
    store.append_preset_revision("Session-Template", "a", b"a", "local", 1, "s1", "r1").expect("append");
    store.append_preset_revision("Session-Template", "b", b"b", "local", 1, "s2", "r2").expect("append");
    store.append_preset_revision("Ultimate-Template", "a", b"u", "local", 1, "s3", "r3").expect("append");

    let a = store.preset_revisions("Session-Template", "a").expect("revisions");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].body, "a");
    let u = store.preset_revisions("Ultimate-Template", "a").expect("revisions");
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].body, "u");
}
