//! #75 接缝 A · 生命周期与状态：归档三代 / pending / aheadCommits / 推送失败 / 并发。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use nous_repo::{Repo, WriteFile};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let root = tmp.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    git(tmp.path(), &["init", "--bare", "-q", remote.to_str().unwrap()]);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-qm", "init"]);
    git(&root, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&root, &["push", "-q", "-u", "origin", "main"]);
    Fixture { _tmp: tmp, root, remote }
}

fn write_and_publish(repo: &Repo, path: &str, content: &[u8]) {
    let w = repo.write_file(WriteFile { path, content, base_etag: None }).unwrap();
    let result = repo.commit(&[w.path], "publish", None).unwrap();
    assert!(result.pushed);
}

#[test]
fn archive_restore_purge_three_generations_move_files_and_commit_each_step() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    write_and_publish(&repo, "unit-a/SKILL.md", b"---\nname: a\ndescription: test\n---\nbody\n");

    let before = git(&f.root, &["rev-list", "--count", "HEAD"]);
    let a = repo.archive("unit-a", None).unwrap();
    assert!(a.pushed, "归档要 git mv + commit + push");
    assert!(!f.root.join("unit-a").exists());
    assert!(f.root.join(".archive/unit-a/SKILL.md").exists());
    assert_eq!(git(&f.root, &["rev-list", "--count", "HEAD"]), incr(&before, 1));

    let r = repo.restore("unit-a", None).unwrap();
    assert!(r.pushed);
    assert!(f.root.join("unit-a/SKILL.md").exists());
    assert!(!f.root.join(".archive/unit-a").exists());
    assert_eq!(git(&f.root, &["rev-list", "--count", "HEAD"]), incr(&before, 2));

    // purge 作用于归档中的单元：restore 之后要再归档一次才谈得上撕。
    repo.archive("unit-a", None).unwrap();
    let p = repo.purge("unit-a", None).unwrap();
    assert!(p.pushed);
    assert!(!f.root.join(".archive/unit-a").exists());
    assert_eq!(git(&f.root, &["rev-list", "--count", "HEAD"]), incr(&before, 4));
    assert!(git(&f.root, &["cat-file", "-e", &format!("{}^:.archive/unit-a/SKILL.md", p.commit)]).is_empty(), "purge 前归档文件在历史里");
    assert!(!f.root.join(".archive/unit-a").exists());
}

fn incr(value: &str, by: i32) -> String {
    (value.parse::<i32>().unwrap() + by).to_string()
}

#[test]
fn pending_groups_by_unit_and_ahead_commits_counts_unpushed_commits() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    // 未提交改动：pending 非空，按顶层单元分组。
    repo.write_file(WriteFile { path: "unit-a/x.md", content: b"x", base_etag: None }).unwrap();
    repo.write_file(WriteFile { path: "unit-b/y.md", content: b"y", base_etag: None }).unwrap();
    let pending = repo.pending().unwrap();
    let names: Vec<_> = pending.iter().map(|u| u.name.as_str()).collect();
    assert!(names.contains(&"unit-a") && names.contains(&"unit-b"), "pending 按单元分组：{pending:?}");
    assert!(pending.iter().all(|u| u.ahead_commits == 0));

    // 只提交 unit-a，不推：remote 改成不可达，commit 成功但 push=false。
    git(&f.root, &["remote", "set-url", "origin", "/nonexistent/remote.git"]);
    let w = repo.read_file("unit-a/x.md").unwrap();
    let result = repo.commit(&[w.path], "unpushed", None).unwrap();
    assert!(!result.pushed, "这条判据要求 commit 成功但没推上去");
    let pending = repo.pending().unwrap();
    let unit = pending.iter().find(|u| u.name == "unit-a").unwrap();
    assert_eq!(unit.ahead_commits, 1, "已 commit 未 push 的单元 aheadCommits=1：{pending:?}");

    // git push 后 pending 清空。
    git(&f.root, &["remote", "set-url", "origin", f.remote.to_str().unwrap()]);
    git(&f.root, &["push", "-q", "origin", "main"]);
    let pending = repo.pending().unwrap();
    assert!(pending.iter().all(|u| u.ahead_commits == 0), "sync 后 ahead 归零：{pending:?}");
    // unit-b 仍未跟踪；提交了它才真正清空。
    let w = repo.read_file("unit-b/y.md").unwrap();
    repo.commit(&[w.path], "rest", None).unwrap();
    git(&f.root, &["push", "-q", "origin", "main"]);
    assert!(repo.pending().unwrap().is_empty(), "全部发布后 pending 为空");
}

#[test]
fn push_failure_reports_false_and_never_rolls_back_local_changes() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    git(&f.root, &["remote", "set-url", "origin", "/nonexistent/remote.git"]);
    let w = repo.write_file(WriteFile { path: "keep.md", content: b"keep me", base_etag: None }).unwrap();
    let result = repo.commit(&[w.path], "offline", None).unwrap();
    assert!(!result.pushed, "推送失败必须报 false，不许冒充 true");
    assert_eq!(std::fs::read(f.root.join("keep.md")).unwrap(), b"keep me", "用户改动必须留在盘上");
    assert_eq!(git(&f.root, &["rev-parse", "--short", "HEAD"]).trim(), &result.commit[..7], "本地提交仍在");
}

#[test]
fn concurrent_writes_to_same_repo_are_serialized_and_both_commits_survive() {
    let f = fixture();
    let repo = Arc::new(Repo::open(&f.root).unwrap());
    let before: usize = git(&f.root, &["rev-list", "--count", "HEAD"]).parse().unwrap();
    let mut handles = Vec::new();
    for i in 0..2 {
        let repo = repo.clone();
        handles.push(std::thread::spawn(move || {
            let w = repo.write_file(WriteFile {
                path: &format!("parallel-{i}.md"), content: format!("writer-{i}").as_bytes(), base_etag: None,
            }).unwrap();
            repo.commit(&[w.path], &format!("writer-{i}"), None).unwrap()
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(results.iter().all(|r| r.pushed));
    let after: usize = git(&f.root, &["rev-list", "--count", "HEAD"]).parse().unwrap();
    assert_eq!(after, before + 2, "两笔提交都要在，不许交叉或丢失");
    let messages = git(&f.root, &["log", "-2", "--pretty=%s"]);
    assert!(messages.contains("writer-0") && messages.contains("writer-1"));
}
