//! `schema-too-new` 的进程级判据：库比代码新 → **退出码 78** + `refuse: schema-too-new`。
//!
//! 为什么必须在真进程上验：`StorageError::is_schema_too_new()` 早就写好了，但没人接上，
//! 于是它走泛化错误路径退出 **2** —— 看起来"崩了"，systemd 按 `Restart=on-failure`
//! 每 3 秒拉一次（`RestartPreventExitStatus=78` 挡不住 2）。这条在 j1900 上真机抓到的
//! 现象，任何"只测函数"的判据都看不见，所以这里起真进程、断言真退出码。
//!
//! 负向哨兵：删掉 `main.rs` 里 `if error.is_schema_too_new() { RunError::Refused(..) }`
//! 那一段 → 退出码变 2、stderr 里没有 `refuse:` → 本测试红。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 起一次服务、等到 `listening on` 后杀掉：目的只是让程序把库建出来。
struct Booted {
    child: Child,
}

impl Drop for Booted {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn bin() -> PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .to_string_lossy()
            .into_owned()
    });
    Path::new(&target).join("debug/nous-app")
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nous-refusal-{tag}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn command(dir: &Path, db: &Path) -> Command {
    let mut cmd = Command::new(bin());
    cmd.current_dir(dir)
        .env("NOUS_DB", db)
        // 端口 0：不占固定端口；回环 + 没有 token 文件 = 允许启动。
        .env("NOUS_LISTEN", "127.0.0.1:0")
        .env("NOUS_TOKEN_FILE", dir.join("no-token"))
        .env("HOME", dir)
        .env_remove("NOUS_ADOPT_LEGACY")
        .env_remove("NOUS_PROVIDER")
        .env_remove("NOUS_ORIGIN")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn boot_once(dir: &Path, db: &Path) -> Booted {
    let mut child = command(dir, db).spawn().expect("起 nous-app");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        line.clear();
        use std::io::BufRead;
        let n = reader.read_line(&mut line).expect("读 stdout");
        if n > 0 && line.contains("listening on") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "30 秒内没等到 listening 行（库建不出来？）"
        );
        assert!(n > 0, "进程在打印 listening 之前就退出了");
    }
    Booted { child }
}

#[test]
fn a_database_newer_than_the_code_refuses_with_exit_78_not_2() {
    let dir = workdir("schema-too-new");
    let db = dir.join("nous.db");

    // ① 先让程序把库建出来（真启动一次、拿到 listening）。
    boot_once(&dir, &db);

    // ② 把 durable 的版本推到「代码看不懂」那一版。这里用真 SQL，不调我们的迁移代码 ——
    //    判据要模拟的是「别的版本写过这个库」，而不是「我们的代码改了版本号」。
    {
        let conn = rusqlite::Connection::open(&db).expect("打开库");
        let bumped: u32 = conn
            .query_row(
                "select version + 1 from durable_schema where singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("读 durable_schema.version");
        conn.execute(
            "update durable_schema set version = ?1 where singleton = 1",
            [bumped],
        )
        .expect("推进版本");
    }

    // ③ 再起一次：必须「拒绝启动」——退出码 78（EX_CONFIG）、stderr 带 refuse: 前缀。
    let output = command(&dir, &db).output().expect("再起一次");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code().expect("正常退出（不是被信号杀）");

    assert_eq!(
        code, 78,
        "库比代码新必须走 EX_CONFIG(78)，实际 {code}；stderr：\n{stderr}"
    );
    assert!(
        stderr.contains("refuse: schema-too-new"),
        "stderr 必须带 `refuse: schema-too-new`（systemd 的 RestartPreventExitStatus 靠退出码，\
         人靠这一行），实际：\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
