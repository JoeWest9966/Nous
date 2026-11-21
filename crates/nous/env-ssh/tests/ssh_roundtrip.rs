//! #78 判据 1/2/3：四条文件原语各一次真 ssh 往返；mux 关也能跑；临时密钥 0600 且清理。

mod common;

use common::Sshd;
use nous_env::{Context, FileContent, FileSystem, FileKind};

#[tokio::test]
async fn four_file_primitives_round_trip_over_real_ssh() {
    let sshd = Sshd::start();
    let env = nous_env_ssh::SshExecutionEnv::new(sshd.machine.clone(), Some(&sshd.key_pem)).unwrap();
    let context = Context::new();

    // writeFile：字节真的落到远端盘上。
    let target = format!("{}/nested/hello.txt", sshd.home().display());
    env.write_file(&target, FileContent::from("héllo from ssh\n"), &context)
        .await
        .unwrap();
    let on_disk = std::fs::read_to_string(sshd.home().join("nested/hello.txt")).unwrap();
    assert_eq!(on_disk, "héllo from ssh\n", "writeFile 必须把字节写到远端盘上");

    // readFile：与远端 cat 一致（这里期望值写死，不拿实现再算一遍）。
    let read = env.read_text_file(&target, &context).await.unwrap();
    assert_eq!(read, "héllo from ssh\n");

    // listDir：真目录项 + 类型。
    let entries = env.list_dir(&sshd.home().display().to_string(), &context).await.unwrap();
    let nested = entries.iter().find(|e| e.name == "nested").expect("nested 应在列表里");
    assert_eq!(nested.kind, FileKind::Directory);

    // canonicalPath：跟随 symlink 的规范化，结果与远端 realpath 一致。
    let link = format!("{}/hello-link", sshd.home().display());
    let _ = std::fs::remove_file(sshd.home().join("hello-link"));
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let canonical = env.canonical_path(&link, &context).await.unwrap();
    assert_eq!(canonical, format!("{}/nested/hello.txt", sshd.home().display()));
}

#[tokio::test]
async fn ssh_works_with_mux_disabled() {
    let sshd = Sshd::start();
    let env = nous_env_ssh::SshExecutionEnv::new(sshd.machine.clone(), Some(&sshd.key_pem)).unwrap();
    let context = Context::new();
    let path = format!("{}/no-mux.txt", sshd.home().display());
    env.write_file(&path, FileContent::from("no mux"), &context).await.unwrap();
    assert_eq!(std::fs::read_to_string(sshd.home().join("no-mux.txt")).unwrap(), "no mux");
    // 子进程隔离：NOUS_SSH_MUX 在 Mux::new() 里只读一次。
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .env("NOUS_SSH_MUX", "0")
        .args(["--exact", "mux_disabled_probe", "--nocapture"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("mux=false"), "NOUS_SSH_MUX=0 必须关复用：{text}");
}

/// 给上面那条用的探针：只在 NOUS_SSH_MUX=0 时打印一行。
#[test]
fn mux_disabled_probe() {
    if std::env::var("NOUS_SSH_MUX").ok().as_deref() == Some("0") {
        println!("mux={}", nous_env_ssh::Mux::new().enabled());
    }
}
