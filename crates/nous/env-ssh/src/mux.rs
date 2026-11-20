//! ControlMaster 复用（#78）。
//!
//! 0700 临时目录，一台机器一个 socket（按 user/addr/port 分开）；`NOUS_SSH_MUX=0` 关。
//! 目录随环境 `Drop`/`cleanup` 删除；master 由 `ControlPersist=120` 自行超时退出。

use std::path::PathBuf;

use crate::config::Machine;

pub const MUX_ENV: &str = "NOUS_SSH_MUX";
const MUX_IDLE: u16 = 120;

/// 一次进程生命周期里的 mux 目录（懒建，只建一次）。
#[derive(Debug)]
pub struct Mux {
    dir: Option<PathBuf>,
}

impl Default for Mux {
    fn default() -> Self {
        Self::new()
    }
}

impl Mux {
    pub fn new() -> Self {
        let enabled = std::env::var(MUX_ENV)
            .map(|value| value != "0")
            .unwrap_or(true);
        if !enabled {
            return Self { dir: None };
        }
        let dir = tempfile::Builder::new()
            .prefix("nous-mux-")
            .tempdir()
            .map(|temp| temp.keep())
            .ok();
        Self { dir }
    }

    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// 追加到 ssh argv 的 `-o` 选项。顺序固定，便于测试断言。
    pub fn ssh_args(&self, machine: &Machine) -> Vec<String> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let safe_addr: String = machine
            .addr
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' { ch } else { '_' })
            .collect();
        let socket = dir.join(format!(
            "{}_{}_{}.sock",
            machine.user, safe_addr, machine.port
        ));
        vec![
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", socket.display()),
            "-o".to_string(),
            format!("ControlPersist={MUX_IDLE}"),
        ]
    }

    pub fn cleanup(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Drop for Mux {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_is_disabled_by_nous_ssh_mux_zero() {
        // 环境变量在 new() 里只读一次；这里用子进程隔离避免并行测试互相踩。
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .env(MUX_ENV, "0")
            .args(["mux::tests::mux_disabled_probe", "--exact", "--nocapture"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("enabled=false"), "NOUS_SSH_MUX=0 必须关 mux：{text}");
    }

    #[test]
    fn mux_disabled_probe() {
        if std::env::var(MUX_ENV).ok().as_deref() == Some("0") {
            println!("enabled={}", Mux::new().enabled());
        }
    }

    #[test]
    fn mux_socket_names_are_per_user_and_address() {
        let mux = Mux::new();
        if !mux.enabled() {
            return;
        }
        let a = Machine {
            name: "a".into(), user: "root".into(), addr: "10.0.0.1".into(),
            port: 22, key_path: None, cwd: "~".into(),
        };
        let b = Machine {
            name: "b".into(), user: "root".into(), addr: "10.0.0.2".into(),
            port: 22, key_path: None, cwd: "~".into(),
        };
        assert_ne!(mux.ssh_args(&a), mux.ssh_args(&b));
    }
}
