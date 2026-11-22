//! 资源仓管理面（`/v1/{skills,prompts,extensions}/{files,file,create,publish}`）的 HTTP 形状。
//!
//! 判据打在公共接缝：真 Router + 真 git 仓（临时目录），不走内部函数。
//! Desk 的编辑页就是按这四条接线：清单 → 读 → 存（带 base）→ 新建 → 发布；
//! 提示词 / 扩展与技能共用一副骨架，只是私库键与骨架文件不同。

mod common;

use std::path::Path;
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 一个已提交的资源私库。`kind` 是 AppState 的私库键（`skills` / `prompts` / `extend`），
/// 每个仓铺一条真实条目 + 一个点开头的目录 + 一个二进制附件。
fn fixture(kind: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let (entry, primary, primary_text) = match kind {
        "skills" => (
            "demo",
            "SKILL.md",
            "---\nname: demo\ndescription: 演示\n---\n\n# demo\n",
        ),
        "prompts" => ("demo", "PROMPT.md", "# 演示\n\n正文\n"),
        _ => ("123", "main.js", "// 扩展入口\n"),
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(kind);
    std::fs::create_dir_all(root.join(entry).join("references")).unwrap();
    std::fs::create_dir_all(root.join(entry).join(".cache")).unwrap();
    std::fs::write(root.join(entry).join(primary), primary_text).unwrap();
    std::fs::write(
        root.join(entry).join("nous.json"),
        "{\n  \"name\": \"demo\"\n}\n",
    )
    .unwrap();
    std::fs::write(root.join(entry).join("references/a.md"), "参考\n").unwrap();
    std::fs::write(root.join(entry).join(".cache/x.md"), "缓存\n").unwrap();
    std::fs::write(root.join(entry).join("logo.png"), [0u8, 1, 2, 3]).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    (tmp, root)
}

fn app_with(kind: &str, root: &Path) -> Router {
    let state =
        AppState::with_source(common::FakeSource::two_sessions()).with_repos_for_test(kind, root);
    router(state)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn files_lists_recursively_and_skips_hidden_and_node_modules() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    let (status, body) = call(&app, "GET", "/v1/skills/files?name=demo", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], json!("demo"));
    let paths: Vec<&str> = body["files"]
        .as_array()
        .expect("files 数组")
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"SKILL.md"), "正文在最前：{paths:?}");
    assert!(paths.contains(&"references/a.md"), "递归到底：{paths:?}");
    assert!(
        !paths.iter().any(|path| path.starts_with(".cache")),
        "点开头不进列表：{paths:?}"
    );
    assert!(
        paths.contains(&"logo.png"),
        "二进制也列（读的时候才拒）：{paths:?}"
    );
    let size = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == json!("references/a.md"))
        .and_then(|file| file["size"].as_u64());
    assert!(size.is_some_and(|size| size > 0), "每行都带 size：{body}");

    let (status, body) = call(&app, "GET", "/v1/skills/files?name=nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "没这条技能：{body}");
}

#[tokio::test]
async fn read_returns_text_and_etag_and_refuses_binary() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    let (status, body) = call(
        &app,
        "GET",
        "/v1/skills/file?name=demo&path=references/a.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], json!("demo"));
    assert_eq!(body["path"], json!("references/a.md"));
    assert_eq!(body["text"], json!("参考\n"));
    assert!(
        body["etag"].as_str().is_some_and(|etag| etag.len() == 64),
        "etag 是内容 sha256：{body}"
    );

    let (status, body) = call(&app, "GET", "/v1/skills/file?name=demo&path=logo.png", None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "二进制要明说看不了：{body}"
    );

    let (status, body) = call(
        &app,
        "GET",
        "/v1/skills/file?name=demo&path=../demo/SKILL.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "不许用 .. 串路径：{body}");
}

#[tokio::test]
async fn save_writes_with_base_and_rejects_stale_base() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    let (_, before) = call(&app, "GET", "/v1/skills/file?name=demo&path=SKILL.md", None).await;
    let base = before["etag"].as_str().unwrap().to_string();

    let (status, saved) = call(
        &app,
        "PUT",
        "/v1/skills/file",
        Some(json!({ "name": "demo", "path": "SKILL.md", "text": "---\nname: demo\ndescription: 改过\n---\n", "base": base })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "带 base 的写回：{saved}");
    assert_eq!(saved["created"], json!(false));
    assert_ne!(saved["etag"], json!(base), "内容变了 etag 就变：{saved}");
    let on_disk = std::fs::read_to_string(root.join("demo/SKILL.md")).unwrap();
    assert!(on_disk.contains("改过"), "真落盘：{on_disk}");

    // 同一个 base 再写一次 = 拿着旧版本覆盖 → 409，不许静默盖掉。
    let (status, body) = call(
        &app,
        "PUT",
        "/v1/skills/file",
        Some(json!({ "name": "demo", "path": "SKILL.md", "text": "x", "base": base })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "旧 base 必须 409：{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("重新打开")),
        "409 要给人话：{body}"
    );

    // 新建一个还不存在的文件：base 省掉。
    let (status, created) = call(
        &app,
        "PUT",
        "/v1/skills/file",
        Some(json!({ "name": "demo", "path": "notes/new.md", "text": "新附件\n" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "新文件可以直接写：{created}");
    assert_eq!(created["created"], json!(true));
    assert_eq!(
        std::fs::read_to_string(root.join("demo/notes/new.md")).unwrap(),
        "新附件\n"
    );
}

#[tokio::test]
async fn create_skill_lands_skill_md_and_manifests_and_never_half_writes() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);

    let (status, body) = call(
        &app,
        "POST",
        "/v1/skills/create",
        Some(json!({ "name": "weekly-digest", "description": "每周汇总", "category": ["复盘"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], json!("weekly-digest"));
    let skill = std::fs::read_to_string(root.join("weekly-digest/SKILL.md")).unwrap();
    assert!(
        skill.starts_with("---\nname: weekly-digest\ndescription:"),
        "{skill}"
    );
    assert!(skill.contains("每周汇总"), "{skill}");
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("weekly-digest/nous.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["name"], json!("weekly-digest"));
    assert_eq!(manifest["summary"], json!(""));
    assert_eq!(manifest["category"], json!(["复盘"]));

    let (status, body) = call(
        &app,
        "POST",
        "/v1/skills/create",
        Some(json!({ "name": "weekly-digest", "description": "重名" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "重名 409：{body}");

    let (status, body) = call(
        &app,
        "POST",
        "/v1/skills/create",
        Some(json!({ "name": "no-desc", "description": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "description 必填：{body}");
    assert!(!root.join("no-desc").exists(), "失败不许留半截目录");

    let (status, body) = call(
        &app,
        "POST",
        "/v1/skills/create",
        Some(json!({ "name": "Bad/Name", "description": "d" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "名字规矩：{body}");
}

#[tokio::test]
async fn create_prompt_lands_prompt_md_and_summary() {
    let (_tmp, root) = fixture("prompts");
    let app = app_with("prompts", &root);

    let (status, body) = call(
        &app,
        "POST",
        "/v1/prompts/create",
        Some(json!({ "name": "strict-review", "summary": "更严格", "category": ["工程"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // 不给正文就落一个标题 —— 新建模板唯一「先有个能用的东西、但不是空壳」的落点。
    assert_eq!(
        std::fs::read_to_string(root.join("strict-review/PROMPT.md")).unwrap(),
        "# strict-review\n"
    );
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("strict-review/nous.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["name"], json!("strict-review"));
    assert_eq!(manifest["summary"], json!("更严格"));
    assert_eq!(manifest["category"], json!(["工程"]));

    let (status, body) = call(
        &app,
        "POST",
        "/v1/prompts/create",
        Some(json!({ "name": "with-body", "summary": "s", "body": "真正文\n" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        std::fs::read_to_string(root.join("with-body/PROMPT.md")).unwrap(),
        "真正文\n"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/v1/prompts/create",
        Some(json!({ "name": "no-summary", "summary": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "summary 必填：{body}");
}

#[tokio::test]
async fn prompts_and_extensions_share_the_same_file_shape() {
    let (_tmp, root) = fixture("prompts");
    let app = app_with("prompts", &root);
    let (status, body) = call(
        &app,
        "GET",
        "/v1/prompts/file?name=demo&path=PROMPT.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["text"]
            .as_str()
            .is_some_and(|text| text.contains("正文")),
        "{body}"
    );
    let (status, saved) = call(
        &app,
        "PUT",
        "/v1/prompts/file",
        Some(json!({ "name": "demo", "path": "PROMPT.md", "text": "# 新版\n" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (_tmp, root) = fixture("extend");
    let app = app_with("extend", &root);
    let (status, body) = call(&app, "GET", "/v1/extensions/files?name=123", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let paths: Vec<&str> = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"main.js"), "扩展也列文件：{paths:?}");
    let (status, body) = call(
        &app,
        "GET",
        "/v1/extensions/file?name=123&path=main.js",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], json!("// 扩展入口\n"));
    let (status, body) = call(
        &app,
        "PUT",
        "/v1/extensions/file",
        Some(json!({ "name": "123", "path": "main.js", "text": "// 改过\n" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        std::fs::read_to_string(root.join("123/main.js")).unwrap(),
        "// 改过\n"
    );
    // 扩展的 create 仍归 ext_admin：这条路由不该被资源面接走。
    let (status, _) = call(
        &app,
        "POST",
        "/v1/extensions/create",
        Some(json!({ "name": "x" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "没装配 ext_admin 时诚实 409（不是 404）"
    );
}

#[tokio::test]
async fn publish_commits_dirty_paths_and_reports_nothing_to_do() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    std::fs::write(
        root.join("demo/SKILL.md"),
        "---\nname: demo\ndescription: 已发布\n---\n",
    )
    .unwrap();

    let before = git(&root, &["rev-parse", "HEAD"]);
    let (status, body) = call(&app, "POST", "/v1/skills/publish", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = body["commit"].as_str().expect("回 commit 哈希");
    assert_ne!(after, before, "真提交了：{body}");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), after);
    assert_eq!(
        git(&root, &["status", "--porcelain"]),
        "",
        "提交后工作树干净"
    );

    let (status, body) = call(&app, "POST", "/v1/skills/publish", Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "没改动不发假成功：{body}");
}

#[tokio::test]
async fn prompt_publish_writes_the_right_default_message() {
    let (_tmp, root) = fixture("prompts");
    let app = app_with("prompts", &root);
    std::fs::write(root.join("demo/PROMPT.md"), "# 改过\n").unwrap();
    let (status, body) = call(&app, "POST", "/v1/prompts/publish", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let message = git(&root, &["log", "-1", "--pretty=%s"]);
    assert_eq!(message, "prompt: 更新模板仓库");
}
