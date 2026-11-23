//! #75 接缝 A：公共 `nous_repo` API（临时真 git 仓 + 本地 bare remote）。
//! 不读生产数据根、不联网、不 spawn 服务端。

use std::path::{Path, PathBuf};
fn git_bin() -> String { String::from("git") }
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use nous_repo::{Pushed, Repo, RepoError, WriteCommit, WriteFile};
use serde_json::json;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(git_bin())
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
    assert_eq!(result.pushed, Pushed::Yes, "本地 bare remote 可达时必须推送成功");

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

    let detail = repo.pending_commit(&result.commit).unwrap();
    assert_eq!(detail.message, "发布 alpha", "pending/commit 要给真提交消息");
    assert_eq!(detail.files.len(), 1, "commit 详情应只有一笔文件");
    assert_eq!(detail.files[0].path, "drafts/alpha.md");
    assert_eq!(
        serde_json::to_value(&detail.files[0].after).unwrap(),
        json!("# alpha\n")
    );

    // 误用的 JSON 输入也不该在这里编译：确保公共 API 上能看到 etag/path。
    assert_eq!(written.path, "drafts/alpha.md");
    assert_eq!(result.pushed, Pushed::Yes, "有远端时必须是 true");
    assert_eq!(serde_json::to_value(result.pushed).unwrap(), json!(true));

    // 防搭车不仅看提交内容，也看 index：`git add .` 会把 UNRELATED.md 悄悄 staged，
    // 即使 commit -- pathspec 没带它，下一次无 pathspec 的提交就会把它捎上。
    let staged = git(&f.root, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, "", "发布后不许留下被误 staged 的搭车文件：{staged}");
}

#[test]
fn commit_without_remote_reports_pushed_null_instead_of_lying() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    git(&tmp.path(), &["init", "-q"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    let repo = Repo::open(&root).unwrap();
    let w = repo
        .write_file(WriteFile {
            path: "no-remote.md",
            content: b"still committed\n",
            base_etag: None,
        })
        .unwrap();
    let result = repo.commit(&[w.path], "no remote", None).unwrap();
    assert_eq!(result.pushed, Pushed::Unknown, "没有远端时不能冒充推送成功");
    assert_eq!(serde_json::to_value(result.pushed).unwrap(), json!(null));
    assert_eq!(
        std::fs::read(root.join("no-remote.md")).unwrap(),
        b"still committed\n",
        "没有远端也不许动用户改动"
    );
}

/// 提交失败（`pre-commit` 钩子拒了）= **这次写什么都没发生**。
///
/// 这条是「写一次提交一次」的另一半：写下去了但没提交，等于盘上多了一份谁也追不到的
/// 改动 —— 锁最怕它（`locked: true` 在盘上、`git log` 里查不到谁锁的、推也推不上去）。
#[test]
fn a_failed_commit_puts_the_file_and_the_index_back() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    repo.write_and_commit(WriteCommit {
        path: "skills/a.md",
        content: Some(b"one\n"),
        base_etag: None,
        message: "feat: a",
        request_id: None,
    })
    .unwrap();
    let before = git(&f.root, &["rev-list", "--count", "HEAD"]).trim().to_string();

    let hook = f.root.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    git(&f.root, &["config", "core.hooksPath", ".git/hooks"]);
    assert!(Command::new("chmod").args(["+x", hook.to_str().unwrap()]).status().unwrap().success());

    // 改内容：报错，盘上还是老那份，工作区干净，提交数不动。
    let err = repo
        .write_and_commit(WriteCommit {
            path: "skills/a.md",
            content: Some(b"two\n"),
            base_etag: None,
            message: "feat: a",
            request_id: None,
        })
        .unwrap_err();
    assert!(matches!(&err, RepoError::Io(_)), "{err:?}");
    assert_eq!(std::fs::read_to_string(f.root.join("skills/a.md")).unwrap(), "one\n");
    assert_eq!(git(&f.root, &["status", "--porcelain"]).trim(), "", "别留下半成品");
    assert_eq!(git(&f.root, &["rev-list", "--count", "HEAD"]).trim(), before);

    // 删也算一次写：失败了，文件得还在。
    let err = repo
        .write_and_commit(WriteCommit {
            path: "skills/a.md",
            content: None,
            base_etag: None,
            message: "chore: rm a",
            request_id: None,
        })
        .unwrap_err();
    assert!(matches!(&err, RepoError::Io(_)), "{err:?}");
    assert_eq!(std::fs::read_to_string(f.root.join("skills/a.md")).unwrap(), "one\n");

    // 新建也算：失败了不该在盘上留一份未跟踪的影子。
    let err = repo
        .write_and_commit(WriteCommit {
            path: "skills/b.md",
            content: Some(b"new\n"),
            base_etag: None,
            message: "feat: b",
            request_id: None,
        })
        .unwrap_err();
    assert!(matches!(&err, RepoError::Io(_)), "{err:?}");
    assert!(!f.root.join("skills/b.md").exists(), "没提交成功的文件不该留在盘上");
    assert_eq!(git(&f.root, &["status", "--porcelain"]).trim(), "");
}
