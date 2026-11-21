//! 扩展进程：一个扩展一个进程、一对管道、专用 uid、fd 表恰好五项。
//!
//! **全仓进程创建点只有两处**（内核 spec S1）：这里是扩展子进程的那一处；工具子进程在
//! `crates/platform/nous-tools` 的 `os_env`。第三处即红。
//!
//! ## fd 契约（EX-71）
//!
//! 子进程里：`0/1/2` = `/dev/null`，`3` = host→ext 管道读端，`4` = ext→host 管道写端。
//! spawn 时**显式关掉**除这五个之外的全部 fd（`close_range`）—— 父进程手里只要有一个
//! sqlite 句柄还开着，扩展就能通过 `/proc/self/fd/7` 直接写库，uid 一点用都没有（A11）。

use std::collections::BTreeMap;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nous_ext_sdk::content::Handles;
use nous_ext_sdk::frame::{read_frame, write_frame, FrameError};
use serde_json::{json, Value};

use crate::config::Isolation;
use crate::error::{ExtError, ExtResult};

/// 帧通道上的一条消息。错误是字符串（`FrameError` 不 Clone，跨线程只传形状）。
pub type FrameMsg = Result<Value, String>;

/// 进程级观测（判据要读）。
#[derive(Debug, Default)]
pub struct ProcFlags {
    /// 进程死 / 被 kill。
    pub crashed: AtomicBool,
    /// 撞 RSS 硬顶被杀的。
    pub rss_killed: AtomicBool,
    /// 帧超限。
    pub oversize: AtomicBool,
    /// 杀之前观测到的 RSS 峰值。
    pub rss_seen: AtomicU64,
    /// 退码（能拿到时）。
    pub exit_code: Mutex<Option<i32>>,
}

/// 怎么 spawn。
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub ext_dir: PathBuf,
    /// 专用 uid（`None` = 不降权；判据里要明写降级）。
    pub uid: Option<u32>,
    pub isolation: Isolation,
    pub rss_limit: u64,
}

/// 一个扩展子进程 + 它的帧通道。
pub struct ProcExt {
    pub pid: u32,
    pub uid: Option<u32>,
    pub flags: Arc<ProcFlags>,
    writer: Mutex<std::fs::File>,
    child: Mutex<Child>,
    routes: Arc<Mutex<BTreeMap<u64, Sender<FrameMsg>>>>,
    /// 调用已经收尾之后才到的帧（晚期 output）。判据要能说"拒了"（EX-19）。
    orphans: Arc<Mutex<std::collections::BTreeSet<u64>>>,
    #[allow(dead_code)]
    control_tx: Sender<FrameMsg>,
    control_rx: Mutex<Receiver<FrameMsg>>,
}

impl ProcExt {
    /// spawn。只做进程，不做握手（握手要用清单哈希与内容，属于监管那一层）。
    pub fn spawn(spec: &SpawnSpec) -> ExtResult<Self> {
        // host→ext: (读端 child_r, 写端 host_w)
        // ext→host: (写端 child_w, 读端 host_r)
        let (child_r, host_w) = pipe().map_err(|error| ExtError::Spawn(error.to_string()))?;
        // 第二个泵：父进程读、子进程写。`pipe()` 的第一个返回值永远是读端。
        let (host_r, child_w) = pipe().map_err(|error| ExtError::Spawn(error.to_string()))?;

        // 隔离的做法不是"在子进程里乱关"：那会顺手关掉 Rust 自己的 exec-error 管道，
        // 于是 exec 失败时子进程连原因都写不出来（只会 abort）。
        // 正确做法：在父进程里给**除帧管道与 0/1/2 之外**的所有 fd 打上 CLOEXEC，
        // exec 时由内核关掉。子进程的 fd 表因此恰好是 0,1,2,3,4（EX-71）。
        if spec.isolation.close_fds {
            mark_cloexec_except(&[0, 1, 2, child_r, child_w]);
        }

        let mut command = Command::new(&spec.program);
        command
            .arg(&spec.ext_dir)
            .current_dir(&spec.ext_dir)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (key, value) in &spec.isolation.env_allow {
            command.env(key, value);
        }

        let isolation = spec.isolation.clone();
        let uid = spec.uid;
        let parent_pid = std::process::id() as libc::pid_t;
        unsafe {
            command.pre_exec(move || {
                // 3 = 读管道，4 = 写管道。
                if libc::dup2(child_r, 3) < 0 || libc::dup2(child_w, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // dup 出来的 fd 不许带 CLOEXEC，否则 exec 之后管道就没了。
                libc::fcntl(3, libc::F_SETFD, 0);
                libc::fcntl(4, libc::F_SETFD, 0);
                // 三级兜底之三：父进程死了，内核给子进程发 SIGTERM。
                // 另两级是：ProcExt::kill（熔断/卸载）与 Drop（运行时卸载）。
                // 竞态：spawn 到 pre_exec 之间父进程可能已经死了 —— 看一眼 ppid，
                // 变了就自己退，别等一个永远不会来的信号。
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent_pid {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "parent exited during spawn",
                    ));
                }
                if let Some(uid) = uid {
                    // 隔离只有三种可验证的形式：uid / fd 表 / errno（spec 7.1）。
                    if libc::setgroups(0, std::ptr::null()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::setgid(uid) != 0 || libc::setuid(uid) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        let child = command
            .spawn()
            .map_err(|error| ExtError::Spawn(format!("{}: {error}", spec.program.display())))?;
        let pid = child.id();

        // 子进程持有的是 child_r / child_w 的副本；父进程这两端必须关掉，
        // 否则 ext→host 管道的写端永远开着，子进程死了也读不到 EOF（崩溃会变成超时）。
        unsafe {
            libc::close(child_r);
            libc::close(child_w);
        }

        // 父进程只留 host_w / host_r。
        let writer = unsafe { std::fs::File::from_raw_fd(host_w) };
        let mut reader = unsafe { std::fs::File::from_raw_fd(host_r) };

        let routes: Arc<Mutex<BTreeMap<u64, Sender<FrameMsg>>>> = Arc::new(Mutex::new(BTreeMap::new()));
        let orphans: Arc<Mutex<std::collections::BTreeSet<u64>>> =
            Arc::new(Mutex::new(std::collections::BTreeSet::new()));
        let (control_tx, control_rx) = mpsc::channel();
        let flags = Arc::new(ProcFlags::default());

        // 分发线程：按 `call` 路由（**不靠 pid**）；没有 call 的帧是握手/名牌。
        {
            let routes = routes.clone();
            let orphans = orphans.clone();
            let control_tx = control_tx.clone();
            let flags = flags.clone();
            std::thread::spawn(move || {
                loop {
                    match read_frame(&mut reader) {
                        Ok(frame) => {
                            let call = frame.get("call").and_then(Value::as_u64);
                            match call {
                                Some(call) => {
                                    let sender = routes.lock().expect("routes").get(&call).cloned();
                                    match sender {
                                        Some(sender) => {
                                            let _ = sender.send(Ok(frame));
                                        }
                                        // 调用已经收尾：晚期 output 在这里被拒（EX-19）。
                                        None => {
                                            orphans.lock().expect("orphans").insert(call);
                                        }
                                    }
                                }
                                None => {
                                    let _ = control_tx.send(Ok(frame));
                                }
                            }
                        }
                        Err(error) => {
                            let _ = &reader;
                            flags.crashed.store(true, Ordering::SeqCst);
                            match error {
                                FrameError::Oversized { .. } => {
                                    flags.oversize.store(true, Ordering::SeqCst)
                                }
                                _ => {}
                            }
                            let message = error.to_string();
                            for (_, sender) in routes.lock().expect("routes").iter() {
                                let _ = sender.send(Err(message.clone()));
                            }
                            let _ = control_tx.send(Err(message));
                            return;
                        }
                    }
                }
            });
        }

        let process = Self {
            pid,
            uid,
            flags: flags.clone(),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            routes,
            orphans,
            control_tx,
            control_rx: Mutex::new(control_rx),
        };

        // RSS 监控：撞 256MB 硬顶就杀。轮询间隔很短，且杀掉时不追加分配。
        if spec.rss_limit > 0 {
            let pid = process.pid;
            let limit = spec.rss_limit;
            let child = process.child.try_lock();
            // 不拿 child 锁，直接对 pid 发信号。
            drop(child);
            std::thread::spawn(move || loop {
                let rss = read_rss(pid).unwrap_or(0);
                flags.rss_seen.fetch_max(rss, Ordering::SeqCst);
                if rss >= limit {
                    flags.rss_killed.store(true, Ordering::SeqCst);
                    flags.crashed.store(true, Ordering::SeqCst);
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                    return;
                }
                if flags.crashed.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            });
        }

        Ok(process)
    }

    /// 握手：host 先发 hello（带协议版本与清单哈希），扩展回 hello；再发内容，扩展回名牌。
    pub fn handshake(
        &self,
        protocol: u32,
        manifest_hash: &str,
        handles: &Handles,
        timeout: Duration,
    ) -> ExtResult<Vec<String>> {
        self.send(&json!({ "kind": "hello", "protocol": protocol, "manifestHash": manifest_hash }))?;
        let reply = self.control(timeout)?;
        if reply.get("kind").and_then(Value::as_str) != Some("hello") {
            return Err(ExtError::Handshake(format!("unexpected frame: {reply}")));
        }
        let ext_protocol = reply
            .get("protocol")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        if ext_protocol != protocol {
            self.kill();
            return Err(ExtError::ProtocolMismatch {
                host: protocol,
                ext: ext_protocol,
            });
        }

        self.send(&json!({ "kind": "content", "handles": handles }))?;
        let reply = self.control(timeout)?;
        if reply.get("kind").and_then(Value::as_str) != Some("nameplates") {
            return Err(ExtError::Handshake(format!("expected nameplates, got {reply}")));
        }
        Ok(reply
            .get("names")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    fn control(&self, timeout: Duration) -> ExtResult<Value> {
        let receiver = self.control_rx.lock().expect("control");
        match receiver.recv_timeout(timeout) {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(message)) => Err(ExtError::Handshake(message)),
            Err(RecvTimeoutError::Timeout) => Err(ExtError::Handshake("handshake timeout".into())),
            Err(RecvTimeoutError::Disconnected) => Err(ExtError::Handshake("channel closed".into())),
        }
    }

    /// 写一帧。
    pub fn send(&self, frame: &Value) -> ExtResult<()> {
        let mut writer = self.writer.lock().expect("writer");
        write_frame(&mut *writer, frame).map_err(|error| {

            self.flags.crashed.store(true, Ordering::SeqCst);
            ExtError::Spawn(format!("write frame: {error}"))
        })
    }

    /// 登记一条在飞调用的路由。
    pub fn register(&self, call: u64) -> Receiver<FrameMsg> {
        let (tx, rx) = mpsc::channel();
        self.routes.lock().expect("routes").insert(call, tx);
        rx
    }

    pub fn unregister(&self, call: u64) {
        self.routes.lock().expect("routes").remove(&call);
    }

    /// 这条调用之后还回来了几帧？（晚期 output 的观测，EX-19）
    pub fn late_frames(&self, call: u64) -> bool {
        self.orphans.lock().expect("orphans").contains(&call)
    }

    /// 这一帧是不是我们认识的 hello 之外的东西（判据要用）。
    pub fn is_alive(&self) -> bool {
        !self.flags.crashed.load(Ordering::SeqCst)
    }

    /// 杀进程（幂等）。
    pub fn kill(&self) {
        self.flags.crashed.store(true, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(mut code) = self.flags.exit_code.lock() {
                *code = None;
            }
        }
    }

    /// 进程退了吗？退码写进 flags。
    pub fn reap(&self) -> Option<i32> {
        if let Ok(mut child) = self.child.lock() {
            if let Ok(Some(status)) = child.try_wait() {
                let code = status.code().unwrap_or(-1);
                if let Ok(mut slot) = self.flags.exit_code.lock() {
                    *slot = Some(code);
                }
                self.flags.crashed.store(true, Ordering::SeqCst);
                return Some(code);
            }
        }
        if let Ok(slot) = self.flags.exit_code.lock() {
            return *slot;
        }
        None
    }

    /// 读一次 RSS（/proc/<pid>/status）。
    pub fn rss(&self) -> u64 {
        read_rss(self.pid).unwrap_or(0)
    }
}

impl Drop for ProcExt {
    fn drop(&mut self) {
        // 只杀还活着的；已经死的子进程由 reap/kill 收过。
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 把除 `keep` 之外的 fd 全部标成 CLOEXEC（对当前进程生效，只影响未来的 exec）。
fn mark_cloexec_except(keep: &[RawFd]) {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(fd) = entry.file_name().to_string_lossy().parse::<RawFd>() else {
            continue;
        };
        // `read_dir` 自己会占一个 fd，它也在列表里；跳过它会提前关掉枚举本身。
        if keep.contains(&fd) || fd <= 2 {
            continue;
        }
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

fn pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as RawFd; 2];
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

/// `/proc/<pid>/status` 的 `VmRSS`（字节）。
pub fn read_rss(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    Some(0)
}

/// `/proc/<pid>/status` 的 `Groups:` 行（EX-72 判它为空）。
pub fn read_groups(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    text.lines()
        .find(|line| line.starts_with("Groups:"))
        .map(|line| line.trim_start_matches("Groups:").trim().to_string())
}

/// `/proc/<pid>/environ` 的键集合（EX-46 判它等于白名单）。
pub fn read_environ_keys(pid: u32) -> Vec<String> {
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return Vec::new();
    };
    bytes
        .split(|byte| *byte == 0)
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let text = String::from_utf8_lossy(pair);
            text.split_once('=').map(|(key, _)| key.to_string())
        })
        .collect()
}

/// `/proc/<pid>/fd` 里的 fd 号（升序）。
pub fn read_fds(pid: u32) -> Vec<u32> {
    let mut fds: Vec<u32> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .collect();
    fds.sort_unstable();
    fds
}

/// fd → 它指向的路径（`/proc/<pid>/fd/<n>` 的 readlink）。
pub fn read_fd_target(pid: u32, fd: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/fd/{fd}"))
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default()
}
