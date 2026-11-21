//! #77 接缝 B：`/v1/repos/:repo/{sources,upstream*}` 的 HTTP 形状与判据 1-5。

mod common;

use std::fs;
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
    upstream: PathBuf,
    root: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("upstream-origin");
        let root = tmp.path().join("skills");
        let data = tmp.path().join("data");
        for dir in [&upstream, &root, &data] {
            fs::create_dir_all(dir).unwrap();
        }
        git(&upstream, &["init", "-q"]);
        git(&upstream, &["config", "user.email", "t@e"]);
        git(&upstream, &["config", "user.name", "T"]);
        git(&upstream, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        Self { _tmp: tmp, upstream, root, data }
    }

    fn commit(&self, rel: &str, content: &str) -> String {
        let path = self.upstream.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        git(&self.upstream, &["add", rel]);
        git(&self.upstream, &["commit", "-qm", "add {rel}"]);
        git(&self.upstream, &["rev-parse", "HEAD"])
    }

    fn app(&self) -> axum::Router {
        let state = AppState::with_source(common::FakeSource::two_sessions())
            .with_repos_for_test("skills", &self.root)
            .with_upstream_data_for_test(&self.data);
        router(state)
    }
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri)
        .header("content-type", "application/json")
        .body(match body { Some(v) => Body::from(serde_json::to_vec(&v).unwrap()), None => Body::empty() })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn source(url: &str) -> Value {
    json!({ "name": "skills", "url": url, "branch": "main" })
}

#[tokio::test]
async fn check_reports_newer_and_sync_pins_over_http() {
    let f = Fixture::new();
    let first = f.commit("demo/SKILL.md", "---\nname: demo\ndescription: d\n---\nbody\n");
    let app = f.app();

    let (status, written) = call(&app, "POST", "/v1/repos/skills/sources",
        Some(json!({ "sources": [source(&f.upstream.to_string_lossy())] }))).await;
    assert_eq!(status, StatusCode::OK, "sources 整份替换 200：{written}");

    // sources.json 是 {v, sources}；GET 要把 v 也摆出来，不能吞掉版本。
    let (_, doc) = call(&app, "GET", "/v1/repos/skills/sources", None).await;
    assert_eq!(doc["v"], json!(1), "sources 文档要有 v：{doc}");
    let on_disk: Value = serde_json::from_str(&fs::read_to_string(f.root.join("sources.json")).unwrap()).unwrap();
    assert_eq!(on_disk["v"], json!(1), "sources.json 要写 v：{on_disk}");

    let (status, refreshed) = call(&app, "POST", "/v1/repos/skills/upstream/refresh",
        Some(json!({ "name": "skills" }))).await;
    assert_eq!(status, StatusCode::OK, "refresh 200：{refreshed}");
    assert_eq!(refreshed["head"], json!(first));

    let second = f.commit("demo/extra.md", "附件\n");
    let (status, checked) = call(&app, "POST", "/v1/repos/skills/upstream/check",
        Some(json!({ "name": "skills" }))).await;
    assert_eq!(status, StatusCode::OK, "check 200：{checked}");
    assert_eq!(checked["checks"][0]["head"], json!(second));

    let (_, overview) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    let row = &overview["sources"][0];
    assert_eq!(row["mirror_head"], json!(first));
    assert_eq!(row["pinned"], json!(first), "首拉基线：{row}");
    assert_eq!(row["newer"], json!(true), "远端 HEAD != pinned → newer");
    assert_eq!(row["drift"], json!(false));
    assert!(row.get("bytes").is_some() && row.get("entries").is_some(), "概览必须有 bytes/entries：{row}");

    let (status, synced) = call(&app, "POST", "/v1/repos/skills/upstream/sync",
        Some(json!({ "name": "skills" }))).await;
    assert_eq!(status, StatusCode::OK, "sync 200：{synced}");
    assert_eq!(synced["head"], json!(second));
    let (_, overview) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    let row = &overview["sources"][0];
    assert_eq!(row["pinned"], json!(second), "sync 必须更新 pinned：{row}");
    assert_eq!(row["newer"], json!(false), "sync 后 newer 归零");

    // 别的端点不改 version：pin 之外只有首拉基线能写。
    let mtime_before = fs::metadata(f.root.join("sources.json")).unwrap().modified().unwrap();
    call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    call(&app, "GET", "/v1/repos/skills/upstream/tree?name=skills&path=demo", None).await;
    call(&app, "GET", "/v1/repos/skills/upstream/file?name=skills&path=demo%2FSKILL.md", None).await;
    let (_, after_sources) = call(&app, "GET", "/v1/repos/skills/sources", None).await;
    assert_eq!(after_sources["sources"][0]["version"]["commit"], json!(second));
    assert_eq!(fs::metadata(f.root.join("sources.json")).unwrap().modified().unwrap(), mtime_before, "只读端点不许重写 sources.json");

    let (status, pinned) = call(&app, "POST", "/v1/repos/skills/upstream/pin",
        Some(json!({ "name": "skills", "commit": first }))).await;
    assert_eq!(status, StatusCode::OK, "pin 200：{pinned}");
    assert_eq!(pinned["commit"], json!(first));
    let (_, final_overview) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    assert_eq!(final_overview["sources"][0]["pinned"], json!(first));
}

#[tokio::test]
async fn drift_clears_after_refresh_over_http() {
    let f = Fixture::new();
    let first = f.commit("demo/SKILL.md", "---\nname: demo\ndescription: d\n---\nbody\n");
    let app = f.app();
    call(&app, "POST", "/v1/repos/skills/sources", Some(json!({ "sources": [source(&f.upstream.to_string_lossy())] }))).await;
    call(&app, "POST", "/v1/repos/skills/upstream/refresh", Some(json!({ "name": "skills" }))).await;

    let mirror = f.data.join("upstream/skills");
    fs::write(mirror.join("demo/SKILL.md"), "---\nname: demo\ndescription: 手改\n---\n").unwrap();

    let (_, drifted) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    assert_eq!(drifted["sources"][0]["drift"], json!(true), "手改镜像文件 → drift：{drifted}");

    call(&app, "POST", "/v1/repos/skills/upstream/refresh", Some(json!({ "name": "skills" }))).await;
    let (_, clean) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    assert_eq!(clean["sources"][0]["mirror_head"], json!(first));
    assert_eq!(clean["sources"][0]["drift"], json!(false), "refresh 后 drift 清零：{clean}");
}

#[tokio::test]
async fn check_does_not_download_or_touch_mirror_files_over_http() {
    let f = Fixture::new();
    let first = f.commit("demo/SKILL.md", "---\nname: demo\ndescription: d\n---\nbody\n");
    let app = f.app();
    call(&app, "POST", "/v1/repos/skills/sources", Some(json!({ "sources": [source(&f.upstream.to_string_lossy())] }))).await;
    call(&app, "POST", "/v1/repos/skills/upstream/refresh", Some(json!({ "name": "skills" }))).await;
    f.commit("demo/extra.md", "上游新增\n");

    let mirror = f.data.join("upstream/skills");
    let before: Vec<_> = files_with_mtime(&mirror)
        .into_iter()
        .filter(|(p, _)| p.file_name().and_then(|n| n.to_str()) != Some("nous-check.json"))
        .collect();
    let objects_before = files_with_mtime(&mirror.join(".git/objects")).len();

    call(&app, "POST", "/v1/repos/skills/upstream/check", Some(json!({ "name": "skills" }))).await;
    assert_eq!(fs::metadata(f.data.join("upstream/skills/.git/nous-check.json")).is_ok(), true, "check 写缓存");
    let after: Vec<_> = files_with_mtime(&mirror)
        .into_iter()
        .filter(|(p, _)| p.file_name().and_then(|n| n.to_str()) != Some("nous-check.json"))
        .collect();
    assert_eq!(before, after, "check 不改镜像文件 mtime");
    assert_eq!(files_with_mtime(&mirror.join(".git/objects")).len(), objects_before, "check 不下载对象");
    assert_eq!(git(&mirror, &["rev-parse", "HEAD"]), first);
}

#[tokio::test]
async fn adopt_writes_foot_and_file_states_over_http() {
    let f = Fixture::new();
    f.commit("demo/SKILL.md", "---\nname: demo\ndescription: d\n---\nbody\n");
    f.commit("demo/attach.md", "附件原文\n");
    let app = f.app();
    call(&app, "POST", "/v1/repos/skills/sources", Some(json!({ "sources": [source(&f.upstream.to_string_lossy())] }))).await;
    call(&app, "POST", "/v1/repos/skills/upstream/refresh", Some(json!({ "name": "skills" }))).await;

    let (status, tree) = call(&app, "GET", "/v1/repos/skills/upstream/tree?name=skills&path=demo", None).await;
    assert_eq!(status, StatusCode::OK, "tree 200：{tree}");
    assert!(tree["items"].as_array().is_some_and(|a| a.iter().any(|i| i["path"] == json!("demo/attach.md"))), "tree 列附件：{tree}");

    let (status, body) = call(&app, "GET", "/v1/repos/skills/upstream/file?name=skills&path=demo%2Fattach.md", None).await;
    assert_eq!(status, StatusCode::OK, "file 200：{body}");
    assert_eq!(body["text"], json!("附件原文\n"));

    let (status, adopted) = call(&app, "POST", "/v1/repos/skills/upstream/adopt",
        Some(json!({ "name": "skills", "path": "demo" }))).await;
    assert_eq!(status, StatusCode::OK, "adopt 200：{adopted}");
    let foot = adopted["foot"].as_str().expect("adopt 必须回 foot 标记");
    assert!(foot.starts_with("skills/demo@"), "foot 形状 source/path@commit：{foot}");
    let manifest: Value = serde_json::from_str(&fs::read_to_string(f.root.join("demo/nous.json")).unwrap()).unwrap();
    assert_eq!(manifest["upstream"], json!(foot), "foot 落进 nous.json");

    let (_, states) = call(&app, "GET", "/v1/repos/skills/upstream/files?name=skills&path=demo", None).await;
    assert!(states["files"].as_array().is_some_and(|a| a.iter().any(|f| f["path"] == "SKILL.md" && f["state"] == "same")), "same：{states}");

    fs::write(f.root.join("demo/attach.md"), "本地改过\n").unwrap();
    let (_, states) = call(&app, "GET", "/v1/repos/skills/upstream/files?name=skills&path=demo", None).await;
    assert!(states["files"].as_array().is_some_and(|a| a.iter().any(|f| f["path"] == "attach.md" && f["state"] == "diff")), "diff：{states}");

    fs::remove_file(f.root.join("demo/attach.md")).unwrap();
    let (_, states) = call(&app, "GET", "/v1/repos/skills/upstream/files?name=skills&path=demo", None).await;
    assert!(states["files"].as_array().is_some_and(|a| a.iter().any(|f| f["path"] == "attach.md" && f["state"] == "missing")), "missing：{states}");

    let (status, conflict) = call(&app, "POST", "/v1/repos/skills/upstream/copy",
        Some(json!({ "name": "skills", "path": "demo", "to": "demo" }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "目标已存在 409：{conflict}");
}

#[tokio::test]
async fn overview_is_envelope_with_sources_and_errors() {
    let f = Fixture::new();
    f.commit("demo/SKILL.md", "---\nname: demo\ndescription: d\n---\nbody\n");
    let app = f.app();
    call(&app, "POST", "/v1/repos/skills/sources",
        Some(json!({ "sources": [source(&f.upstream.to_string_lossy())] }))).await;

    let (status, overview) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    assert_eq!(status, StatusCode::OK, "overview 200：{overview}");
    let rows = overview.get("sources").and_then(Value::as_array)
        .expect("overview 必须是 {{sources, errors}} 信封：{overview}");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["mirror"], json!("missing"));
    assert_eq!(rows[0]["pinned"], Value::Null, "没拉过镜像就没有 pinned");
    assert_eq!(rows[0]["newer"], json!(false), "没 check 过 → newer:false（打开面板不联网）");
    assert_eq!(rows[0]["drift"], json!(false));
    assert!(overview.get("errors").is_some_and(Value::is_array), "overview 必须有 errors 数组：{overview}");

    call(&app, "POST", "/v1/repos/skills/upstream/refresh", Some(json!({ "name": "skills" }))).await;
    let (_, ready) = call(&app, "GET", "/v1/repos/skills/upstream", None).await;
    assert_eq!(ready["sources"][0]["mirror"], json!("ready"));
    assert!(ready["sources"][0].get("entries").is_some(), "概览必须报 entries：{ready}");
}

#[tokio::test]
async fn unknown_repo_is_400_and_unknown_source_is_400() {
    let f = Fixture::new();
    let app = f.app();
    let (status, _) = call(&app, "GET", "/v1/repos/unknown/upstream", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, "POST", "/v1/repos/skills/upstream/refresh",
        Some(json!({ "name": "not-subscribed" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

fn files_with_mtime(dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    fn collect(dir: &Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        out.push((path, meta.modified().unwrap()));
                    } else if meta.is_dir() {
                        collect(&path, out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    collect(dir, &mut out);
    out
}
