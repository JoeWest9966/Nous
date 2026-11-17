//! 真 HTTP：起 `nous-server` 子进程，或在本进程里起一个真监听。
//!
//! 判据要的是**真端点**（EX-53 / EX-54 / EX-62 / EX-63），不是路由层单测。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use nous_server::AppState;

/// 起一个真的 `nous-server` 子进程，返回它打印出来的地址。
pub fn spawn(db: &Path) -> (SocketAddr, Child) {
    let binary = std::env::current_exe()
        .expect("exe")
        .parent()
        .expect("parent")
        .join("nous-server");
    let mut child = Command::new(binary)
        .env("NOUS_DB", db)
        .env("NOUS_SERVER_ADDR", "127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn nous-server");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("listening line");
    let addr = line
        .trim()
        .strip_prefix("listening on ")
        .expect("listening line")
        .parse()
        .expect("addr");
    (addr, child)
}

/// 一次阻塞的 GET。
pub fn get(addr: SocketAddr, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(addr).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        addr
    )
    .ok()?;
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((status, body.to_string()))
}

/// 连上 SSE 并读到第一条匹配的 `data:` 帧。`timeout` 到就返回 None。
pub fn sse_first<F: Fn(&str) -> bool>(
    addr: SocketAddr,
    path: &str,
    timeout: std::time::Duration,
    predicate: F,
) -> Option<String> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream
        .set_read_timeout(Some(timeout))
        .ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
        addr
    )
    .ok()?;
    let reader = BufReader::new(stream);
    for line in reader.lines().map_while(Result::ok) {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if predicate(data) {
            return Some(data.to_string());
        }
    }
    None
}

/// 在本进程里起一个真监听（`AppState` 与我们共享，推送钩子才连得上）。
pub fn serve_in_process(state: Arc<AppState>) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let addr = listener.local_addr().expect("addr");
            tx.send(addr).expect("send addr");
            let _ = axum::serve(listener, nous_server::router(state)).await;
        });
    });
    let addr = rx.recv().expect("addr");
    (addr, handle)
}
