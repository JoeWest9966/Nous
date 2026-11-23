//! 元素库的「写一次提交一次」判据：真临时 git 仓，不走内部函数。
//!
//! 与 `crates/nous/server/tests/resource_admin_endpoints.rs` 同一副夹具：git 用本机
//! 二进制、`GIT_CONFIG_NOSYSTEM` 掐掉宿主配置，不碰网络、不依赖仓库远端。

use std::path::Path;
use std::process::Command;

use nous_elements::{check_id, get, put, remove, resolve, scan, Kind};
use nous_repo::{Pushed, Repo, RepoError};
use serde_json::json;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 一个已提交的元素库：一份组、一份包、一份坏 JSON、一份 id 不合法的文件。
fn fixture() -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Nous-Elements");
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    std::fs::write(
        root.join("groups/frontend.json"),
        "{\n  \"id\": \"frontend\",\n  \"name\": \"前端\",\n  \"packages\": [\"react-rules\", \"gone\"]\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("packages/react-rules.json"),
        "{\n  \"id\": \"react-rules\",\n  \"name\": \"React 规范\",\n  \"kind\": \"prompt\",\n  \"body\": \"1. Hooks 只能放顶层。\",\n  \"tags\": [\"react\"]\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("packages/broken.json"), "{ 这不是 JSON").unwrap();
    std::fs::write(root.join("groups/Bad.json"), "{}\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    let repo = Repo::open(&root).unwrap();
    (tmp, repo)
}

fn count(repo: &Repo) -> String {
    let out = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(repo.root())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn log(repo: &Repo) -> String {
    let out = Command::new("git")
        .args(["log", "--format=%B"])
        .current_dir(repo.root())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn scan_lists_both_kinds_and_isolates_broken_files() {
    let (_tmp, repo) = fixture();
    let groups = scan(&repo, Kind::Groups).unwrap();
    assert_eq!(groups.items.len(), 1);
    assert_eq!(groups.items[0].id, "frontend");
    assert_eq!(groups.items[0].fields["name"], "前端");
    assert_eq!(groups.broken.len(), 1, "Bad.json 的 id 不合法，要单独标红");
    assert_eq!(groups.broken[0].id, "Bad");

    let packages = scan(&repo, Kind::Packages).unwrap();
    assert_eq!(packages.items.len(), 1);
    assert_eq!(packages.items[0].id, "react-rules");
    assert_eq!(packages.broken.len(), 1, "坏 JSON 只标红那一份");
    assert!(packages.broken[0].detail.contains("JSON"));

    // 引用解析：react-rules 在，gone 不在。
    let resolved = resolve(&groups.items, &packages.items);
    assert_eq!(resolved["groups"]["frontend"]["react-rules"]["state"], "ok");
    assert_eq!(resolved["groups"]["frontend"]["gone"]["state"], "missing");
    assert_eq!(resolved["missing"][0], "frontend/gone");
}

#[test]
fn scan_of_a_missing_kind_dir_is_empty_not_an_error() {
    let (_tmp, repo) = fixture();
    std::fs::remove_dir_all(repo.root().join("packages")).unwrap();
    let packages = scan(&repo, Kind::Packages).unwrap();
    assert!(packages.items.is_empty());
    assert!(packages.broken.is_empty());
}

#[test]
fn put_writes_canonical_json_and_commits_exactly_once() {
    let (_tmp, repo) = fixture();
    let before = count(&repo);
    let fields = serde_json::json!({
        "id": "backend",
        "name": "后端",
        "packages": ["react-rules"],
        "unknown": { "keep": [1, 2, 3] }
    });
    let outcome = put(&repo, Kind::Groups, "backend", fields.clone(), None, Some("req-1")).unwrap();
    assert!(outcome.created);
    assert!(!outcome.replay);
    assert!(outcome.commit.is_some(), "写一次必须提交一次");
    assert_eq!(outcome.commit.as_ref().unwrap().pushed, Pushed::Unknown, "没有远端 = null");

    let text = std::fs::read_to_string(repo.root().join("groups/backend.json")).unwrap();
    assert_eq!(text, nous_elements::serialize(&fields), "两空格 + 尾换行，未知字段原样在");

    let after: i32 = count(&repo).parse().unwrap();
    assert_eq!(after, before.parse::<i32>().unwrap() + 1);
    assert!(log(&repo).contains("feat(group): backend"));
    assert!(log(&repo).contains("Request-Id: req-1"));

    // 回读：未知字段一个都不少。
    let read = get(&repo, Kind::Groups, "backend").unwrap();
    assert_eq!(read.fields["unknown"]["keep"], serde_json::json!([1, 2, 3]));
    assert!(!read.etag.is_empty());
}

#[test]
fn replaying_the_same_request_id_does_not_commit_again() {
    let (_tmp, repo) = fixture();
    let fields = serde_json::json!({ "id": "backend", "name": "后端" });
    let first = put(&repo, Kind::Groups, "backend", fields.clone(), None, Some("req-2")).unwrap();
    let commits_after_first = count(&repo);
    assert!(first.commit.is_some());

    let replay = put(&repo, Kind::Groups, "backend", fields, None, Some("req-2")).unwrap();
    assert!(replay.replay, "同锚重放必须认出来");
    assert!(replay.commit.is_none());
    assert!(replay.etag.is_none());
    assert_eq!(count(&repo), commits_after_first, "提交数不许涨");
}

#[test]
fn unchanged_content_makes_no_empty_commit() {
    let (_tmp, repo) = fixture();
    // 先把夹具里那份（内联数组）规范化落盘一次：这才叫「盘上已经是规范格式」。
    let existing = get(&repo, Kind::Packages, "react-rules").unwrap();
    put(&repo, Kind::Packages, "react-rules", existing.fields, None, None).unwrap();
    let normalized = count(&repo);

    let again = get(&repo, Kind::Packages, "react-rules").unwrap();
    let outcome = put(&repo, Kind::Packages, "react-rules", again.fields, None, None).unwrap();
    assert!(outcome.commit.is_none(), "内容一致不该制造空提交");
    assert!(outcome.etag.is_some(), "没提交也要回当前指纹");
    assert_eq!(count(&repo), normalized);
}

#[test]
fn stale_base_etag_is_a_conflict_with_the_current_body() {
    let (_tmp, repo) = fixture();
    let stale = get(&repo, Kind::Packages, "react-rules").unwrap().etag;
    let changed = serde_json::json!({ "id": "react-rules", "name": "改过" });
    put(&repo, Kind::Packages, "react-rules", changed, None, None).unwrap();

    let error = put(
        &repo,
        Kind::Packages,
        "react-rules",
        serde_json::json!({ "id": "react-rules", "name": "又改" }),
        Some(&stale),
        None,
    )
    .unwrap_err();
    match error {
        RepoError::Conflict { current, etag, .. } => {
            assert!(current.is_some(), "409 要带磁盘当前正文");
            assert!(etag.is_some());
        }
        other => panic!("应该是 409，实得 {other:?}"),
    }
}

#[test]
fn remove_commits_then_404s() {
    let (_tmp, repo) = fixture();
    let before: i32 = count(&repo).parse().unwrap();
    let outcome = remove(&repo, Kind::Packages, "react-rules", None, None).unwrap();
    assert!(outcome.commit.is_some());
    assert_eq!(count(&repo).parse::<i32>().unwrap(), before + 1);
    assert!(log(&repo).contains("chore(package): rm react-rules"));
    assert!(!repo.root().join("packages/react-rules.json").exists());

    assert!(matches!(
        remove(&repo, Kind::Packages, "react-rules", None, None),
        Err(RepoError::NotFound(_))
    ));
    assert!(matches!(
        get(&repo, Kind::Packages, "react-rules"),
        Err(RepoError::NotFound(_))
    ));
}

#[test]
fn bad_ids_and_bad_shapes_are_rejected_before_touching_the_repo() {
    let (_tmp, repo) = fixture();
    let before = count(&repo);
    assert!(matches!(
        put(&repo, Kind::Groups, "Bad", serde_json::json!({ "id": "Bad", "name": "x" }), None, None),
        Err(RepoError::BadRequest(_))
    ));
    assert!(matches!(
        put(&repo, Kind::Groups, "ok", serde_json::json!({ "id": "ok" }), None, None),
        Err(RepoError::Validation { field, .. }) if field == "name"
    ));
    assert!(matches!(
        put(&repo, Kind::Groups, "ok", serde_json::json!({ "id": "other", "name": "x" }), None, None),
        Err(RepoError::Validation { field, .. }) if field == "id"
    ));
    assert!(matches!(
        put(&repo, Kind::Groups, "ok", serde_json::json!({ "id": "ok", "name": "x", "packages": "nope" }), None, None),
        Err(RepoError::Validation { field, .. }) if field == "packages"
    ));
    assert_eq!(count(&repo), before, "校验失败不许落盘、不许提交");
    assert!(!repo.root().join("groups/ok.json").exists());
}

#[test]
fn id_rule_matches_the_old_panel() {
    assert!(check_id("react-rules").is_ok());
    assert!(check_id("React").is_err());
}

/// 空私库（还没有任何提交）也要能写进去。
///
/// 踩过：用刚 `git init` 的目录验 Desk 的「新建」，PUT 直接失败在
/// `git log ... your current branch 'master' does not have any commits yet` ——
/// 幂等回执的查询把「没有历史」当成了错误，于是**第一次写永远写不进去**。
#[test]
fn a_repo_without_commits_accepts_the_first_write() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("empty-elements");
    std::fs::create_dir_all(root.join("groups")).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);

    let repo = Repo::open(&root).unwrap();
    let outcome = put(
        &repo,
        Kind::Groups,
        "first",
        json!({"id": "first", "name": "第一个", "packages": []}),
        None,
        Some("rid-first"),
    )
    .expect("空仓的第一次写要成功");
    assert!(outcome.created, "没有旧文件 → created");
    assert!(!outcome.replay);
    assert!(outcome.commit.is_some(), "写完必须有一条提交");
    assert_eq!(count(&repo), "1");

    // 同一锚重放仍然不二次提交。
    let again = put(
        &repo,
        Kind::Groups,
        "first",
        json!({"id": "first", "name": "第一个", "packages": []}),
        None,
        Some("rid-first"),
    )
    .unwrap();
    assert!(again.replay, "同一个 requestId 重放要认出来");
    assert_eq!(count(&repo), "1");
}
