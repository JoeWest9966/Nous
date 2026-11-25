//! 判据：进程读的 token 文件 = `NOUS_TOKEN_FILE`，**不是** `$HOME/.config/nous/token`。
//!
//! 为什么值得一条判据：`deploy/nous-app.service` 把 token 放在 `/etc/nous/token`
//! （`scripts/deploy.sh` 生成、`root:nous` `0640`），而默认路径按 `$HOME` 算。
//! 两边猜的不是同一个文件，后果是「运维轮转了钥匙，进程读的是另一把」——
//! 表现成认证静默失效，或者服务怎么都起不来。
//!
//! 负向哨兵：把 `auth::token_path()` 改回 `default_token_path()`，第二个断言必红。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn bin() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` 由 cargo 为集成测试注入；不用手拼 target 路径（拼错一层就静默 NotFound）。
    PathBuf::from(env!("CARGO_BIN_EXE_nous-app"))
}

fn curl(args: &[&str]) -> String {
    let out = Command::new("curl").args(["-sS", "-m", "5"]).args(args).output().expect("curl 可用");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn status(url: &str, token: Option<&str>) -> u16 {
    let header = token.map(|t| format!("authorization: Bearer {t}"));
    let mut args: Vec<&str> = vec!["-o", "/dev/null", "-w", "%{http_code}"];
    if let Some(h) = header.as_deref() {
        args.extend(["-H", h]);
    }
    args.push(url);
    curl(&args).trim().parse().unwrap_or(0)
}

struct Server {
    child: Child,
    url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 起一个真进程：`NOUS_TOKEN_FILE` 指向 `token_file`，`$HOME` 里另放一把 `home_token`。
fn start(dir: &Path, token_file: &Path, home_token: Option<&str>) -> Server {
    std::fs::create_dir_all(dir.join(".config/nous")).unwrap();
    std::fs::write(token_file, "from-nous-token-file\n").unwrap();
    if let Some(value) = home_token {
        std::fs::write(dir.join(".config/nous/token"), format!("{value}\n")).unwrap();
    }
    let mut child = Command::new(bin())
        .current_dir(dir)
        .env("HOME", dir)
        .env("NOUS_DB", dir.join("nous.db"))
        .env("NOUS_LISTEN", "127.0.0.1:0")
        .env("NOUS_TOKEN_FILE", token_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("nous-app 起得来");
    let stdout = child.stdout.take().expect("piped");
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut url = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            assert!(Instant::now() < deadline, "服务没在 10 秒内打印 listening on");
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("listening on ") {
            url = format!("http://{rest}");
            break;
        }
    }
    // stdout 被搬进线程不划算：这里拿到的两行就够，剩下的不读（管道有余量）。
    std::mem::forget(reader);
    Server { child, url }
}

#[test]
fn the_process_reads_the_token_at_nous_token_file_not_under_home() {
    let dir = std::env::temp_dir().join(format!("nous-token-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let server = start(&dir, &dir.join("etc-token"), Some("from-home-file"));

    // ① `NOUS_TOKEN_FILE` 里那把能用。
    assert_eq!(status(&format!("{}/v1/models", server.url), Some("from-nous-token-file")), 200);

    // ② `$HOME/.config/nous/token` 里那把**不认**（认了就是读错了文件）。
    assert_eq!(status(&format!("{}/v1/models", server.url), Some("from-home-file")), 401);

    // ③ 不带 header 也是 401（认证确实挂上了，不是「碰巧放行」）。
    assert_eq!(status(&format!("{}/v1/models", server.url), None), 401);

    let _ = std::fs::remove_dir_all(&dir);
}
