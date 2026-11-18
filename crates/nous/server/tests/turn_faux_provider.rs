//! #51 的判据：NOUS_PROVIDER=faux 时真起 nous-app，走真 HTTP，落真库。
//!
//! 接缝是进程外的公开面：`POST /v1/sessions` → `POST /messages` → `GET /events`
//! 与 sqlite 里的 entry。判据不调用 `TurnDriver`、不读 provider 内部状态。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 票面钉死的回复正文（faux 只服务验收，不能配置）。
const FAUX_REPLY: &str = "provider=faux model=echo（假回复，别当真）";

struct Server {
    child: Child,
    addr: String,
    db: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.db.display()));
        }
    }
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nous-faux-{tag}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bin() -> PathBuf {
    // 和 verify.sh 一样认 CARGO_TARGET_DIR；测试自己把产物当被测对象，路径只有一个来源。
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .to_string_lossy()
            .into_owned()
    });
    Path::new(&target).join("debug/nous-app")
}

impl Server {
    fn start(tag: &str, faux: bool) -> Self {
        let dir = workdir(tag);
        let db = dir.join("nous.db");
        let out = dir.join("stdout.log");
        let err = dir.join("stderr.log");
        let mut cmd = Command::new(bin());
        cmd.current_dir(&dir)
            .env("NOUS_DB", &db)
            // 测试不许继承开发机上的真 token（`~/.config/nous/token`）：那个文件一存在，
            // `/v1/*` 就全量 401，这条判据会在别人的机器上莫名其妙地红。
            .env("HOME", &dir)
            .env("NOUS_TOKEN_FILE", dir.join("no-token"))
            .env("NOUS_LISTEN", "127.0.0.1:0")
            .stdout(Stdio::from(std::fs::File::create(&out).unwrap()))
            .stderr(Stdio::from(std::fs::File::create(&err).unwrap()));
        if faux {
            cmd.env("NOUS_PROVIDER", "faux");
        } else {
            cmd.env_remove("NOUS_PROVIDER");
        }
        let child = cmd.spawn().expect("nous-app 起得来");
        let deadline = Instant::now() + Duration::from_secs(10);
        let addr = loop {
            let text = std::fs::read_to_string(&out).unwrap_or_default();
            if let Some(line) = text.lines().find_map(|line| line.strip_prefix("listening on ")) {
                break line.trim().to_string();
            }
            assert!(Instant::now() < deadline, "启动超时：stderr={}", std::fs::read_to_string(&err).unwrap_or_default());
            std::thread::sleep(Duration::from_millis(25));
        };
        Self { child, addr, db }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

/// 同步 curl：判据在进程外，用系统 curl 而不是再写一个 HTTP client。
fn curl(args: &[&str]) -> String {
    let output = Command::new("curl")
        .args(["-sS", "--max-time", "5"])
        .args(args)
        .output()
        .expect("curl 可用");
    assert!(
        output.status.success(),
        "curl 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("HTTP 响应是 UTF-8")
}

fn post_json(url: &str, body: Value) -> (String, Value) {
    let body = serde_json::to_string(&body).unwrap();
    let out = curl(&[
        "-w",
        "\n%{http_code}",
        "-H",
        "content-type: application/json",
        "-d",
        &body,
        url,
    ]);
    let (payload, code) = out.rsplit_once('\n').expect("curl 带 http_code");
    (code.trim().to_string(), serde_json::from_str(payload).unwrap_or(Value::Null))
}

fn get_json(url: &str) -> Value {
    serde_json::from_str(&curl(&[url])).expect("GET 回 JSON")
}

fn wait_for_assistant(db: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        // 只读连接：测试观察落库结果，不充当第二个写者。
        let output = Command::new("python3")
            .args([
                "-c",
                "import json,sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nrows=c.execute(\"select record from entries where record like '%pi.assistant%' order by id\").fetchall()\nprint(json.dumps([json.loads(r[0]) for r in rows]))",
                db.to_str().unwrap(),
            ])
            .output()
            .expect("python3 可用");
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if let Some(entry) = rows.into_iter().last() {
            return entry;
        }
        assert!(Instant::now() < deadline, "assistant entry 10 秒内没落库");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sse_types_from(raw: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(raw)
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|frame| frame["type"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn faux_provider_answers_over_http_and_persists_a_done_entry() {
    let server = Server::start("faux-ok", true);

    let models = get_json(&server.url("/v1/models"));
    assert_eq!(models["default"], json!({ "provider": "faux", "modelId": "echo" }));
    assert!(models["items"].as_array().is_some_and(|items| items.iter().any(|item| {
        item["provider"] == json!("faux") && item["modelId"] == json!("echo")
    })), "faux/echo 必须在 /v1/models 可见：{models}");

    let (code, created) = post_json(&server.url("/v1/sessions"), json!({
        "model": { "provider": "faux", "modelId": "echo" },
    }));
    assert_eq!(code, "201", "建会话实测 {created}");
    let session = created["id"].as_i64().expect("带会话 id");

    // 先连 SSE（拿 snapshot），再发消息 —— 客户端心跳的三帧都在这条连接上。
    let sse_url = server.url(&format!("/v1/sessions/{session}/events"));
    // 等 snapshot 第一帧真的到了再 POST，否则 message_start 会落在订阅之前。
    // curl 的输出重定向到文件，另一边轮询文件内容（不是端口：accept 了不等于 handler 订阅了）。
    let frames_path = server.db.with_extension("sse");
    let frames = std::fs::File::create(&frames_path).unwrap();
    let sse = std::thread::spawn(move || {
        let status = Command::new("curl")
            .args(["-sS", "--max-time", "12", "-N", &sse_url])
            .stdout(Stdio::from(frames))
            .stderr(Stdio::null())
            .status()
            .expect("SSE 客户端可起");
        status.success() || status.code() == Some(28)
    });
    let snapshot_ready = Instant::now() + Duration::from_secs(5);
    loop {
        let text = std::fs::read_to_string(&frames_path).unwrap_or_default();
        if text.contains("\"snapshot\"") {
            break;
        }
        assert!(Instant::now() < snapshot_ready, "SSE 没吐 snapshot：{text}");
        std::thread::sleep(Duration::from_millis(20));
    }

    let (code, body) = post_json(
        &server.url(&format!("/v1/sessions/{session}/messages")),
        json!({ "content": "ping" }),
    );
    assert_eq!(code, "202", "实测 {body}");

    let entry = wait_for_assistant(&server.db);
    assert_eq!(entry["kind"], json!("pi.assistant"));
    assert_eq!(entry["reason"], json!("done"));
    assert_eq!(entry["model"][0]["content"][0]["text"], json!(FAUX_REPLY));
    assert_eq!(entry["submissionId"], json!(body["submissionId"]));

    assert!(sse.join().expect("SSE 线程收尾"), "SSE curl 异常退出");
    let types = sse_types_from(&std::fs::read(&frames_path).expect("SSE 帧可读"));
    for expected in ["message_start", "entry_appended", "message_end"] {
        assert!(types.iter().any(|seen| seen == expected), "缺 {expected}：{types:?}");
    }
    let allowed = [
        "snapshot", "message_start", "message_update", "message_end", "entry_appended",
        "submission", "task_failed", "tool_execution_start", "tool_execution_update",
    ];
    assert!(types.iter().all(|seen| allowed.contains(&seen.as_str())), "出现未知帧：{types:?}");
}

#[test]
fn without_no_provider_the_server_starts_and_capabilities_say_real() {
    let server = Server::start("default", false);
    let caps = get_json(&server.url("/v1/capabilities"));
    assert_eq!(
        caps["runtime"]["provider"]["kind"],
        json!("real"),
        "不设 NOUS_PROVIDER 时应是 real：{caps}"
    );
    assert_eq!(caps["runtime"]["provider"]["configured"], json!(false));

    let server = Server::start("faux-caps", true);
    let caps = get_json(&server.url("/v1/capabilities"));
    assert_eq!(caps["runtime"]["provider"]["kind"], json!("faux"));
    assert_eq!(caps["runtime"]["provider"]["configured"], json!(true));
    assert!(caps["runtime"]["provider"]["message"].as_str().is_some());
}
