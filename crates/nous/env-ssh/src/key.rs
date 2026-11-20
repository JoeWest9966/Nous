//! 私钥只以内存字符串进入这里，落盘即 0600 文件，退出/cleanup 删除。

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 进程级兜底：即使 `cleanup` 没被调用，退出时也要删掉临时密钥目录。
static LIVE_KEYS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn live_keys() -> &'static Mutex<Vec<PathBuf>> {
    LIVE_KEYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 把 PEM 文本物化成 0700 目录下的 0600 文件。返回私钥文件路径。
pub fn materialize(pem: &str) -> std::io::Result<PathBuf> {
    let dir = tempfile::Builder::new()
        .prefix("nous-ssh-")
        .tempdir()?;
    let dir = dir.keep(); // 交给 RAII 之外的生命周期：由 cleanup/drop 删除
    // 目录权限显式钉 0700：tempfile 在部分 TMPDIR/umask 组合下会给 0755，
    // 私钥虽然还是 0600，但目录可列会让旁人看见「这里有一把密钥」。
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let file = dir.join("id");
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)?;
    handle.write_all(pem.as_bytes())?;
    if !pem.ends_with('\n') {
        handle.write_all(b"\n")?;
    }
    drop(handle);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    live_keys().lock().unwrap().push(dir.clone());
    Ok(file)
}

/// 删除一把临时密钥所在的整个目录（幂等）。
pub fn remove(path: &Path) {
    let Some(dir) = path.parent() else { return };
    let mut keys = live_keys().lock().unwrap();
    keys.retain(|entry| entry != dir);
    drop(keys);
    let _ = std::fs::remove_dir_all(dir);
}

/// 进程里还活着的临时密钥目录数量（测试用公共接缝，不是内部实现细节）。
pub fn live_key_dirs() -> usize {
    live_keys().lock().unwrap().len()
}

/// 密钥文件的 RAII 句柄：`cleanup` 或 `Drop` 都删。
pub struct TemporaryKey {
    path: PathBuf,
}

impl TemporaryKey {
    pub fn new(pem: &str) -> std::io::Result<Self> {
        Ok(Self { path: materialize(pem)? })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cleanup(&mut self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        remove(&self.path);
        self.path = PathBuf::new();
    }
}

impl Drop for TemporaryKey {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_key_is_mode_0600_and_removed_on_cleanup() {
        let mut key = TemporaryKey::new("-----BEGIN OPENSSH PRIVATE KEY-----\ntest\n").unwrap();
        let path = key.path().to_path_buf();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "私钥文件必须 0600，实测 {mode:o}");
        assert_eq!(
            std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777,
            0o700,
            "私钥目录必须 0700"
        );
        key.cleanup();
        assert!(!path.exists(), "cleanup 后私钥文件必须消失");
        assert!(!path.parent().unwrap().exists(), "临时目录也必须消失");
    }
}
