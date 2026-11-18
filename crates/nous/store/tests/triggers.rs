//! 判据 5：`code` 不可变 + 删除只留墓碑（code 永远被占着）。
//!
//! 这三条都由**库自己**拒绝（SQL 触发器），不是「我们的 API 没提供这个方法」——
//! 后者对 `conn()` 拿到裸连接的人毫无约束。

use nous_store::{NewTrigger, Store, StoreError};

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("nous.db")).expect("open");
    (dir, store)
}

fn new(code: &str) -> NewTrigger<'_> {
    NewTrigger { code, name: "夜报", spec: r#"{"cron":"0 3 * * *"}"# }
}

#[test]
fn code_is_immutable_even_in_raw_sql() {
    let (_dir, store) = open();
    store.create_trigger(new("nightly_digest"), 1_000).expect("create");

    let error = store
        .conn()
        .execute("UPDATE triggers SET code = 'other_code' WHERE code = 'nightly_digest'", [])
        .expect_err("改 code 必须被拒");
    assert!(
        error.to_string().contains("不可变"),
        "拒绝的理由要指名 code（requestId / actor.id / 审计都挂着它）：{error}"
    );
    assert!(store.trigger("nightly_digest").expect("read").is_some());
    assert!(store.trigger("other_code").expect("read").is_none());
}

#[test]
fn delete_is_a_tombstone_and_the_code_stays_occupied() {
    let (_dir, store) = open();
    let created = store.create_trigger(new("nightly_digest"), 1_000).expect("create");
    store.set_schedule("nightly_digest", true, Some(5_000)).expect("schedule");

    store.delete_trigger("nightly_digest", 9_000).expect("delete");

    let row = store.trigger("nightly_digest").expect("read").expect("行必须还在");
    assert_eq!(row.id, created.id);
    assert_eq!(row.deleted_at, Some(9_000));
    assert_eq!(row.next_fire_at, None, "删掉的触发器不再有下一次");
    assert!(!row.enabled);
    assert!(row.is_deleted());

    // 同 code 再建 → 被拒（墓碑占着这个字符串）。
    match store.create_trigger(new("nightly_digest"), 9_500) {
        Err(StoreError::CodeTaken { code, deleted_at }) => {
            assert_eq!(code, "nightly_digest");
            assert_eq!(deleted_at, Some(9_000), "要说明「它已经被删了，但仍然占着」");
        }
        other => panic!("墓碑的 code 不许复用，实得 {other:?}"),
    }

    // 再删一次不动墓碑时间（历史是历史）。
    store.delete_trigger("nightly_digest", 20_000).expect("delete again");
    assert_eq!(
        store.trigger("nightly_digest").expect("read").expect("行").deleted_at,
        Some(9_000)
    );

    // 真 DELETE 被库拒绝：行还在，code 还在。
    assert!(
        store
            .conn()
            .execute("DELETE FROM triggers WHERE code = 'nightly_digest'", [])
            .is_err(),
        "真 DELETE 必须被拒 —— 墓碑表不许撕"
    );
    assert_eq!(store.triggers().expect("list").len(), 1);
}

#[test]
fn code_format_is_enforced_on_create() {
    let (_dir, store) = open();

    for code in ["abc", "nightly_digest", "a1_b", "a01", &"a".repeat(64), &format!("a{}", "0".repeat(63))] {
        store
            .create_trigger(new(code), 1_000)
            .unwrap_or_else(|error| panic!("{code:?} 应该合法：{error}"));
    }

    for code in ["", "ab", "Abc", "1abc", "_abc", "abc-def", "abc def", "AB1", &format!("a{}", "b".repeat(64))] {
        match store.create_trigger(new(code), 1_000) {
            Err(StoreError::InvalidCode { .. }) => {}
            other => panic!("{code:?} 必须被判 invalid，实得 {other:?}"),
        }
    }
}

#[test]
fn rename_changes_only_the_name() {
    let (_dir, store) = open();
    let created = store.create_trigger(new("nightly_digest"), 1_000).expect("create");

    store.rename_trigger("nightly_digest", "夜报（改）").expect("rename");

    let row = store.trigger("nightly_digest").expect("read").expect("行");
    assert_eq!(row.name, "夜报（改）");
    assert_eq!(row.code, created.code, "改名不动 code");
    assert_eq!(row.id, created.id);
    assert_eq!(row.created_at, created.created_at);
    assert_eq!(row.spec, created.spec);
}

#[test]
fn a_duplicate_live_code_is_refused() {
    let (_dir, store) = open();
    store.create_trigger(new("nightly_digest"), 1_000).expect("create");
    match store.create_trigger(new("nightly_digest"), 2_000) {
        Err(StoreError::CodeTaken { deleted_at, .. }) => assert_eq!(deleted_at, None),
        other => panic!("重名必须被拒，实得 {other:?}"),
    }
    assert_eq!(store.triggers().expect("list").len(), 1);
}

#[test]
fn spec_is_stored_verbatim() {
    let (_dir, store) = open();
    let spec = "{\n  \"cron\": \"0 3 * * *\",\n  \"tz\": \"Asia/Shanghai\",\n  \"备注\": \"原样存，本票不解释它\"\n}";
    store
        .create_trigger(NewTrigger { code: "nightly_digest", name: "夜报", spec }, 1_000)
        .expect("create");
    assert_eq!(store.trigger("nightly_digest").expect("read").expect("行").spec, spec);
}

#[test]
fn an_unknown_code_is_named_not_silently_ignored() {
    let (_dir, store) = open();
    for outcome in [
        store.rename_trigger("ghost", "x").map(|_| ()),
        store.set_schedule("ghost", true, Some(1)).map(|_| ()),
        store.delete_trigger("ghost", 1_000).map(|_| ()),
    ] {
        match outcome {
            Err(StoreError::CodeUnknown(code)) => assert_eq!(code, "ghost"),
            other => panic!("不存在的 code 必须明确报错，实得 {other:?}"),
        }
    }
    assert!(store.triggers().expect("list").is_empty());
}
