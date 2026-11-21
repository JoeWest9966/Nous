//! #76 接缝 B · `/v1/repos/:repo/variants/*` 的 HTTP 形状与错误码（真 Router）。

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
    table: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let root = tmp.path().join("skills");
    let table = tmp.path().join("data/skills.local.json");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp.path(), &["init", "--bare", "-q", remote.to_str().unwrap()]);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-qm", "init"]);
    git(&root, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    Fixture { _tmp: tmp, root, remote, table }
}

fn app_with(f: &Fixture) -> axum::Router {
    router(
        AppState::with_source(common::FakeSource::two_sessions())
            .with_repos_for_test("skills", &f.root)
            .with_variant_table_for_test(&f.table),
    )
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body { Some(v) => Body::from(serde_json::to_vec(&v).unwrap()), None => Body::empty() })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn bind_publish_branch_and_unbind_are_live_over_http() {
    let f = fixture();
    let app = app_with(&f);
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();

    let (status, bound) = call(&app, "POST", "/v1/repos/skills/variants/bind",
        Some(json!({ "id": "alpha" }))).await;
    assert_eq!(status, StatusCode::OK, "bind 200：{bound}");
    let root = bound["root"].as_str().unwrap();
    let branch = bound["branch"].as_str().unwrap();
    assert_eq!(branch, "skill/alpha");
    assert!(Path::new(root).join("alpha/SKILL.md").exists());

    let (status, table) = call(&app, "GET", "/v1/repos/skills/variants", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(table["variants"]["skill"]["alpha"]["branch"], json!(branch));

    std::fs::write(Path::new(root).join("alpha/SKILL.md"), "---\nname: alpha\ndescription: v2\n---\nbody\n").unwrap();
    let (status, published) = call(&app, "POST", "/v1/repos/skills/variants/publish",
        Some(json!({ "id": "alpha", "message": "v2", "requestId": "r-1" }))).await;
    assert_eq!(status, StatusCode::OK, "publish 200：{published}");
    assert_eq!(published["pushed"], json!(true));
    assert_eq!(published["branch"], json!(branch));
    let remote = git(&f.remote, &["rev-parse", &format!("refs/heads/{branch}")]);
    assert_eq!(published["commit"], json!(remote), "远端变体分支有这笔提交");

    let (status, switched) = call(&app, "POST", "/v1/repos/skills/variants/branch",
        Some(json!({ "id": "alpha", "branch": "skill/other" }))).await;
    assert_eq!(status, StatusCode::OK, "branch 200：{switched}");
    assert_eq!(switched["branch"], json!("skill/other"));

    let (status, unbound) = call(&app, "POST", "/v1/repos/skills/variants/unbind",
        Some(json!({ "id": "alpha" }))).await;
    assert_eq!(status, StatusCode::OK, "unbind 200：{unbound}");
    assert_eq!(unbound["removed"], json!(true));
    assert!(Path::new(root).exists(), "unbind 后树还在");
}

#[tokio::test]
async fn branch_main_is_400_and_prune_unmerged_needs_force() {
    let f = fixture();
    let app = app_with(&f);
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();
    let (_, bound) = call(&app, "POST", "/v1/repos/skills/variants/bind",
        Some(json!({ "id": "alpha" }))).await;
    let root = bound["root"].as_str().unwrap();

    let (status, body) = call(&app, "POST", "/v1/repos/skills/variants/branch",
        Some(json!({ "id": "alpha", "branch": "main" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "branch main 400：{body}");

    let (status, body) = call(&app, "POST", "/v1/repos/skills/variants/prune",
        Some(json!({ "id": "alpha" }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "未合并 prune 409：{body}");
    assert!(Path::new(root).exists(), "409 不删树");

    let (status, body) = call(&app, "POST", "/v1/repos/skills/variants/prune",
        Some(json!({ "id": "alpha", "force": true }))).await;
    assert_eq!(status, StatusCode::OK, "带 force 真删：{body}");
    assert_eq!(body["unmerged"], json!(true));
    assert!(!Path::new(root).exists());
}

#[tokio::test]
async fn usage_returns_session_ids_for_a_variant_name() {
    let f = fixture();
    // 真 durable 源：一个会话用了 skill:alpha。
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("sessions.sqlite");
    // 真 durable 库：会话 7 的 skills 字段挂 alpha。
    {
        use nous_durable::session::{CommitOptions, Session};
        use nous_durable::StorageWrite;
        let session = Session::open(&db).unwrap();
        session.commit(
            vec![StorageWrite::Conversation(json!({
                "id": 7, "skills": ["alpha"],
            }))],
            CommitOptions::default(),
        ).await.unwrap();
        session.close().await.unwrap();
    }
    let app = router(
        AppState::durable(&db).unwrap()
            .with_repos_for_test("skills", &f.root)
            .with_variant_table_for_test(&f.table),
    );

    let (status, body) = call(&app, "GET", "/v1/repos/skills/variants/usage?id=alpha", None).await;
    assert_eq!(status, StatusCode::OK, "usage 200：{body}");
    assert!(body.as_array().is_some_and(|a| a.contains(&json!(7))), "usage 含会话 id：{body}");

    let (status, body) = call(&app, "GET", "/v1/repos/skills/variants/usage?id=nobody", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]), "没人用回空数组：{body}");
}
