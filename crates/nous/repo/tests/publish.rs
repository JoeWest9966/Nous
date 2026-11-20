//! #75 接缝 A：公共 `nous_repo` API（临时真 git 仓 + 本地 bare remote）。
//! 不读生产数据根、不联网、不 spawn 服务端。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use nous_repo::{Repo, WriteFile};
use serde_json::json;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", dir)
        .output()
        .expect("git 可用");
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    remote: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let root = tmp.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp.path(), &["init", "--bare", "-q", remote.to_str().unwrap()]);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    std::fs::write(root.join("README.md"), "initial\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-qm", "init"]);
    git(&root, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    let _ = NEXT.fetch_add(1, Ordering::Relaxed);
    Fixture { _tmp: tmp, remote, root }
}

#[test]
fn write_then_publish_commits_only_the_requested_path_and_advances_remote() {
    let f = fixture();
    // 制造一个「搭车」文件：它留在工作树里，绝不能进这次提交。
    std::fs::write(f.root.join("UNRELATED.md"), "should not be committed\n").unwrap();
    let before = git(&f.remote, &["rev-parse", "refs/heads/main"]);

    let repo = Repo::open(&f.root).unwrap();
    let written = repo
        .write_file(WriteFile {
            path: "drafts/alpha.md",
            content: b"# alpha\n",
            base_etag: None,
        })
        .unwrap();
    assert!(!written.etag.is_empty(), "etag 应有内容哈希");
    let result = repo
        .commit(&[written.path.clone()], "发布 alpha", None)
        .unwrap();
    assert!(result.pushed, "本地 bare remote 可达时必须推送成功");

    let after = git(&f.remote, &["rev-parse", "refs/heads/main"]);
    assert_ne!(before, after, "bare remote 的 HEAD 应多一笔");

    let names = git(
        &f.root,
        &["show", "--pretty=format:", "--name-only", &result.commit],
    );
    let names: Vec<&str> = names.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        names,
        vec!["drafts/alpha.md"],
        "提交只许包含请求的那一个路径（防搭车）"
    );

    let files = repo.pending_commit(&result.commit).unwrap();
    assert_eq!(files.len(), 1, "commit 详情应只有一笔文件");
    assert_eq!(files[0].path, "drafts/alpha.md");
    assert_eq!(
        serde_json::to_value(&files[0].after).unwrap(),
        json!("# alpha\n")
    );

    // 误用的 JSON 输入也不该在这里编译：确保公共 API 上能看到 etag/path。
    assert_eq!(written.path, "drafts/alpha.md");
}
