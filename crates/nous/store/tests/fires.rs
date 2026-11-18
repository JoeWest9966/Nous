//! 判据 3（唯一键真去重）+ 判据 4（`reclaim` 只捞该捞的，穷举）。

use nous_store::fires::Action::{MarkOrphaned, Replay};
use nous_store::{
    reclaim, request_id, Fire, FireState, Inserted, NewTrigger, Store, GRACE_MS,
};

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("nous.db")).expect("open");
    store
        .create_trigger(
            NewTrigger { code: "nightly_digest", name: "夜报", spec: "{}" },
            1_000,
        )
        .expect("create");
    (dir, store)
}

fn count(store: &Store) -> i64 {
    store
        .conn()
        .query_row("SELECT count(*) FROM fires", [], |row| row.get(0))
        .expect("count")
}

fn fire(state: FireState, fire_at: i64) -> Fire {
    Fire {
        trigger_code: "nightly_digest".to_string(),
        fire_at,
        state,
        request_id: request_id("nightly_digest", fire_at),
        note: None,
    }
}

#[test]
fn same_pair_twice_is_still_one_row() {
    let (_dir, store) = open();
    // 判据 3（唯一键 = 幂等的物理保证）：同 (code, fire_at) 再来一次不是错误，
    // 是「已经有了」。去掉 PRIMARY KEY → 行数 2 → 这条红。
    assert_eq!(
        store.record_fire("nightly_digest", 1_700_000_060_000, None).expect("first"),
        Inserted::New
    );
    assert_eq!(
        store.record_fire("nightly_digest", 1_700_000_060_000, None).expect("second"),
        Inserted::AlreadyThere
    );
    assert_eq!(count(&store), 1, "唯一键去掉 → 这里变 2");

    // 不同的 fire_at 是新的一笔账。
    assert_eq!(
        store.record_fire("nightly_digest", 1_700_000_120_000, None).expect("third"),
        Inserted::New
    );
    assert_eq!(count(&store), 2);

    let row = store
        .fire("nightly_digest", 1_700_000_060_000)
        .expect("read")
        .expect("行");
    assert_eq!(row.state, FireState::Pending, "新账一开始是 pending");
    assert_eq!(row.request_id, request_id("nightly_digest", 1_700_000_060_000));
}

#[test]
fn request_id_is_derived_from_code_and_fire_at() {
    assert_eq!(request_id("nightly_digest", 0), "trigger:nightly_digest:0");
    assert_eq!(
        request_id("nt", 1_700_000_060_000),
        "trigger:nt:1700000060000"
    );
}

#[test]
fn pending_within_the_grace_period_is_left_alone() {
    let now = 10 * GRACE_MS;
    for age in [-5 * GRACE_MS, -1, 0, 1, GRACE_MS / 2, GRACE_MS] {
        let row = fire(FireState::Pending, now - age);
        assert!(
            reclaim(std::slice::from_ref(&row), now).is_empty(),
            "宽限期内不动（age={age}，含 fire_at 在未来的情况）"
        );
    }
}

#[test]
fn overdue_pending_is_orphaned_and_then_replayed() {
    let now = 10 * GRACE_MS;
    for age in [GRACE_MS + 1, 2 * GRACE_MS, i64::MAX / 4] {
        let row = fire(FireState::Pending, now - age);
        let actions = reclaim(std::slice::from_ref(&row), now);
        assert_eq!(
            actions,
            vec![
                MarkOrphaned { trigger_code: "nightly_digest".to_string(), fire_at: now - age },
                Replay {
                    trigger_code: "nightly_digest".to_string(),
                    fire_at: now - age,
                    request_id: request_id("nightly_digest", now - age),
                },
            ],
            "先记 orphaned 再重放（顺序写死，age={age}）"
        );
    }
}

#[test]
fn done_is_never_replayed_at_any_age() {
    // 负向哨兵：把 done 也扫进去 → 同一条消息重复提交 → 这条红。
    let now = 10 * GRACE_MS;
    for age in [-5 * GRACE_MS, -1, 0, 1, GRACE_MS, GRACE_MS + 1, 100 * GRACE_MS, i64::MAX / 4] {
        let row = fire(FireState::Done, now - age);
        assert!(
            reclaim(std::slice::from_ref(&row), now).is_empty(),
            "done 永不重放（age={age}）"
        );
    }
}

#[test]
fn orphaned_left_by_a_crash_between_mark_and_submit_is_replayed() {
    // 顺序是「写 orphaned → 重放」；两步之间崩了，下一轮必须补上重放，而不是永远丢了。
    let now = 10 * GRACE_MS;
    for age in [GRACE_MS + 1, 2 * GRACE_MS] {
        let row = fire(FireState::Orphaned, now - age);
        assert_eq!(
            reclaim(std::slice::from_ref(&row), now),
            vec![Replay {
                trigger_code: "nightly_digest".to_string(),
                fire_at: now - age,
                request_id: request_id("nightly_digest", now - age),
            }],
            "已经 orphaned 的不再重复记一次（age={age}）"
        );
    }
}

#[test]
fn reclaim_is_pure_and_keeps_input_order() {
    let now = 10 * GRACE_MS;
    let rows = vec![
        fire(FireState::Done, now - 10 * GRACE_MS),
        fire(FireState::Pending, now - GRACE_MS - 1),
        fire(FireState::Pending, now),
        fire(FireState::Orphaned, now - 3 * GRACE_MS),
    ];
    let first = reclaim(&rows, now);
    let second = reclaim(&rows, now);
    assert_eq!(first, second, "纯函数：同样的 (rows, now) 同样的输出");
    assert_eq!(
        first,
        vec![
            MarkOrphaned { trigger_code: "nightly_digest".to_string(), fire_at: now - GRACE_MS - 1 },
            Replay {
                trigger_code: "nightly_digest".to_string(),
                fire_at: now - GRACE_MS - 1,
                request_id: request_id("nightly_digest", now - GRACE_MS - 1),
            },
            Replay {
                trigger_code: "nightly_digest".to_string(),
                fire_at: now - 3 * GRACE_MS,
                request_id: request_id("nightly_digest", now - 3 * GRACE_MS),
            },
        ],
        "每行按输入顺序，done 与宽限期内的 pending 都不出现"
    );
}

#[test]
fn the_scan_sees_pending_and_orphaned_but_not_done() {
    let (_dir, store) = open();
    store.record_fire("nightly_digest", 100, None).expect("pending");
    store.record_fire("nightly_digest", 200, None).expect("orphaned");
    store.record_fire("nightly_digest", 300, None).expect("done");
    store.mark_orphaned("nightly_digest", 200).expect("mark");
    store.mark_done("nightly_digest", 300).expect("mark");

    let open_fires = store.open_fires().expect("open_fires");
    let shape: Vec<(i64, FireState)> = open_fires.iter().map(|f| (f.fire_at, f.state)).collect();
    assert_eq!(
        shape,
        vec![(100, FireState::Pending), (200, FireState::Orphaned)],
        "done 不进扫描输入，且按 fire_at 升序"
    );
    assert_eq!(store.fires().expect("fires").len(), 3, "fires() 是全量账本");
}

#[test]
fn done_is_terminal_a_later_scan_cannot_downgrade_it() {
    // 判据 4 的另一半：`done` 不只是「这一轮不重放」，而是**任何一轮都碰不到**。
    // `mark_orphaned` 的 `AND state = 'pending'` 就是这条保证；去掉它 → 重放同一条消息 → 红。
    let (_dir, store) = open();
    store.record_fire("nightly_digest", 100, None).expect("fire");
    store.mark_done("nightly_digest", 100).expect("done");

    store.mark_orphaned("nightly_digest", 100).expect("no-op");
    let row = store.fire("nightly_digest", 100).expect("read").expect("行");
    assert_eq!(row.state, FireState::Done, "done 不许被扫回 orphaned");

    // 再记账（同一拍敲两次）也不许把它退回 pending。
    assert_eq!(
        store.record_fire("nightly_digest", 100, None).expect("dup"),
        Inserted::AlreadyThere
    );
    assert_eq!(
        store.fire("nightly_digest", 100).expect("read").expect("行").state,
        FireState::Done
    );

    // 于是它永远不进扫描输入。
    assert!(store.open_fires().expect("open_fires").is_empty());
    assert!(reclaim(&store.fires().expect("fires"), 100 + 100 * GRACE_MS).is_empty());
}

#[test]
fn mark_done_and_mark_orphaned_are_idempotent() {
    let (_dir, store) = open();
    store.record_fire("nightly_digest", 100, Some("首轮")).expect("fire");

    store.mark_orphaned("nightly_digest", 100).expect("orphaned");
    store.mark_orphaned("nightly_digest", 100).expect("orphaned again");
    assert_eq!(
        store.fire("nightly_digest", 100).expect("read").expect("行").state,
        FireState::Orphaned
    );

    store.mark_done("nightly_digest", 100).expect("done");
    store.mark_done("nightly_digest", 100).expect("done again");
    let row = store.fire("nightly_digest", 100).expect("read").expect("行");
    assert_eq!(row.state, FireState::Done, "重放成功后 done 覆盖 orphaned");
    assert_eq!(row.note.as_deref(), Some("首轮"), "note 不因状态变而丢");
}
