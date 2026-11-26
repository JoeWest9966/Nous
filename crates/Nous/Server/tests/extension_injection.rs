//! 扩展的段进系统提示词（`render_turn` 接进真轮次）的判据。
//!
//! 接缝：真 Router（HTTP 建会话 / 起轮 / 读 entries）+ 真 `nous_extend::Runtime`（样例扩展
//! `exts-handle` 的真进程）+ 临时私库根。判据打的是字节级事实：扩展渲染出来的那一段
//! 逐字出现在本轮 `pi.system` 的 `sections.extensions` 里。
//!
//! 为什么要这条判据：2026-11-23 发现 `runtime.render_turn` 只有 `ext-probe` 在调，轮次
//! 驱动器没调 —— 扩展装得上、跑得稳，模型却一个字都看不到。这条测的就是那根线。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::session::Session;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

/// 扩展段文本里会出现的哨兵：文件名就是它（`exts-handle` 把句柄里的文件名投影成一行行）。
const SENTINEL: &str = "EXT-SENTINEL-9f3a.md";

fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root.pop();
    root
}

/// 编译样例扩展 `exts-handle`，返回二进制路径（与 #84 判据同法：问 cargo，不信固定路径）。
fn sample_ext_bin() -> PathBuf {
    let built = Command::new("cargo")
        .args(["build", "--quiet", "-p", "exts-handle", "--bin", "exts-handle"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo build exts-handle");
    assert!(
        built.status.success(),
        "样例扩展编译失败：{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let path = PathBuf::from(env!("CARGO_BIN_EXE_nous-app"))
        .parent()
        .unwrap()
        .join("exts-handle");
    assert!(path.exists(), "样例扩展二进制不存在：{}", path.display());
    path
}

/// 装一个真能跑起来的扩展：源码目录（`nous.json`）+ 产物（二进制 + `.json` 元数据），
/// 再把 `prompts` 句柄指到一个放着哨兵文件的目录。
fn start_extension(base: &Path, binary: &Path) -> Arc<nous_extend::Runtime> {
    let root = base.join("exts");
    let artifacts = base.join("artifacts");
    let handle_dir = base.join("handles/prompts");
    let dir = root.join("handle");
    for dir in [&dir, &artifacts, &handle_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        dir.join("nous.json"),
        json!({
            "name": "handle", "version": "0.1.0", "protocol": 3,
            "file": "exts-handle", "entry": "exts-handle",
            "sections": [{"key": "prompts", "tag": "prompts"}],
            "handles": ["prompts"], "summary": "#86 sample", "category": "sample"
        })
        .to_string(),
    )
    .unwrap();
    // 句柄内容：一个文件名就是哨兵的文件。扩展只能看文件名（EX-26 说它拿不到值）。
    std::fs::write(handle_dir.join(SENTINEL), "body").unwrap();

    let target = artifacts.join("exts-handle");
    std::fs::copy(binary, &target).unwrap();
    let sha = Command::new("sha256sum").arg(&target).output().unwrap();
    let hash = String::from_utf8(sha.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    std::fs::write(
        artifacts.join("exts-handle.json"),
        json!({
            "hash": hash, "builtFrom": "t86", "buildMachine": "container",
            "protocol": 3, "triple": "x86_64-unknown-linux-gnu"
        })
        .to_string(),
    )
    .unwrap();

    let mut config = nous_extend::RuntimeConfig::new(&root, &artifacts);
    // 判据进程不是 root 时 uid 隔离本来就会退化；这里显式关掉，只验「段进提示词」。
    config.isolation.enabled = false;
    config
        .handles
        .insert("prompts".into(), nous_extend::HandleValue::dir(handle_dir));
    let runtime = nous_extend::Runtime::new(config);
    runtime.start_all();
    assert_eq!(
        runtime.state_of("handle").map(|state| state.as_str()),
        Some("normal"),
        "样例扩展要先真装载起来，判据才算数"
    );
    runtime
}

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-ext-inject-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
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
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// 一条挂在真扩展运行时上的会话。
async fn fixture(tag: &str, with_extension: bool) -> (axum::Router, i64) {
    let db = temp_db(tag);
    let base = std::env::temp_dir().join(format!(
        "nous-ext-inject-root-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // 模板/技能各指一个空目录：本票只验扩展那一段，别让 #72/#83 的段混进来。
    std::env::set_var("NOUS_SKILLS", base.join("no-skills"));
    std::env::set_var("NOUS_PROMPTS", base.join("no-prompts"));

    let session = Arc::new(Session::open(&db).unwrap());
    let provider = nous_ai::providers::faux::FauxProvider::new(
        nous_ai::providers::faux::FauxScript::new().with_text("ok"),
    );
    let tools: Arc<BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>>> = Arc::new(BTreeMap::new());
    let runtime = nous_server::runtime::assemble_with_provider(provider, tools, session.clone());
    let mut state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(&db).unwrap(),
        nous_server::DurableCommands::with_session(
            session,
            nous_durable::session::ObserverRegistry::new(),
        ),
    )
    .with_models(nous_server::runtime::faux_catalog())
    .with_runtime(Arc::new(runtime));
    if with_extension {
        state = state.with_ext(start_extension(&base, &sample_ext_bin()));
    }
    let app = router(state);
    let (status, created) = call(
        &app,
        "POST",
        "/v1/sessions",
        Some(json!({"name": "", "skills": [], "template": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (app, created["id"].as_i64().unwrap())
}

/// 起一轮，等到 assistant 落库，再读回放段里的 `sections.extensions`。
async fn turn_extensions(app: &axum::Router, id: i64, request_id: &str) -> String {
    let (status, submitted) = call(
        app,
        "POST",
        &format!("/v1/sessions/{id}/messages"),
        Some(json!({"content": "hi", "requestId": request_id})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{submitted}");
    for _ in 0..200 {
        let (status, body) = call(app, "GET", &format!("/v1/sessions/{id}/entries?system=1"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["kind"] == json!("pi.assistant")))
        {
            // 最后一条带 extensions 段的 system 就是本轮生效的那份（覆盖式合并）。
            return body["items"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|item| item["kind"] == json!("pi.system"))
                .and_then(|item| item["model"][0]["sections"]["extensions"].as_str())
                .unwrap_or_default()
                .to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("轮次没落库");
}

/// 判据 1：装上的扩展渲染出来的段，逐字进本轮系统提示词。
#[tokio::test]
async fn installed_extension_sections_reach_the_system_prompt() {
    let (app, id) = fixture("one", true).await;
    let text = turn_extensions(&app, id, "r1").await;
    assert!(
        text.contains(SENTINEL),
        "判据 1：扩展段要含句柄里的文件名，实测：{text:?}"
    );
    assert!(text.contains("prompts: 1"), "判据 1：段文本是扩展自己的字节，实测：{text:?}");
}

/// 判据 2：没装扩展运行时（精简装配）时，这一轮照跑、只是没有 extensions 段 —— 不许因此失败。
#[tokio::test]
async fn without_an_extension_runtime_the_turn_still_runs() {
    let (app, id) = fixture("none", false).await;
    let text = turn_extensions(&app, id, "r2").await;
    assert!(text.is_empty(), "没有运行时就不该有 extensions 段：{text:?}");
}

/// 判据 3：同一段会话连起两轮，扩展都被问到（不是只接第一轮）。
#[tokio::test]
async fn every_turn_asks_the_extensions_again() {
    let (app, id) = fixture("twice", true).await;
    let first = turn_extensions(&app, id, "r3a").await;
    let second = turn_extensions(&app, id, "r3b").await;
    assert!(first.contains(SENTINEL), "第一轮要含扩展段：{first:?}");
    assert_eq!(first, second, "判据 3：同一扩展两轮逐字相同（段是纯函数）");
}
