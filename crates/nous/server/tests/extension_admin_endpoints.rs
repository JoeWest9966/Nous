//! #84：扩展管理面的 HTTP 判据。
//!
//! 两个正交接缝：写动作落真私库（盘上文件 + git pending），运行动作经 #81 的
//! 真 `Runtime`（清单 + 子进程 pid）。种子和断言都不读产品内部状态，只读
//! HTTP 回包、盘上文件、git 输出和 `/proc`。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

mod common;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 进程级环境变量的看门人：退出时恢复，避免测试间互相踩路径。
struct EnvGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new(pairs: &[(&'static str, PathBuf)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut previous = Vec::new();
        for (name, value) in pairs {
            previous.push((*name, std::env::var_os(name)));
            std::env::set_var(name, value);
        }
        Self { previous, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root.pop();
    root
}

/// 编译样例扩展，返回二进制路径（与 #81 同法：问 cargo，不信固定 target 路径）。
fn sample_ext_bin() -> PathBuf {
    let built = Command::new("cargo")
        .args(["build", "--quiet", "-p", "exts-handle", "--bin", "exts-handle"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo build exts-handle");
    assert!(built.status.success(), "样例扩展编译失败：{}", String::from_utf8_lossy(&built.stderr));
    let path = PathBuf::from(env!("CARGO_BIN_EXE_nous-app"))
        .parent().unwrap().join("exts-handle");
    assert!(path.exists(), "样例扩展二进制不存在：{}", path.display());
    path
}

/// 造一个能真装载的扩展目录 + 产物元数据，返回可直接 start_all 的运行时。
fn install_runnable(root: &Path, name: &str, binary: &Path) -> std::sync::Arc<nous_ext::Runtime> {
    let exts = root.join(name);
    let artifacts = root.parent().unwrap().join("artifacts");
    std::fs::create_dir_all(&exts).unwrap();
    std::fs::create_dir_all(&artifacts).unwrap();
    let manifest = json!({
        "name": name, "version": "0.1.0", "protocol": 3,
        "file": "exts-handle", "entry": "exts-handle",
        "sections": [{"key":"prompts","tag":"prompts"}],
        "handles": [], "summary": "#84 sample", "category": "sample"
    });
    std::fs::write(exts.join("nous.json"), manifest.to_string()).unwrap();
    let target = artifacts.join("exts-handle");
    std::fs::copy(binary, &target).unwrap();
    let output = Command::new("sha256sum").arg(&target).output().unwrap();
    let hash = String::from_utf8(output.stdout).unwrap().split_whitespace().next().unwrap().to_string();
    std::fs::write(artifacts.join("exts-handle.json"), json!({
        "hash": hash, "builtFrom": "t84", "buildMachine": "container",
        "protocol": 3, "triple": "x86_64-unknown-linux-gnu"
    }).to_string()).unwrap();

    let prompts = root.parent().unwrap().join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    let mut config = nous_ext::RuntimeConfig::new(root, artifacts);
    config.isolation.enabled = false;
    config.handles.insert("prompts".into(), nous_ext::HandleValue::dir(prompts));
    let runtime = nous_ext::Runtime::new(config);
    runtime.start_all();
    runtime
}

/// 只造产物元数据（不建扩展源码目录），供 install 从 HEAD 装回后真装载。
fn install_runnable_artifacts_only(root: &Path, binary: &Path) -> std::sync::Arc<nous_ext::Runtime> {
    let artifacts = root.parent().unwrap().join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let target = artifacts.join("exts-handle");
    std::fs::copy(binary, &target).unwrap();
    let output = Command::new("sha256sum").arg(&target).output().unwrap();
    let hash = String::from_utf8(output.stdout).unwrap().split_whitespace().next().unwrap().to_string();
    std::fs::write(artifacts.join("exts-handle.json"), json!({
        "hash": hash, "builtFrom": "t84", "buildMachine": "container",
        "protocol": 3, "triple": "x86_64-unknown-linux-gnu"
    }).to_string()).unwrap();
    let prompts = root.parent().unwrap().join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    let mut config = nous_ext::RuntimeConfig::new(root, artifacts);
    config.isolation.enabled = false;
    config.handles.insert("prompts".into(), nous_ext::HandleValue::dir(prompts));
    let runtime = nous_ext::Runtime::new(config);
    runtime.start_all();
    runtime
}

/// 真 git 仓（带 origin bare remote），pending 视图才有可信的 HEAD。
fn init_repo(root: &Path) {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t84")
            .env("GIT_AUTHOR_EMAIL", "t84@example.invalid")
            .env("GIT_COMMITTER_NAME", "t84")
            .env("GIT_COMMITTER_EMAIL", "t84@example.invalid")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".gitignore"), "artifacts/\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "seed"]);
    let remote = root.parent().unwrap().join(format!("{}-remote.git", root.file_name().unwrap().to_string_lossy()));
    Command::new("git")
        .args(["init", "-q", "--bare", &remote.to_string_lossy()])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    git(&["remote", "add", "origin", &remote.to_string_lossy()]);
}

fn app_with(root: &Path, trash: &Path) -> Router {
    let state = AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test("extend", root)
        .with_ext_admin(root, trash);
    router(state)
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let builder = if body.is_some() {
        builder.header("content-type", "application/json")
    } else {
        builder
    };
    let request = builder
        .body(Body::from(body.map(|v| serde_json::to_vec(&v).unwrap()).unwrap_or_default()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}

/// 判据 1：create 落目录 + 三文件，并进 #75 的 pending（未提交）。
#[tokio::test]
async fn create_writes_scaffold_and_shows_up_as_uncommitted_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("extend-trash"));

    let (status, body) = call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":"alpha"}))).await;
    assert_eq!(status, StatusCode::CREATED, "create 实测 {status:?} {body}");

    let dir = root.join("alpha");
    for file in ["nous.json", "README.md", "main.rs"] {
        assert!(dir.join(file).is_file(), "create 应落 {file}：{}", dir.display());
    }
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("nous.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], json!("alpha"), "清单名字就是目录名：{manifest}");

    let (_, pending) = call(&app, Method::GET, "/v1/repos/extend/pending", None).await;
    let units = pending["units"].as_array().unwrap();
    let unit = units.iter().find(|u| u["name"] == json!("alpha")).unwrap_or_else(|| panic!("pending 里没有 alpha：{pending}"));
    assert_eq!(unit["state"], json!("untracked"), "create 不自动 commit：{unit}");
    let head = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&root).output().unwrap();
    let seeded_head = String::from_utf8(head.stdout).unwrap().trim().to_string();
    let (status, body) = call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":"beta"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let head = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&root).output().unwrap();
    assert_eq!(String::from_utf8(head.stdout).unwrap().trim(), seeded_head, "create 不改 HEAD（不自动 commit）：{body}");

    // pending 的 diff / commit 三件直接复用 #75 的 repo 端点，不在 ext 里另建实现。
    let (status, diff) = call(&app, Method::GET, "/v1/repos/extend/pending/diff?path=alpha/nous.json", None).await;
    assert_eq!(status, StatusCode::OK, "pending/diff 复用 #75：{diff}");
    assert_eq!(diff["before"], json!(""), "新建文件的 diff.before 为空：{diff}");
    let after: Value = serde_json::from_str(diff["after"].as_str().unwrap()).unwrap();
    assert_eq!(after["name"], json!("alpha"), "diff.after 是真文件内容：{diff}");
}

/// 判据 2：duplicate 只改清单的 name，其余文件逐个 sha 相同。
#[tokio::test]
async fn duplicate_renames_manifest_and_keeps_every_other_file_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("extend-trash"));

    let (status, _) = call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":"alpha"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    // 放一份非清单文件，验证它也被逐字节复制（并验证 sha 确实来自文件而非代码）。
    std::fs::write(root.join("alpha/extra.txt"), "固定字节\n").unwrap();
    let (status, body) = call(&app, Method::POST, "/v1/extensions/duplicate", Some(json!({"name":"alpha","to":"beta"}))).await;
    assert_eq!(status, StatusCode::CREATED, "duplicate 实测 {status:?} {body}");

    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(root.join("beta/nous.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], json!("beta"), "新目录清单名字改了：{manifest}");
    for file in ["README.md", "main.rs", "extra.txt"] {
        let a = std::fs::read(root.join("alpha").join(file)).unwrap();
        let b = std::fs::read(root.join("beta").join(file)).unwrap();
        assert_eq!(a, b, "{file} 逐字节相同（sha 必然相同）");
    }
}

/// 判据 5：词表重命名一个类别，所有挂它的扩展（两个）的 category 跟着改。
#[tokio::test]
async fn category_rename_rewrites_every_extension_that_uses_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("extend-trash"));

    for name in ["alpha", "beta"] {
        let (status, _) = call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":name}))).await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (_, _) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"add","name":"old"}))).await;
    for name in ["alpha", "beta"] {
        let (status, body) = call(&app, Method::POST, "/v1/extensions/category", Some(json!({"name":name,"category":"old"}))).await;
        assert_eq!(status, StatusCode::OK, "挂类别 {name}：{body}");
    }
    let (status, body) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"rename","name":"old","to":"new"}))).await;
    assert_eq!(status, StatusCode::OK, "词表改名：{body}");
    assert_eq!(body["categories"], json!(["new"]));
    let affected = body["affected"].as_array().unwrap();
    assert_eq!(affected.len(), 2, "两个扩展都被改写：{body}");

    for name in ["alpha", "beta"] {
        let manifest: Value = serde_json::from_str(&std::fs::read_to_string(root.join(name).join("nous.json")).unwrap()).unwrap();
        assert_eq!(manifest["category"], json!("new"), "{name} 的 category 跟词表改名：{manifest}");
    }
    let words = serde_json::from_str::<Value>(&std::fs::read_to_string(root.join("categories.json")).unwrap()).unwrap();
    assert_eq!(words, json!(["new"]), "词表落 categories.json，随私库发布");
}

/// 判据 4：核心扩展（nous / skills / prompts）不许本地删。
#[tokio::test]
async fn core_extensions_cannot_be_uninstalled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("extend-trash"));

    for name in ["nous", "skills", "prompts"] {
        let core = root.join(name);
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(core.join("nous.json"), json!({"name":name,"version":"0.1.0","protocol":3,"file":name,"entry":name,"sections":[{"key":"x","tag":"x"}]}).to_string()).unwrap();
        let (status, body) = call(&app, Method::POST, "/v1/extensions/uninstall", Some(json!({"name":name}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "核心扩展 {name} 必须 400：{body}");
        assert!(core.is_dir(), "核心扩展 {name} 的目录还在");
    }
}

/// 判据 6：有会话用了扩展就回该会话 id（usage 复用 #85 的扫描，不另写扫描器）。
#[tokio::test]
async fn usage_lists_the_session_that_uses_an_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("usage.db");
    let session = nous_durable::session::Session::open(&path).unwrap();
    session.commit(
        vec![nous_durable::StorageWrite::Conversation(json!({
            "id": 42, "extensions": ["alpha", "beta"]
        }))],
        nous_durable::session::CommitOptions::default(),
    ).await.unwrap();
    session.close().await.unwrap();

    let app = router(AppState::durable(&path).unwrap());
    let (status, body) = call(&app, Method::GET, "/v1/extensions?usage=1", None).await;
    assert_eq!(status, StatusCode::OK, "usage 实测 {status:?} {body}");
    assert_eq!(body["alpha"], json!([42]), "用了 alpha 的会话 id：{body}");
    assert_eq!(body["beta"], json!([42]), "用了 beta 的会话 id：{body}");
    let _ = std::fs::remove_file(&path);
}

/// 判据 3：uninstall 移进垃圾站并从清单消失；install 回来后 state=running（真装载）。
#[tokio::test]
async fn uninstall_trashes_source_and_install_restores_a_running_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let root = base.join("exts");
    let trash = base.join("data/extend-trash");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let runtime = install_runnable(&root, "alpha", &sample_ext_bin());
    assert_eq!(runtime.state_of("alpha").map(|s| s.as_str()), Some("normal"), "先真装载 alpha");
    let app = router(AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test("extend", &root)
        .with_ext_admin(&root, &trash)
        .with_ext(runtime.clone()));

    let (status, list) = call(&app, Method::GET, "/v1/extensions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["items"].as_array().unwrap().iter().any(|i| i["name"] == json!("alpha")), "卸载前清单有 alpha：{list}");

    let (status, body) = call(&app, Method::POST, "/v1/extensions/uninstall", Some(json!({"name":"alpha"}))).await;
    assert_eq!(status, StatusCode::OK, "uninstall 实测 {status:?} {body}");
    assert!(!root.join("alpha").exists(), "源码目录消失");
    let trashed = PathBuf::from(body["trashed"].as_str().unwrap());
    assert!(trashed.is_dir() && trashed.join("nous.json").is_file(), "垃圾站里有它：{body}");
    // 卸载过的**仍在清单里**，只是变成了 `absent`：界面上「安装」这个入口就靠它
    // （目录搬进垃圾站、源码根里没有，扫不出来就等于这条扩展消失了）。
    let (_, list) = call(&app, Method::GET, "/v1/extensions", None).await;
    let alpha = list["items"].as_array().unwrap().iter().find(|i| i["name"] == json!("alpha")).cloned();
    assert_eq!(alpha.as_ref().map(|i| i["kind"].clone()), Some(json!("absent")), "卸载后清单里是 absent：{list}");
    assert_eq!(alpha.as_ref().map(|i| i["state"].clone()), Some(json!("absent")), "{list}");
    let deleted: Value = serde_json::from_str(&std::fs::read_to_string(root.join("deleted-extensions.json")).unwrap()).unwrap();
    assert_eq!(deleted, json!(["alpha"]), "删除名单记下来，防 git pull 复活：{deleted}");

    // 模拟 git pull 把删掉的目录带回来：删除名单必须继续压住它。
    std::fs::create_dir_all(root.join("alpha")).unwrap();
    std::fs::copy(&trashed.join("nous.json"), root.join("alpha/nous.json")).unwrap();
    let (_, list) = call(&app, Method::GET, "/v1/extensions", None).await;
    let alpha = list["items"].as_array().unwrap().iter().find(|i| i["name"] == json!("alpha")).cloned();
    assert_eq!(alpha.as_ref().map(|i| i["kind"].clone()), Some(json!("absent")), "git pull 不能让删除名单里的扩展复活成 native：{list}");

    // 装回时清掉名单，目录才真的重新可见。
    let (status, body) = call(&app, Method::POST, "/v1/extensions/install", Some(json!({"name":"alpha"}))).await;
    assert_eq!(status, StatusCode::OK, "install 实测 {status:?} {body}");
    assert_eq!(body["from"], json!("trash"), "从垃圾站装回：{body}");
    assert_eq!(body["state"], json!("running"), "装回后真装载成 running：{body}");
    assert!(root.join("alpha/nous.json").is_file(), "源码回到私库");
    let (_, list) = call(&app, Method::GET, "/v1/extensions", None).await;
    let item = list["items"].as_array().unwrap().iter().find(|i| i["name"] == json!("alpha")).unwrap();
    assert_eq!(item["state"], json!("running"), "清单投影也是 running：{item}");
    assert_eq!(item["kind"], json!("native"), "装回来了就不是 absent 了：{item}");
}

/// review 8：垃圾站清空、但私库 HEAD 里还有 → install 从 HEAD 装回，state=running。
#[tokio::test]
async fn install_falls_back_to_head_when_trash_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let root = base.join("exts");
    let trash = base.join("data/extend-trash");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);

    // 先把 alpha 提交进 HEAD，再从工作树移走（模拟干净 checkout + 卸载后垃圾站也清了）。
    std::fs::create_dir_all(root.join("alpha")).unwrap();
    std::fs::write(root.join("alpha/nous.json"), json!({
        "name": "alpha", "version": "0.1.0", "protocol": 3,
        "file": "exts-handle", "entry": "exts-handle",
        "sections": [{"key":"prompts","tag":"prompts"}],
        "handles": [], "summary": "#84 head fallback", "category": "sample"
    }).to_string()).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git").args(args).current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t84").env("GIT_AUTHOR_EMAIL", "t84@example.invalid")
            .env("GIT_COMMITTER_NAME", "t84").env("GIT_COMMITTER_EMAIL", "t84@example.invalid")
            .output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["add", "alpha"]);
    git(&["commit", "-qm", "seed alpha"]);
    std::fs::write(root.join("deleted-extensions.json"), json!(["alpha"]).to_string()).unwrap();
    // 工作树里没有 alpha，垃圾站也是空的。
    std::fs::remove_dir_all(root.join("alpha")).unwrap();
    let _ = std::fs::remove_dir_all(&trash);

    // 产物元数据（与 HEAD 里的 nous.json 的 file/entry 对应）。
    let runtime = install_runnable_artifacts_only(&root, &sample_ext_bin());
    let app = router(AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test("extend", &root)
        .with_ext_admin(&root, &trash)
        .with_ext(runtime.clone()));

    let (status, body) = call(&app, Method::POST, "/v1/extensions/install", Some(json!({"name":"alpha"}))).await;
    assert_eq!(status, StatusCode::OK, "HEAD 回退 install 实测 {status:?} {body}");
    assert_eq!(body["from"], json!("head"), "从 HEAD 装回：{body}");
    assert_eq!(body["state"], json!("running"), "装回后真装载：{body}");
    assert!(root.join("alpha/nous.json").is_file(), "源码回到私库");

    // 两边都没有 → 明确错误（404 类）。
    let (status, body) = call(&app, Method::POST, "/v1/extensions/install", Some(json!({"name":"nowhere"}))).await;
    assert!(status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
        "两边都没有要给明确错误，不该 200：{status:?} {body}");
    assert!(body["error"].is_string(), "错误体形状：{body}");
}

/// 判据：词表 `remove` 不只是从表里划掉 —— 挂着它的扩展清单里的 `category` 也要清掉，
/// 否则界面上会留一个「词表里没有的类别」，等于两条真相。
#[tokio::test]
async fn categories_remove_also_clears_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("extend-trash"));

    call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":"alpha"}))).await;
    call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":"beta"}))).await;
    call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"add","name":"旧类别"}))).await;
    call(&app, Method::POST, "/v1/extensions/category", Some(json!({"name":"alpha","category":"旧类别"}))).await;

    let (status, body) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"remove","name":"旧类别"}))).await;
    assert_eq!(status, StatusCode::OK, "词表删除：{body}");
    assert_eq!(body["categories"], json!([]), "词表空了：{body}");
    assert_eq!(body["affected"], json!(["alpha"]), "只有挂着它的那条被改写：{body}");

    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(root.join("alpha/nous.json")).unwrap()).unwrap();
    assert!(manifest.get("category").is_none(), "清单里的 category 要摘掉：{manifest}");
    let beta: Value = serde_json::from_str(&std::fs::read_to_string(root.join("beta/nous.json")).unwrap()).unwrap();
    assert!(beta.get("category").is_none(), "没挂类别的别长出一个空串：{beta}");
    let (_, list) = call(&app, Method::GET, "/v1/extensions/categories", None).await;
    assert_eq!(list["categories"], json!([]), "GET 也是空：{list}");
}

/// 词表操作对不存在的名字要报错，不许静默成功 —— 静默成功等于「我改了」但什么都没改。
#[tokio::test]
async fn categories_unknown_name_and_op_are_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("extend-trash"));

    let (status, body) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"rename","name":"不存在","to":"新"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "改名不存在的类别要 400：{body}");
    let (status, body) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"rename","name":"x"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "rename 缺 to 要 400：{body}");
    let (status, body) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"explode","name":"x"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知 op 要 400：{body}");
    // `add` 幂等：重复加同一类别不报错、也不长出两条。
    call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"add","name":"a"}))).await;
    let (status, body) = call(&app, Method::POST, "/v1/extensions/categories", Some(json!({"op":"add","name":"a"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["categories"], json!(["a"]), "重复 add 不许重复：{body}");
}

/// 判据：垃圾站能看见 + 能按条目彻底删。
/// 卸载不是删除 —— 目录还在 `data/extend-trash/` 里，界面得能列出来，也得能真的扔掉。
#[tokio::test]
async fn trash_lists_uninstalled_entries_and_purge_removes_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let trash = tmp.path().join("data/extend-trash");
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &trash);

    let (_, empty) = call(&app, Method::GET, "/v1/extensions/trash", None).await;
    assert_eq!(empty["items"], json!([]), "空垃圾站回空表：{empty}");

    call(&app, Method::POST, "/v1/extensions/create", Some(json!({"name":"alpha"}))).await;
    let (_, body) = call(&app, Method::POST, "/v1/extensions/uninstall", Some(json!({"name":"alpha"}))).await;
    let entry = body["trashed"].as_str().unwrap().rsplit('/').next().unwrap().to_string();

    let (status, list) = call(&app, Method::GET, "/v1/extensions/trash", None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "垃圾站里有一条：{list}");
    assert_eq!(items[0]["name"], json!("alpha"), "名字从条目名切回来：{list}");
    assert_eq!(items[0]["entry"], json!(entry));
    assert!(items[0]["trashed_at"].as_u64().unwrap() > 0, "条目名里的毫秒要解析出来：{list}");
    assert_eq!(items[0]["has_manifest"], json!(true), "清单还在：{list}");

    let (status, body) = call(&app, Method::POST, "/v1/extensions/trash/purge", Some(json!({"entry":entry}))).await;
    assert_eq!(status, StatusCode::OK, "彻底删：{body}");
    assert_eq!(body["removed"], json!(true));
    assert!(!trash.join(&entry).exists(), "目录真的没了");
    let (_, list) = call(&app, Method::GET, "/v1/extensions/trash", None).await;
    assert_eq!(list["items"], json!([]), "删完就空：{list}");

    // 删掉的这份再装不回来（垃圾站没了、HEAD 里也没有），但 404/400 要响得明显。
    let (status, body) = call(&app, Method::POST, "/v1/extensions/install", Some(json!({"name":"alpha"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "垃圾站和 HEAD 都没有 → 400：{body}");
    let (status, body) = call(&app, Method::POST, "/v1/extensions/trash/purge", Some(json!({"entry":entry}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "同一份删两次 400：{body}");
}

/// 条目名是外部输入：`../` 之类的必须被挡在垃圾站外。
#[tokio::test]
async fn trash_purge_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("extends");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    let _env = EnvGuard::new(&[("NOUS_EXTEND", root.clone())]);
    let app = app_with(&root, &tmp.path().join("data/extend-trash"));

    for entry in ["../extends", "..", "a/b"] {
        let (status, body) = call(&app, Method::POST, "/v1/extensions/trash/purge", Some(json!({"entry":entry}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{entry} 必须 400：{body}");
    }
    assert!(root.is_dir(), "私库根没被铲");
}
