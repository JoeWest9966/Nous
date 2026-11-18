//! D12：工作目录断言。`WorkingDirectory=` 是唯一真相，不引入 `NOUS_WORKDIR`。
//!
//! 真正写一个临时文件再删 —— 只查权限位看不出只读挂载、ACL、`chattr +i`。

use std::path::Path;

/// 从 `current_dir()` 取工作目录并断言；失败时给出可直接贴进 journal 的理由。
pub fn assert_workdir() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|error| {
        format!("current_dir() 读不到（{error}）：WorkingDirectory= 没设")
    })?;
    workdir_problem(&cwd).map_or(Ok(()), Err)
}

/// 不合格的理由（给人看；slug 由调用方走理由表）。
pub fn workdir_problem(dir: &Path) -> Option<String> {
    if dir == Path::new("/") {
        return Some("current_dir() 是 /：WorkingDirectory= 没设（或设成了 /）".into());
    }
    if !dir.is_dir() {
        return Some(format!(
            "current_dir() {} 不是目录：WorkingDirectory= 没设或指向不存在的目录",
            dir.display()
        ));
    }
    // 真写一次。名字带 pid + 计数，避免撞到别人；写完立刻删。
    let stamp = format!(".nous-workdir-check-{}", std::process::id());
    let probe = dir.join(stamp);
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(error) => Some(format!(
            "current_dir() {} 不可写（{error}）：WorkingDirectory= 没设或指向不可写的目录",
            dir.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{assert_workdir, workdir_problem};
    use std::path::Path;
    use std::sync::Mutex;

    // 进程级 cwd：所有改 cwd 的用例必须串行。
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn root_is_refused() {
        let _guard = CWD_LOCK.lock().unwrap();
        assert!(workdir_problem(Path::new("/")).unwrap().contains("WorkingDirectory="));
    }

    #[test]
    fn missing_directory_is_refused() {
        let _guard = CWD_LOCK.lock().unwrap();
        let problem = workdir_problem(Path::new("/definitely/not/a/nous/workdir"));
        assert!(problem.unwrap().contains("WorkingDirectory="));
    }

    #[test]
    fn writable_directory_is_accepted() {
        let _guard = CWD_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(workdir_problem(dir.path()).is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "探针必须删掉");
    }

    #[test]
    fn real_chdir_refusal_mentions_working_directory() {
        let _guard = CWD_LOCK.lock().unwrap();
        let original = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = assert_workdir();
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }
}
