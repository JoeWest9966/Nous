//! #81：扩展运行时的进程级判据（真 nous-app + 真扩展子进程 + 真 HTTP）。
//!
//! 这是「装载/熔断/热重载」的验收面，不是单元投影：pid 从 `/v1/extensions` 读，
//! kill 后用 `kill -0` 验证，reload 后 pid 必须变。所有载体都在临时目录里造，
//! 不碰私库、不碰 `/var/lib/nous`。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/Nous/Server
    path.pop(); // crates/nous
    path.pop(); // crates
    path
}

fn sample_ext_bin(name: &str) -> PathBuf {
    // 二进制路径只问 cargo：先显式编出样例（workspace 的 target-dir 可能在仓外），
    // 再从 `CARGO_BIN_EXE_nous-app` 的同目录取同名 sibling —— 和 judge.sh 的
    // `cargo build -p exts-*` 同一条生产路径，不猜 `<repo>/target/debug`。
    // 两个进程级测试可能并行：用目录锁串行化 cargo build，别让它们互抢 target 锁
    // 吃掉 start_server 的启动窗口。
    let lock = std::env::temp_dir().join(format!("nous-t81-build-{name}.lock"));
    while std::fs::create_dir(&lock).is_err() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let built = Command::new("cargo")
        .args(["build", "--quiet", "-p", name, "--bin", name])
        .current_dir(workspace_root())
        .output()
        .expect("cargo build 样例扩展");
    let _ = std::fs::remove_dir(&lock);
    assert!(
        built.status.success(),
        "样例扩展 {name} 编译失败：{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let path = PathBuf::from(env!("CARGO_BIN_EXE_nous-app"))
        .parent()
        .expect("nous-app binary has a parent")
        .join(name);
    assert!(
        path.exists(),
        "样例扩展二进制不存在：{}（cargo build -p {name} 没产出同名 bin）",
        path.display()
    );
    path
}

struct Server {
    child: Child,
    base: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").expect("bind port");
    socket.local_addr().unwrap().port()
}

fn http(url: &str, method: &str) -> Value {
    let output = Command::new("curl")
        .args(["-sS", "-X", method, "--max-time", "5", url])
        .output()
        .expect("curl exists");
    assert!(
        output.status.success(),
        "curl {method} {url} 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("不是 JSON：{error} {url} → {}", String::from_utf8_lossy(&output.stdout)))
}

fn alive(pid: u64) -> bool {
    // `kill -0` 对僵尸也成功；父死后子进程若没被收割会留僵尸，
    // 判据 4 要的是「零残留」，所以读 /proc 的状态，Z 也算死了。
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let state = stat.rsplit_once(") ").map(|(_, rest)| rest.chars().next()).flatten();
    state.is_some_and(|ch| ch != 'Z')
}

/// 造一个最小扩展目录 + artifacts + 真哈希元数据，形状与 ext-probe 沙盒一致。
fn install_sample(root: &Path, binary: &Path) {
    let exts = root.join("exts");
    let artifacts = root.join("artifacts");
    let prompts = root.join("prompts");
    for dir in [&exts, &artifacts, &prompts] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let ext = exts.join("probe-ext");
    std::fs::create_dir_all(&ext).unwrap();
    let manifest = json!({
        "name": "handle",
        "version": "0.1.0",
        "protocol": 3,
        "file": "exts-handle",
        "entry": "exts-handle",
        "sections": [{ "key": "prompts", "tag": "prompts" }],
        "handles": ["prompts"],
        "summary": "#81 sample",
        "category": "sample"
    });
    std::fs::write(ext.join("nous.json"), manifest.to_string()).unwrap();
    copy_artifact(&artifacts, binary, "exts-handle");
}

fn copy_artifact(artifacts: &Path, binary: &Path, file: &str) {
    let target = artifacts.join(file);
    std::fs::copy(binary, &target).unwrap();
    // 独立入口算 sha256（不读产品代码的哈希函数）。
    let output = Command::new("sha256sum").arg(&target).output().unwrap();
    let hash = String::from_utf8(output.stdout).unwrap();
    let hash = hash.split_whitespace().next().unwrap().to_string();
    std::fs::write(
        artifacts.join(format!("{file}.json")),
        json!({ "hash": hash, "builtFrom": "t81", "buildMachine": "container", "protocol": 3, "triple": "x86_64-unknown-linux-gnu" }).to_string(),
    )
    .unwrap();
}

/// 造一个「入口立刻 panic」的扩展：evil 样例的 crash 模式由扩展目录里的 `mode` 打开。
fn install_crashing_sample(root: &Path, binary: &Path) {
    install_sample(root, binary);
    let exts = root.join("exts");
    let ext = exts.join("probe-ext");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(ext.join("nous.json")).unwrap()).unwrap();
    manifest["name"] = json!("evil");
    manifest["file"] = json!("exts-evil");
    manifest["sections"] = json!([{ "key": "evil", "tag": "evil" }]);
    std::fs::write(ext.join("nous.json"), manifest.to_string()).unwrap();
    std::fs::write(ext.join("mode"), "exit-after-hello").unwrap();
    copy_artifact(&root.join("artifacts"), binary, "exts-evil");
}

fn start_server(root: &Path, port: u16) -> Server {
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nous-app"))
        .current_dir(&data)
        .env("NOUS_LISTEN", format!("127.0.0.1:{port}"))
        .env("NOUS_DB", data.join("nous.db"))
        .env("NOUS_EXTEND", root.join("exts"))
        .env("NOUS_SKILLS", root.join("skills"))
        .env("NOUS_PROMPTS", root.join("prompts"))
        // 指到临时目录里一个不存在的文件：`env_remove` 会退回到 `$HOME/.config/nous/token`
        // （开发机上真有），于是每个请求 401，判据变成「这台机器有没有 token 文件」。
        .env("NOUS_TOKEN_FILE", root.join("no-token"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn nous-app");
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(value) = std::panic::catch_unwind(|| http(&format!("{base}/v1/device"), "GET")) {
            if value.get("version").is_some() {
                return Server { child, base };
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    panic!("nous-app 60 秒内没起来：{base}");
}

fn extension(body: &Value, name: &str) -> Value {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == json!(name))
        .cloned()
        .unwrap_or_else(|| panic!("清单里没有扩展 {name}：{body}"))
}

/// 判据 4 的第三级兜底（静态命中行）：扩展 spawn 必须装 PDEATHSIG 且关掉 spawn 竞态。
/// 进程级判据里父死子退还可能由管道 EOF 达成，单独打不中这一级；
/// 这里直接看源码命中行，把它和「管道 EOF」「Runtime::drop」两级区分开。
#[test]
fn spawn_installs_parent_death_signal_and_closes_race() {
    let source = std::fs::read_to_string(
        workspace_root().join("crates/Nous/Ext/src/spawn.rs"),
    )
    .unwrap();
    // 命中行必须落在真执行路径上：`if false && prctl(...)`、`cfg!(never) && …`
    // 都含字符串却不装信号 —— 静态判据必须把它们判红（负向哨兵）。
    let prctl_line = source
        .lines()
        .find(|line| line.contains("libc::prctl(libc::PR_SET_PDEATHSIG"))
        .unwrap_or("");
    assert!(
        prctl_line.contains("if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0"),
        "PR_SET_PDEATHSIG 必须在一个真执行的 if 条件里，不许只出现在注释/短路表达式：{prctl_line}"
    );
}

/// 判据 2：扩展崩了进熔断，state=crashed + error 非空，主进程仍 200。
/// 不直接杀进程 —— 用 `evil` 的 crash 模式经真 section 调用触发，走同一条熔断阶梯。
#[test]
fn crashing_extension_trips_breaker_and_host_stays_up() {
    let root = tempfile::tempdir().unwrap();
    install_crashing_sample(root.path(), &sample_ext_bin("exts-evil"));
    let port = free_port();
    let server = start_server(root.path(), port);

    let body = http(&format!("{}/v1/extensions", server.base), "GET");
    let item = extension(&body, "evil");
    // 入口 panic 在握手/握手后第一段就暴露；items 的观测必须说崩了。
    assert_eq!(item["state"], json!("crashed"), "崩溃扩展必须可见：{item}");
    assert!(
        item["error"].as_str().is_some_and(|text| !text.is_empty()),
        "crashed 必须有非空 error：{item}"
    );
    // 主进程没被拖死。
    let device = http(&format!("{}/v1/device", server.base), "GET");
    assert!(device.get("version").is_some(), "扩展崩溃后 nous-app 仍 200：{device}");
}

#[test]
fn loaded_extension_is_visible_reload_changes_pid_and_parent_death_reaps_it() {
    let root = tempfile::tempdir().unwrap();
    install_sample(root.path(), &sample_ext_bin("exts-handle"));
    let port = free_port();
    let mut server = start_server(root.path(), port);

    // 判据 1：真清单里有它，state=running，pid>0。
    let body = http(&format!("{}/v1/extensions", server.base), "GET");
    let item = extension(&body, "handle");
    assert_eq!(item["state"], json!("running"), "{item}");
    let pid = item["pid"].as_u64().expect("pid 是数字");
    assert!(pid > 0);
    assert_eq!(item["loaded"], json!(true));
    assert!(alive(pid), "扩展进程 {pid} 应该活着");

    // 判据 3：reload 真换进程，pid 变了。
    let reloaded = http(&format!("{}/v1/extensions/reload", server.base), "POST");
    assert!(
        reloaded["reloaded"].as_array().is_some_and(|items| items.contains(&json!("handle"))),
        "reload 没重装 handle：{reloaded}"
    );
    let body = http(&format!("{}/v1/extensions", server.base), "GET");
    let new_pid = extension(&body, "handle")["pid"].as_u64().expect("pid 是数字");
    assert_ne!(new_pid, pid, "reload 后 pid 必须变（真重装，不是原地假装）");

    // 判据 4：父进程退出 → 扩展子进程也退出（PR_SET_PDEATHSIG）。
    // 用 SIGKILL 杀父进程，确保不是 Rust 的 Drop 兜底在起作用。
    unsafe {
        libc::kill(server.child.id() as i32, libc::SIGKILL);
    }
    let _ = server.child.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(new_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(new_pid), "父进程退出后扩展进程 {new_pid} 还有残留");
}
