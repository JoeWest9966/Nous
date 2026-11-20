//! #75 接缝 A · 文件层：etag / If-Match / 路径安全 / 二进制与大小闸。

use std::path::Path;
fn git_bin() -> String { String::from("git") }
use std::process::Command;

use nous_repo::{Repo, RepoError, WriteFile};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new(git_bin())
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn repo() -> (tempfile::TempDir, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    (tmp, Repo::open(&root).unwrap())
}

#[test]
fn stale_if_match_returns_409_with_current_content_and_fresh_etag_wins() {
    let (_tmp, repo) = repo();
    let first = repo.write_file(WriteFile { path: "a.md", content: b"v1", base_etag: None }).unwrap();
    let second = repo.write_file(WriteFile {
        path: "a.md", content: b"v2", base_etag: Some(&first.etag),
    }).unwrap();

    let err = repo.write_file(WriteFile {
        path: "a.md", content: b"v3", base_etag: Some(&first.etag),
    }).unwrap_err();
    match err {
        RepoError::Conflict { current, etag, .. } => {
            assert_eq!(current.unwrap(), b"v2", "409 要带回磁盘当前正文");
            assert_eq!(etag.unwrap(), second.etag, "409 要带回当前 etag");
        }
        other => panic!("期望 409 Conflict，实际 {other:?}"),
    }

    let third = repo.write_file(WriteFile {
        path: "a.md", content: b"v3", base_etag: Some(&second.etag),
    }).unwrap();
    assert_eq!(repo.read_file("a.md").unwrap().content, b"v3");
    assert_eq!(third.etag, repo.read_file("a.md").unwrap().etag);
}

#[test]
fn path_escape_is_rejected_without_creating_any_file() {
    let (_tmp, repo) = repo();
    let outside = repo.root().parent().unwrap().join("escaped.txt");
    let err = repo.write_file(WriteFile {
        path: "../../escaped.txt", content: b"oops", base_etag: None,
    }).unwrap_err();
    assert!(matches!(err, RepoError::BadRequest(_)), "逃逸必须是 400：{err:?}");
    assert!(!outside.exists(), "磁盘上不许出现逃逸文件");
}

#[test]
fn symlink_escape_is_rejected_by_realpath_check() {
    // `../../` 由组件检查就能挡住；只有 realpath 能挡住「路径在仓内、但符号链接指向仓外」。
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("skills");
    std::fs::create_dir_all(&root).unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("link")).unwrap();

    let repo = Repo::open(&root).unwrap();
    let err = repo.write_file(WriteFile {
        path: "link/escaped.txt", content: b"oops", base_etag: None,
    }).unwrap_err();
    assert!(matches!(err, RepoError::BadRequest(_)), "符号链接逃逸必须 400：{err:?}");
    assert!(!outside.join("escaped.txt").exists(), "仓外不许出现文件");
}

#[test]
fn binary_and_oversize_files_are_rejected() {
    let (_tmp, repo) = repo();
    let binary = repo.write_file(WriteFile {
        path: "bin.md", content: b"a\0b", base_etag: None,
    }).unwrap_err();
    assert!(matches!(binary, RepoError::BadRequest(_)), "二进制必须 400");
    let huge = vec![b'x'; 512 * 1024 + 1];
    let oversize = repo.write_file(WriteFile {
        path: "huge.md", content: &huge, base_etag: None,
    }).unwrap_err();
    assert!(matches!(oversize, RepoError::BadRequest(_)), "超过 512KB 必须 400");
    assert!(!repo.root().join("bin.md").exists());
    assert!(!repo.root().join("huge.md").exists());
}

#[test]
fn write_uses_atomic_rename_without_leaving_temp_file() {
    let (_tmp, repo) = repo();
    repo.write_file(WriteFile { path: "nested/x.md", content: b"hello", base_etag: None }).unwrap();
    assert_eq!(repo.read_file("nested/x.md").unwrap().content, b"hello");
    let leftovers: Vec<_> = std::fs::read_dir(repo.root().join("nested")).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, vec!["x.md"], "rename 后不许留临时文件");
}
