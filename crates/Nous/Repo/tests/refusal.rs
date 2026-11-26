//! `Refusal`：`RepoError` → 回包体的翻译（搬进插件前后逐字一致）。
//!
//! 重点盯 409 的富化三格：`{error, content, etag}`。Desk 靠 `content` 在
//! If-Match 失败时显示盘上现在的正文，少了它客户端只能重新拉一次。

use nous_repo::{Refusal, RepoError};

#[test]
fn conflict_carries_current_content_and_etag() {
    let refusal = Refusal::from_repo(RepoError::conflict(
        "文件已被别人改过",
        b"hello\n".to_vec(),
        "\"abc123\"".to_string(),
    ));
    assert_eq!(refusal.status, 409);
    let body = refusal.body();
    assert_eq!(body["error"], "文件已被别人改过 —— 先重新打开这个文件，再存一次");
    assert_eq!(body["content"], "hello\n");
    assert_eq!(body["etag"], "\"abc123\"");
}

#[test]
fn conflict_without_current_omits_the_field() {
    let refusal = Refusal::from_repo(RepoError::Conflict {
        message: "被占用".into(),
        current: None,
        etag: None,
    });
    let body = refusal.body();
    assert_eq!(body["error"], "被占用 —— 先重新打开这个文件，再存一次");
    assert!(body.get("content").is_none(), "没有磁盘正文时不该凭空造一格");
    assert!(body.get("etag").is_none());
}

#[test]
fn statuses_match_the_old_http_table() {
    // 搬家前的 `server::repos::repo_error` 就是这张表，逐格钉住。
    let cases: Vec<(RepoError, u16)> = vec![
        (RepoError::BadRequest("x".into()), 400),
        (RepoError::InvalidPath("x".into()), 400),
        (RepoError::Validation { field: "f".into(), problem: "p".into() }, 400),
        (RepoError::NotFound("x".into()), 404),
        (RepoError::Busy("x".into()), 409),
        (RepoError::Locked("x".into()), 423),
        (RepoError::Io("x".into()), 500),
        (RepoError::LockPoisoned, 500),
    ];
    for (error, status) in cases {
        let refusal = Refusal::from_repo(error);
        assert_eq!(refusal.status, status);
    }
}

#[test]
fn validation_carries_field_and_problem() {
    let refusal = Refusal::from_repo(RepoError::Validation {
        field: "name".into(),
        problem: "不能为空".into(),
    });
    let body = refusal.body();
    assert_eq!(body["error"], "内容闸未过");
    assert_eq!(body["field"], "name");
    assert_eq!(body["problem"], "不能为空");
}
