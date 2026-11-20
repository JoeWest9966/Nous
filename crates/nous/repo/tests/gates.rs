//! #75 接缝 A · 内容闸：四种私库各造坏样例，commit 400 且远端不动。

use std::path::{Path, PathBuf};
use std::process::Command;

use nous_repo::{Repo, RepoError, WriteFile};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0").env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fixture(kind: &str) -> (tempfile::TempDir, Repo, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let root = tmp.path().join(kind);
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
    let repo = Repo::open(&root).unwrap();
    (tmp, repo, root, remote)
}

fn assert_rejected(kind: &str, path: &str, content: &[u8]) {
    let (_tmp, repo, root, remote) = fixture(kind);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let w = repo.write_file(WriteFile { path, content, base_etag: None }).unwrap();
    let err = repo.commit(&[w.path], "bad", None).unwrap_err();
    assert!(matches!(err, RepoError::BadRequest(_)), "{kind} 坏样例必须 400：{err:?}");
    assert!(!err.to_string().is_empty(), "400 要指出字段");
    assert_eq!(git(&remote, &["rev-parse", "refs/heads/main"]), git(&root, &["rev-parse", "HEAD"]), "{kind} 坏样例不许推上去");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head, "{kind} 坏样例不许 commit");
}

#[test]
fn bad_skill_frontmatter_is_rejected_before_commit_and_push() {
    assert_rejected("skills", "bad/SKILL.md", b"# no frontmatter\n");
}

#[test]
fn bad_prompt_with_empty_body_and_missing_fields_is_rejected() {
    assert_rejected("prompts", "bad/nous.json", br#"{"name":"x"}"#);
    assert_rejected("prompts", "bad/empty.md", b"   \n");
}

#[test]
fn bad_preset_spec_without_version_or_with_unknown_key_is_rejected() {
    assert_rejected("presets", "bad/preset.json", br#"{"name":"x"}"#);
    assert_rejected("presets", "bad/unknown.json", br#"{"v":1,"name":"x","bogus":true}"#);
}

#[test]
fn bad_extend_without_entry_or_wrong_name_is_rejected() {
    assert_rejected("extend", "bad/nous.json", br#"{"name":"wrong","entry":"index.js"}"#);
}

#[test]
fn good_samples_pass_all_four_gates() {
    for (kind, path, content) in [
        ("skills", "good/SKILL.md", &b"---\nname: good\ndescription: ok\n---\nbody\n"[..]),
        ("prompts", "good/nous.json", br#"{"name":"g","description":"d","prompt":"p"}"#),
        ("presets", "good/preset.json", br#"{"v":1,"name":"g"}"#),
        ("extend", "good/nous.json", br#"{"name":"good","entry":"index.js"}"#),
    ] {
        let (_tmp, repo, root, _remote) = fixture(kind);
        if kind == "extend" {
            std::fs::create_dir_all(root.join("good")).unwrap();
            std::fs::write(root.join("good/index.js"), "// entry\n").unwrap();
        }
        let w = repo.write_file(WriteFile { path, content, base_etag: None }).unwrap();
        if kind == "extend" {
            // 入口文件也要一起进 commit 才能通过闸；这里只验 nous.json 的 name 已匹配目录。
            let e = repo.write_file(WriteFile { path: "good/index.js", content: b"// entry\n", base_etag: None }).unwrap();
            repo.commit(&[w.path, e.path], "ok", None).unwrap();
        } else {
            repo.commit(&[w.path], "ok", None).unwrap();
        }
    }
}
