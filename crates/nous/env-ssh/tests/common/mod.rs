//! 真 sshd 夹具：本机起一台临时 sshd，给 env-ssh 判据用。
//!
//! 不 mock ssh —— 判据要的是「字节真的跨过 ssh 落到远端盘上」。
//! 环境没有 sshd 的机器上这些测试会 panic 并指名缺什么，绝不静默 SKIP。

#![cfg(unix)]

use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nous_env_ssh::config::Machine;

pub struct Sshd {
    pub root: tempfile::TempDir,
    pub machine: Machine,
    pub key_pem: String,
    pub port: u16,
    child: Option<Child>,
}

impl Sshd {
    /// 起一台只听 127.0.0.1 的 sshd；HOME 与 authorized_keys 都在临时目录里。
    pub fn start() -> Self {
        assert!(
            Path::new("/usr/sbin/sshd").is_file(),
            "需要 /usr/sbin/sshd 才能跑真 ssh 判据（SKIP 不是 PASS）"
        );
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let run = root.path().join("run");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();

        // 客户端私钥（ed25519）
        let key_path = root.path().join("id_ed25519");
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key_path)
            .status()
            .unwrap();
        assert!(status.success(), "ssh-keygen 失败");
        let key_pem = std::fs::read_to_string(&key_path).unwrap();
        let public = std::fs::read_to_string(root.path().join("id_ed25519.pub")).unwrap();

        let ssh_dir = home.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        std::fs::set_permissions(&ssh_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(ssh_dir.join("authorized_keys"), public).unwrap();
        std::fs::set_permissions(
            ssh_dir.join("authorized_keys"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        // 宿主 key：sshd 自己要一个 host key，复用系统里那只。
        let host_key = Path::new("/etc/ssh/ssh_host_ed25519_key");
        assert!(host_key.is_file(), "缺少 host key：{}", host_key.display());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let config = root.path().join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\n\
                 PidFile {}\nAuthorizedKeysFile {}\n\
                 PermitRootLogin prohibit-password\nPasswordAuthentication no\nPubkeyAuthentication yes\n\
                 UsePAM no\nStrictModes no\nSubsystem sftp /usr/lib/openssh/sftp-server\n",
                host_key.display(),
                run.join("sshd.pid").display(),
                ssh_dir.join("authorized_keys").display(),
            ),
        )
        .unwrap();

        let child = Command::new("/usr/sbin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        // 等端口真的起来（不是 sleep 固定时间）：ssh-keyscan 通了才算 ready。
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = false;
        while Instant::now() < deadline {
            if Command::new("ssh-keyscan")
                .args(["-T", "2", "-p", &port.to_string(), "127.0.0.1"])
                .output()
                .map(|out| out.status.success() && !out.stdout.is_empty())
                .unwrap_or(false)
            {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ready, "临时 sshd 没在 {port} 上起来");

        let machine = Machine {
            name: "test-sshd".to_string(),
            user: whoami(),
            addr: "127.0.0.1".to_string(),
            port,
            key_path: Some(key_path.to_string_lossy().into_owned()),
            cwd: home.to_string_lossy().into_owned(),
        };
        Self { root, machine, key_pem, port, child: Some(child) }
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    pub fn write_catalog(&self, path: &Path) {
        let catalog = serde_json::json!({
            "machines": [{
                "name": self.machine.name,
                "user": self.machine.user,
                "addr": self.machine.addr,
                "port": self.machine.port,
                "keyPath": self.machine.key_path,
                "cwd": self.machine.cwd,
            }]
        });
        std::fs::write(path, serde_json::to_string(&catalog).unwrap()).unwrap();
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn whoami() -> String {
    std::env::var("USER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "root".to_string())
}
