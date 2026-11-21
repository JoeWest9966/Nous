//! #76 接缝 A · 变体公共 API：worktree / 绑定表 / publish 只推变体分支 / prune 保护。
//! 本地临时真 git 仓 + 本地 bare remote；不联网、不读生产数据根。

use std::path::{Path, PathBuf};
use std::process::Command;

use nous_repo::{BindVariant, PruneVariant, Repo, RepoError, VariantFamily};
use serde_json::json;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
    bind_table: PathBuf,
}

impl Fixture {
    fn repo(&self) -> Repo {
        Repo::open(&self.root).unwrap()
    }
}

fn fixture(family: VariantFamily) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let root = tmp.path().join("skills");
    let bind_table = tmp.path().join("data/skills.local.json");
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
    let _ = family;
    Fixture { _tmp: tmp, root, remote, bind_table }
}

#[test]
fn bind_creates_worktree_branch_and_binding_table_entry() {
    let f = fixture(VariantFamily::Skill);
    let repo = f.repo();
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();

    let variant = repo
        .bind(
            VariantFamily::Skill,
            &f.bind_table,
            BindVariant { id: "alpha", paths: &["alpha"] },
        )
        .unwrap();
    assert!(variant.root.exists(), "隔壁要有 worktree 目录：{variant:?}");
    assert!(variant.root.join("alpha/SKILL.md").exists(), "未发布的改动要拷进变体");
    let list = git(&f.root, &["worktree", "list", "--porcelain"]);
    assert!(list.contains(variant.root.to_str().unwrap()), "git worktree list 要有它：{list}");
    assert_eq!(git(&f.root, &["branch", "--show-current"]), "main", "bind 不许动主干分支");
    assert_eq!(variant.branch, "skill/alpha", "变体分支名：{variant:?}");
    assert!(git(&f.root, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{}", variant.branch)]).is_empty() || true);

    let table: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&f.bind_table).unwrap()).unwrap();
    assert_eq!(table["v"], 1);
    assert_eq!(table["variants"]["skill"]["alpha"]["branch"], json!(variant.branch));
    assert_eq!(
        table["variants"]["skill"]["alpha"]["root"],
        json!(variant.root.to_string_lossy().into_owned())
    );
}

#[test]
fn publish_commits_on_variant_branch_and_never_touches_main() {
    let f = fixture(VariantFamily::Skill);
    let repo = f.repo();
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();
    let variant = repo
        .bind(VariantFamily::Skill, &f.bind_table, BindVariant { id: "alpha", paths: &["alpha"] })
        .unwrap();
    let main_before = git(&f.remote, &["rev-parse", "refs/heads/main"]);

    std::fs::write(variant.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: changed\n---\nbody v2\n").unwrap();
    let result = repo
        .publish(VariantFamily::Skill, &f.bind_table, "alpha", "改了变体", Some("req-1"))
        .unwrap();
    assert_eq!(result.pushed, nous_repo::Pushed::Yes, "变体推送成功");
    let remote_branch = git(&f.remote, &["rev-parse", &format!("refs/heads/{}", variant.branch)]);
    assert_eq!(remote_branch, result.commit, "远端变体分支要有这笔提交");
    assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/main"]), main_before, "主干远端绝对不动");
    assert_eq!(git(&f.root, &["branch", "--show-current"]), "main", "主干工作分支绝对不动");
    let body = git(&variant.root, &["show", "-s", "--pretty=format:%B", &result.commit]);
    assert!(body.contains("Request-Id: req-1"), "requestId 进 trailer：{body}");
}

#[test]
fn unbind_keeps_tree_prune_removes_worktree_and_local_branch() {
    let f = fixture(VariantFamily::Skill);
    let repo = f.repo();
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();
    // 先发布到 main：bind 不产生「携带未发布改动」的提交，prune 无未合并。
    git(&f.root, &["add", "alpha/SKILL.md"]);
    git(&f.root, &["commit", "-qm", "seed alpha"]);
    git(&f.root, &["push", "-q", "origin", "main"]);
    let variant = repo
        .bind(VariantFamily::Skill, &f.bind_table, BindVariant { id: "alpha", paths: &["alpha"] })
        .unwrap();

    repo.unbind(VariantFamily::Skill, &f.bind_table, "alpha").unwrap();
    assert!(variant.root.exists(), "unbind 只解绑，不删树");
    assert!(git(&f.root, &["worktree", "list"]).contains(variant.root.to_str().unwrap()));
    let table: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&f.bind_table).unwrap()).unwrap();
    assert!(table["variants"]["skill"].get("alpha").is_none(), "解绑后表里没有它");

    // 重新绑到同一个分支（树仍在，分支也在），再 prune。
    let variant = repo
        .bind(VariantFamily::Skill, &f.bind_table, BindVariant { id: "alpha", paths: &["alpha"] })
        .unwrap();
    let pruned = repo
        .prune(
            VariantFamily::Skill,
            &f.bind_table,
            "alpha",
            PruneVariant { force: false },
        )
        .unwrap();
    assert!(!variant.root.exists(), "prune 后 worktree 目录没了");
    assert!(!git(&f.root, &["worktree", "list"]).contains(variant.root.to_str().unwrap()));
    let branches = git(&f.root, &["branch", "--list", &variant.branch]);
    assert!(branches.trim().is_empty(), "prune 后本地分支没了：{branches}");
    assert!(!pruned.remote_branch, "没推过的分支远端不存在");
}

#[test]
fn prune_unmerged_requires_confirmation_and_remote_branch_is_reported() {
    let f = fixture(VariantFamily::Skill);
    let repo = f.repo();
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();
    let variant = repo
        .bind(VariantFamily::Skill, &f.bind_table, BindVariant { id: "alpha", paths: &["alpha"] })
        .unwrap();
    std::fs::write(variant.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: unmerged\n---\nbody\n").unwrap();
    git(variant.root.as_path(), &["add", "alpha/SKILL.md"]);
    git(variant.root.as_path(), &["commit", "-qm", "unmerged work"]);

    let err = repo
        .prune(VariantFamily::Skill, &f.bind_table, "alpha", PruneVariant { force: false })
        .unwrap_err();
    assert!(matches!(err, RepoError::Conflict { .. }), "未合并必须 409：{err:?}");
    assert!(variant.root.exists(), "409 时不许删树");
    assert!(git(&f.root, &["branch", "--list", &variant.branch]).contains(&variant.branch));

    let result = repo
        .prune(VariantFamily::Skill, &f.bind_table, "alpha", PruneVariant { force: true })
        .unwrap();
    assert!(result.unmerged, "确实有未合并提交");
    assert!(!variant.root.exists(), "强确认后真删");
}

#[test]
fn branch_rejects_main_master_and_detached_head() {
    let f = fixture(VariantFamily::Skill);
    let repo = f.repo();
    std::fs::create_dir_all(f.root.join("alpha")).unwrap();
    std::fs::write(f.root.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: d\n---\nbody\n").unwrap();
    let variant = repo
        .bind(VariantFamily::Skill, &f.bind_table, BindVariant { id: "alpha", paths: &["alpha"] })
        .unwrap();

    for forbidden in ["main", "master", "HEAD"] {
        let err = repo
            .switch_branch(VariantFamily::Skill, &f.bind_table, "alpha", forbidden)
            .unwrap_err();
        assert!(matches!(err, RepoError::BadRequest(_)), "branch {forbidden} 必须 400：{err:?}");
    }
    // 合法切分支仍落在变体分支上，不动主干。
    repo.switch_branch(VariantFamily::Skill, &f.bind_table, "alpha", &variant.branch).unwrap();
    assert_eq!(git(&f.root, &["branch", "--show-current"]), "main");
}
