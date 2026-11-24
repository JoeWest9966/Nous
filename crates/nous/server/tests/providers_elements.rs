//! 元素组的供应商 → 真 provider 的判据。
//!
//! 判据打在两个接缝上：
//! - `providers::build`：密钥引用（`$ENV` / `!cmd` / `secret:<id>`）解不出来就诚实跳过，
//!   诊断里点名，绝不把引用串当密钥发出去；
//! - 真 HTTP：`stream_simple` 按 `model.provider` 分派，答案来自元素包写的 `baseUrl`，
//!   请求头里是解出来的密钥。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_ai::{Context, Model, SimpleStreamOptions, StopReason};
use nous_durable::session::Session;
use nous_runtime::{ProviderRegistry, RuntimeConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

/// 假供应商：一个只回一句固定文本的 OpenAI completions SSE 服务。
struct MockProvider {
    address: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockProvider {
    async fn new(text: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let observed = hits.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                let mut request = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .map_or(0, |value| value.trim().parse().unwrap());
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                recorded
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request).to_ascii_lowercase());
                let body = format!(
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{text}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Self {
            address,
            hits,
            requests,
            task,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }
}

/// 一个供应商的 `ModelsConfig`：`apiKey` 原样放引用串。
fn config_with(providers: &[(&str, &str, &str)]) -> RuntimeConfig {
    let mut entries = serde_json::Map::new();
    for (id, base_url, api_key) in providers {
        entries.insert(
            (*id).to_string(),
            json!({
                "name": id,
                "baseUrl": base_url,
                "api": "openai-completions",
                "apiKey": api_key,
                "models": [{ "id": format!("{id}-model"), "name": id }],
            }),
        );
    }
    RuntimeConfig {
        models: serde_json::from_value(json!({ "providers": entries })).unwrap(),
        ..Default::default()
    }
}

fn model_of(provider: &str) -> Model {
    Model::new(
        format!("{provider}-model"),
        provider,
        nous_ai::Api::OpenaiCompletions,
        provider,
        "http://localhost:1",
    )
}

async fn answer(handle: &Arc<dyn nous_ai::Provider>, provider: &str) -> nous_ai::types::AssistantMessage {
    tokio::time::timeout(Duration::from_secs(3), async {
        handle
            .stream_simple(
                &model_of(provider),
                &Context::new(vec![]),
                &SimpleStreamOptions {
                    timeout: Some(Duration::from_secs(2)),
                    ..Default::default()
                },
            )
            .await
            .result()
            .await
    })
    .await
    .expect("轮次必须在 3 秒内收敛")
    .unwrap()
}

fn text_of(message: &nous_ai::types::AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            nous_ai::Content::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}

/// `$ENV` 引用：解出来就绑上，答案真从元素包写的 baseUrl 回来，密钥进请求头。
#[tokio::test]
async fn env_reference_is_resolved_and_the_answer_comes_from_the_configured_base_url() {
    let mock = MockProvider::new("来自元素组的回答").await;
    let config = config_with(&[("testprov", &mock.base_url(), "$TEST_KEY")]);
    let values = BTreeMap::from([("TEST_KEY".to_string(), "k-测试值".to_string())]);

    let (handle, notes) = nous_server::providers::build(&config, &ProviderRegistry::default(), &values);
    let handle = handle.expect("有密钥引用就该绑上");
    assert!(
        notes.iter().any(|note| note.contains("已绑定供应商：testprov")),
        "诊断要点名绑上的供应商：{notes:?}"
    );

    let message = answer(&handle, "testprov").await;
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(text_of(&message), "来自元素组的回答");
    let request = mock.requests.lock().unwrap()[0].clone();
    assert!(
        request.contains("authorization: bearer k-测试值"),
        "密钥要进请求头：{request}"
    );
    assert!(request.contains("\"model\":\"testprov-model\""), "{request}");
}

/// 两个供应商：分派认 `model.provider`，别人的端点一个请求都不该收到。
#[tokio::test]
async fn each_model_is_dispatched_to_its_own_provider() {
    let alpha = MockProvider::new("alpha 说").await;
    let beta = MockProvider::new("beta 说").await;
    let config = config_with(&[
        ("alpha", &alpha.base_url(), "$ALPHA_KEY"),
        ("beta", &beta.base_url(), "$BETA_KEY"),
    ]);
    let values = BTreeMap::from([
        ("ALPHA_KEY".to_string(), "a-key".to_string()),
        ("BETA_KEY".to_string(), "b-key".to_string()),
    ]);

    let (handle, notes) = nous_server::providers::build(&config, &ProviderRegistry::default(), &values);
    let handle = handle.expect("两家都有密钥就该都绑上");
    assert!(notes.iter().any(|note| note.contains("alpha") && note.contains("beta")), "{notes:?}");

    let message = answer(&handle, "beta").await;
    assert_eq!(text_of(&message), "beta 说");
    assert_eq!(beta.hits.load(Ordering::SeqCst), 1, "beta 该被打一次");
    assert_eq!(alpha.hits.load(Ordering::SeqCst), 0, "alpha 一个请求都不该收到");
}

/// 引用解不出来：整个供应商跳过，诊断点名，不许拿引用串当密钥。
#[test]
fn unresolvable_key_references_skip_the_provider_with_a_named_note() {
    let cases: Vec<(&str, &str)> = vec![
        ("$NOPE_KEY", "环境变量 NOPE_KEY 没有值"),
        ("secret:wb2api", "密钥库里没有 wb2api"),
        ("!exit 3", "退出码 3"),
        ("sk-plaintext", "明文"),
        ("", "没写 apiKey"),
    ];
    for (reference, expected) in cases {
        let config = config_with(&[("testprov", "http://127.0.0.1:1/v1", reference)]);
        let (handle, notes) = nous_server::providers::build(
            &config,
            &ProviderRegistry::default(),
            &BTreeMap::new(),
        );
        assert!(handle.is_none(), "`{reference}` 不该绑出供应商");
        assert!(
            notes.iter().any(|note| note.contains(expected)),
            "`{reference}` 的诊断要点名原因：{notes:?}"
        );
    }
}

/// `secret:<id>` 走宿主的密钥空间，`!cmd` 取命令输出：两条都解得出来就都绑上。
#[tokio::test]
async fn secret_and_command_references_resolve_through_the_host() {
    let mock = MockProvider::new("ok").await;
    let values = BTreeMap::from([("NOUS_SECRET_WB2API".to_string(), "secret-key".to_string())]);

    let secret = config_with(&[("wb2api", &mock.base_url(), "secret:wb2api")]);
    let (handle, notes) = nous_server::providers::build(&secret, &ProviderRegistry::default(), &values);
    assert!(handle.is_some(), "secret: 引用该解出来：{notes:?}");
    handle.unwrap().stream_simple(&model_of("wb2api"), &Context::new(vec![]), &SimpleStreamOptions::default()).await.result().await.unwrap();
    assert!(mock.requests.lock().unwrap()[0].contains("authorization: bearer secret-key"));

    let command = config_with(&[("cmdprov", &mock.base_url(), "!printf cmd-key")]);
    let (handle, notes) = nous_server::providers::build(&command, &ProviderRegistry::default(), &values);
    assert!(handle.is_some(), "!cmd 引用该解出来：{notes:?}");
    handle.unwrap().stream_simple(&model_of("cmdprov"), &Context::new(vec![]), &SimpleStreamOptions::default()).await.result().await.unwrap();
    assert!(mock.requests.lock().unwrap()[1].contains("authorization: bearer cmd-key"));
}

/// 分派不到的模型：回错误流（轮次照常收敛），不静默空转。
#[tokio::test]
async fn an_unbound_model_answers_with_an_error_stream() {
    let mock = MockProvider::new("不该被调用").await;
    let config = config_with(&[("alpha", &mock.base_url(), "$ALPHA_KEY")]);
    let values = BTreeMap::from([("ALPHA_KEY".to_string(), "a-key".to_string())]);
    let (handle, _) = nous_server::providers::build(&config, &ProviderRegistry::default(), &values);
    let handle = handle.unwrap();

    let message = answer(&handle, "ghost").await;
    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(
        message.error_message.unwrap_or_default().contains("ghost"),
        "错误要说清哪个供应商没绑定"
    );
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// 端到端：真装配（非 faux）走元素组的供应商，答案落进 entries
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 元素库：一个 `nous-models` 组 + 一个供应商包。
fn elements_repo(base_url: &str, api_key: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    std::fs::write(
        root.join("groups/nous-models.json"),
        json!({
            "id": "nous-models",
            "name": "Nous 模型入口",
            "packages": ["provider-e2e"],
            "default": "e2e/e2e-model",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("packages/provider-e2e.json"),
        json!({
            "id": "provider-e2e",
            "name": "E2E",
            "kind": "provider",
            "provider": "e2e",
            "baseUrl": base_url,
            "api": "openai-completions",
            "apiKey": api_key,
            "models": [{ "id": "e2e-model", "name": "E2E Model" }],
        })
        .to_string(),
    )
    .unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@e"]);
    git(root, &["config", "user.name", "T"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    tmp
}

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-providers-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn seed_conversation(path: &PathBuf) {
    let session = Session::open(path).unwrap();
    session
        .commit(
            vec![nous_durable::StorageWrite::Conversation(json!({ "id": 1 }))],
            nous_durable::session::CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(body) => Body::from(serde_json::to_vec(&body).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn read_rows(path: &PathBuf, table: &str) -> Vec<Value> {
    let output = std::process::Command::new("python3")
        .args([
            "-c",
            "import json,sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nrows=c.execute('select record from '+sys.argv[2] order by id').fetchall()\nprint(json.dumps([json.loads(r[0]) for r in rows]))",
            path.to_str().unwrap(),
            table,
        ])
        .output()
        .expect("python3 可用");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn wait_for(path: &PathBuf, table: &str, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let rows = read_rows(path, table);
        if rows.iter().any(&predicate) {
            return rows;
        }
        assert!(tokio::time::Instant::now() < deadline, "{table} 里没出现期望行：{rows:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 真装配 + 元素组：发一条消息，答案来自元素包写的供应商（不是离线 provider）。
#[tokio::test]
async fn a_real_turn_answers_through_the_elements_provider() {
    let mock = MockProvider::new("真供应商的回答").await;
    let elements = elements_repo(&mock.base_url(), "$E2E_KEY");
    let path = temp_db("e2e");
    seed_conversation(&path).await;

    let values = BTreeMap::from([("E2E_KEY".to_string(), "e2e-key".to_string())]);
    let session = Arc::new(Session::open(&path).unwrap());
    let assembled = nous_server::runtime::assemble_with_elements(
        std::env::temp_dir(),
        &values,
        session,
        elements.path(),
    )
    .expect("装配该成功");
    assert_eq!(assembled.provider_kind, "real");

    let state = nous_server::AppState::durable(&path)
        .unwrap()
        .with_runtime(Arc::new(assembled))
        .with_repos_for_test("elements", elements.path());
    let app = nous_server::router(state);

    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let entries = wait_for(&path, "entries", |entry| entry["kind"] == json!("pi.assistant")).await;
    let assistant = entries
        .iter()
        .find(|entry| entry["kind"] == json!("pi.assistant"))
        .expect("要有 assistant 条目");
    let recorded = serde_json::to_string(assistant).unwrap();
    assert!(recorded.contains("真供应商的回答"), "答案要落库：{recorded}");

    let request = mock.requests.lock().unwrap()[0].clone();
    assert!(request.contains("authorization: bearer e2e-key"), "{request}");
    assert!(request.contains("e2e-model"), "{request}");
}

/// 元素组的 default 指向一个没有密钥的供应商：轮次必须收敛成失败，不许静默空转。
#[tokio::test]
async fn a_turn_fails_loudly_when_the_provider_has_no_key() {
    let mock = MockProvider::new("不该被调用").await;
    let elements = elements_repo(&mock.base_url(), "$ABSENT_KEY");
    let path = temp_db("nokey");
    seed_conversation(&path).await;

    let session = Arc::new(Session::open(&path).unwrap());
    let assembled = nous_server::runtime::assemble_with_elements(
        std::env::temp_dir(),
        &BTreeMap::<String, String>::new(),
        session,
        elements.path(),
    )
    .expect("装配不该因为没密钥而失败");

    let state = nous_server::AppState::durable(&path)
        .unwrap()
        .with_runtime(Arc::new(assembled))
        .with_repos_for_test("elements", elements.path());
    let app = nous_server::router(state);

    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let submissions = wait_for(&path, "submissions", |row| {
        row["status"] == json!("failed") || row["status"] == json!("unanswered")
    })
    .await;
    assert!(
        submissions.iter().any(|row| row["status"] == json!("failed") || row["status"] == json!("unanswered")),
        "没密钥的供应商不许停在 queued：{submissions:?}"
    );
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0, "一个请求都不该发出去");
}
