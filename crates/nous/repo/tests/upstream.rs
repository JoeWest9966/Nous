//! #77 接缝 A · 判据 1-5：`Upstream` 公共 API，两个本地临时 git 仓模拟上游与我们。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use nous_repo::{Source, SourceVersion, Upstream};

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
    upstream: PathBuf,
    root: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("upstream-origin");
        let root = tmp.path().join("skills");
        let data = tmp.path().join("data");
        fs::create_dir_all(&upstream).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&data).unwrap();
        git(&upstream, &["init", "-q"]);
        git(&upstream, &["config", "user.email", "t@e"]);
        git(&upstream, &["config", "user.name", "T"]);
        git(&upstream, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        Self { _tmp: tmp, upstream, root, data }
    }

    fn commit(&self, rel: &str, content: &str) -> String {
        let path = self.upstream.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        git(&self.upstream, &["add", rel]);
        git(&self.upstream, &["commit", "-qm", "add {rel}"]);
        git(&self.upstream, &["rev-parse", "HEAD"])
    }

    fn seed_skill(&self, dir: &str) -> String {
        self.commit(
            &format!("{dir}/SKILL.md"),
            &format!("---\nname: {dir}\ndescription: 上游技能\n---\n正文 v1\n"),
        )
    }

    fn subscribe(&self) -> Upstream {
        let up = Upstream::open(&self.root, &self.data).unwrap();
        up.write_sources(&[Source {
            name: "skills".into(),
            url: self.upstream.to_string_lossy().into_owned(),
            branch: Some("main".into()),
            prefix: None,
            version: None,
        }])
        .unwrap();
        up
    }
}

#[test]
fn check_reports_newer_sync_pins_and_only_pin_writes_version() {
    let f = Fixture::new();
    let first = f.seed_skill("demo");
    let up = f.subscribe();

    let head = up.refresh("skills").unwrap();
    assert_eq!(head, first, "首拉 HEAD 是上游当前 commit");
    // 首拉顺手记基线：唯一一次非 pin 写 version。
    assert_eq!(up.sources().unwrap()[0].version.as_ref().unwrap().commit, first);

    // 上游加一个文件 → check 报 newer:true（远端 HEAD 与认的不一样）。
    let second = f.commit("demo/extra.md", "新增附件\n");
    let check = up.check(Some("skills")).unwrap();
    assert_eq!(check[0].head, second);
    let status = up.status().unwrap();
    assert_eq!(status[0].mirror_head.as_deref(), Some(first.as_str()), "check 不改镜像");
    assert_eq!(status[0].pinned_commit(), first, "pinned 是认的版本");
    assert!(status[0].newer, "远端 HEAD != pinned → newer");
    assert!(!status[0].drift, "镜像还没动，不该 drift");

    // sync = refresh + pin：pinned 变成上游 HEAD，newer 归零。
    let synced = up.sync("skills").unwrap();
    assert_eq!(synced, second);
    let status = up.status().unwrap();
    assert_eq!(status[0].pinned_commit(), second, "sync 必须更新 pinned");
    assert!(!status[0].newer, "sync 后 newer 归零");
    assert!(!status[0].drift);
    assert_eq!(up.sources().unwrap()[0].version.as_ref().unwrap().commit, second);

    // 别的端点都不许改 version：refresh / check / tree / file / status 后仍是 second。
    up.refresh("skills").unwrap();
    up.check(None).unwrap();
    let _ = up.tree_items("skills", "demo").unwrap();
    let _ = up.file("skills", "demo/SKILL.md").unwrap();
    let _ = up.status().unwrap();
    assert_eq!(up.sources().unwrap()[0].version.as_ref().unwrap().commit, second, "只有 pin/sync 能改 version");

    // pin 显式钉到旧版本：version 变了，drift 立刻可见。
    let pinned = up.pin("skills", Some(&first)).unwrap();
    assert_eq!(pinned, first);
    assert_eq!(up.sources().unwrap()[0].version.as_ref().unwrap().commit, first);
    let status = up.status().unwrap();
    assert!(status[0].drift, "镜像 HEAD second != pinned first");
}

#[test]
fn manual_mirror_edit_drifts_and_refresh_clears_it() {
    let f = Fixture::new();
    let first = f.seed_skill("demo");
    let up = f.subscribe();
    up.refresh("skills").unwrap();

    // 手改镜像里的工作区文件 → drift:true（工作区与认的版本已经不是一回事）。
    let mirror = f.data.join("upstream/skills");
    fs::write(mirror.join("demo/SKILL.md"), "---\nname: demo\ndescription: 本地改过\n---\n").unwrap();
    let status = up.status().unwrap();
    assert_eq!(status[0].mirror_head.as_deref(), Some(first.as_str()));
    assert!(status[0].drift, "镜像里手改文件 → drift:true");

    // 在镜像工作树里提交 → 镜像 HEAD 前进但 version 不动 → drift:true。
    git(&mirror, &["add", "demo/SKILL.md"]);
    git(&mirror, &["config", "user.email", "t@e"]);
    git(&mirror, &["config", "user.name", "T"]);
    git(&mirror, &["commit", "-qm", "manual"]);
    let manual = git(&mirror, &["rev-parse", "HEAD"]);
    assert_ne!(manual, first, "手动提交应让镜像 HEAD 前进");
    assert!(up.status().unwrap()[0].drift, "镜像 HEAD 手动前进 → drift");

    // refresh 拉回远端：HEAD 回到 first，drift 清零。
    up.refresh("skills").unwrap();
    let status = up.status().unwrap();
    assert_eq!(status[0].mirror_head.as_deref(), Some(first.as_str()), "refresh 后镜像 HEAD 回到远端 {first}（manual={manual}）");
    assert!(!status[0].drift, "refresh 后 drift 清零");
}

#[test]
fn check_uses_ls_remote_only_and_never_touches_mirror_files() {
    let f = Fixture::new();
    let first = f.seed_skill("demo");
    let up = f.subscribe();
    up.refresh("skills").unwrap();
    f.commit("demo/extra.md", "上游加了文件\n");

    let mirror = f.data.join("upstream/skills");
    let before: Vec<_> = walk(&mirror)
        .filter(|(p, _)| p.file_name().and_then(|n| n.to_str()) != Some("nous-check.json"))
        .map(|(p, m)| (p, m.modified().unwrap()))
        .collect();
    let before_objects = count_objects(&mirror);

    up.check(Some("skills")).unwrap();

    let after: Vec<_> = walk(&mirror)
        .filter(|(p, _)| p.file_name().and_then(|n| n.to_str()) != Some("nous-check.json"))
        .map(|(p, m)| (p, m.modified().unwrap()))
        .collect();
    assert_eq!(before, after, "check 只 ls-remote，不许改镜像文件 mtime（缓存文件除外）");
    assert_eq!(count_objects(&mirror), before_objects, "check 不下载任何 git 对象");
    assert_eq!(git(&mirror, &["rev-parse", "HEAD"]), first, "check 不动镜像 HEAD");
}

#[test]
fn adopt_writes_foot_and_compares_blob_sha_states() {
    let f = Fixture::new();
    f.seed_skill("demo");
    f.commit("demo/attach.md", "附件原文\n");
    let up = f.subscribe();
    up.refresh("skills").unwrap();

    let result = up.adopt("skills", "demo", None, true).unwrap();
    assert_eq!(result.name, "demo");
    let foot = result.foot.expect("adopt 必须写 foot 标记");
    assert!(foot.starts_with("skills/demo@"), "foot 形状 source/path@commit：{foot}");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.root.join("demo/nous.json")).unwrap()).unwrap();
    assert_eq!(manifest["upstream"], serde_json::Value::String(foot.clone()), "foot 要落进 nous.json");
    assert_eq!(fs::read_to_string(f.root.join("demo/attach.md")).unwrap(), "附件原文\n", "整目录 + 附件都要抄过来");

    let states = up.compare("skills", "demo").unwrap();
    assert!(states.iter().any(|s| s.path == "SKILL.md" && s.state == "same"), "刚 adopt 的文件应 same：{states:?}");

    // diff：本地改一个字节 → blob sha 不同。
    fs::write(f.root.join("demo/attach.md"), "本地改过\n").unwrap();
    let states = up.compare("skills", "demo").unwrap();
    assert!(states.iter().any(|s| s.path == "attach.md" && s.state == "diff"), "改过的文件应 diff：{states:?}");

    // missing：本地删掉一个文件。
    fs::remove_file(f.root.join("demo/attach.md")).unwrap();
    let states = up.compare("skills", "demo").unwrap();
    assert!(states.iter().any(|s| s.path == "attach.md" && s.state == "missing"), "删掉的文件应 missing：{states:?}");
}

#[test]
fn copy_does_not_write_foot_and_rejects_duplicate_target() {
    let f = Fixture::new();
    f.seed_skill("demo");
    let up = f.subscribe();
    up.refresh("skills").unwrap();

    let result = up.adopt("skills", "demo", Some("copied"), false).unwrap();
    assert_eq!(result.foot, None, "copy 不写 foot 标记");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.root.join("copied/nous.json")).unwrap()).unwrap();
    assert!(manifest.get("upstream").is_none(), "copy 的 nous.json 不该有 upstream");
    assert_eq!(manifest["copiedFrom"], "skills/demo");

    let err = up.adopt("skills", "demo", Some("copied"), true).unwrap_err();
    assert!(format!("{err:?}").contains("Conflict"), "目标已存在必须 409：{err:?}");
}

fn walk(dir: &Path) -> impl Iterator<Item = (PathBuf, fs::Metadata)> {
    fn collect(dir: &Path, out: &mut Vec<(PathBuf, fs::Metadata)>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        out.push((path, meta));
                    } else if meta.is_dir() {
                        collect(&path, out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.into_iter()
}

fn count_objects(mirror: &Path) -> usize {
    let objects = mirror.join(".git/objects");
    walk(&objects).count()
}

trait PinnedCommit {
    fn pinned_commit(&self) -> &str;
}

impl PinnedCommit for nous_repo::UpstreamStatus {
    fn pinned_commit(&self) -> &str {
        self.source.version.as_ref().map(|v| v.commit.as_str()).unwrap_or_default()
    }
}

// 显式构造 SourceVersion 的构造器也要在公共 API 上（pin/sync 用它写盘）。
#[test]
fn source_version_shape_is_public() {
    let v = SourceVersion { commit: "a".repeat(40), at: "2026-11-20T00:00:00Z".into() };
    assert_eq!(v.commit.len(), 40);
}
