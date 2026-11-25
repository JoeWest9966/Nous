//! #71：判据在公共接缝 —— `GET /v1/skills`（真 Router，进程内 oneshot）。
//!
//! `nous_skills` 的单测证明解析对；这里证明「解析结果真的到了线上端点」，
//! 免得又出现「接线被 let _x = clone() 骗过去」（docs/pitfalls.md）。

mod common;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

static CONTENT_ENV_LOCK: Mutex<()> = Mutex::new(());

struct SkillsEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl SkillsEnv {
    fn cleared() -> Self {
        let guard = CONTENT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = ["NOUS_SKILLS", "NOUS_PROMPTS", "NOUS_PRESETS", "NOUS_EXTEND"]
            .iter()
            .map(|name| {
                let prev = std::env::var_os(name);
                std::env::remove_var(name);
                (*name, prev)
            })
            .collect();
        Self { previous, _guard: guard }
    }

    fn set(&mut self, name: &'static str, value: &std::path::Path) {
        std::env::set_var(name, value);
    }
}

impl Drop for SkillsEnv {
    fn drop(&mut self) {
        for (name, prev) in self.previous.drain(..) {
            match prev {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

async fn get_skills(root: &std::path::Path) -> Value {
    let mut env = SkillsEnv::cleared();
    env.set("NOUS_SKILLS", root);
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let response = app
        .oneshot(Request::builder().uri("/v1/skills").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn fixture() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let ok = root.join("alpha");
        std::fs::create_dir_all(&ok).unwrap();
        std::fs::write(
            ok.join("SKILL.md"),
            "---\nname: 阿尔法\ndescription: 有描述的技能\ncategory: [工具/搜索]\n---\n",
        )
        .unwrap();
        std::fs::write(ok.join("a.md"), "附件一").unwrap();
        std::fs::write(ok.join("b.md"), "附件二").unwrap();
        std::fs::create_dir_all(ok.join("node_modules")).unwrap();
        std::fs::write(ok.join("node_modules").join("x.js"), "").unwrap();

        let no_desc = root.join("beta");
        std::fs::create_dir_all(&no_desc).unwrap();
        std::fs::write(
            no_desc.join("SKILL.md"),
            "---\nname: 贝塔\ncategory: [工具/搜索]\ndisable-model-invocation: true\n---\n",
        )
        .unwrap();

        let missing = root.join("gamma");
        std::fs::create_dir_all(&missing).unwrap();

        Self { _dir: dir, root }
    }

    fn by_name<'a>(body: &'a Value, name: &str) -> &'a Value {
        body["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|skill| skill["name"] == name)
            .unwrap_or_else(|| panic!("找不到技能 {name}，实际 {body}"))
    }
}

/// 判据 1：description = 有值 / null / problem，load.state = ok/ok/problem。
#[tokio::test]
async fn endpoint_reports_description_and_load_state_from_disk() {
    let repo = Repo::fixture();
    let body = get_skills(&repo.root).await;

    assert_eq!(body["skills"].as_array().unwrap().len(), 3);
    assert_eq!(Repo::by_name(&body, "阿尔法")["description"], "有描述的技能");
    assert_eq!(Repo::by_name(&body, "阿尔法")["load"]["state"], "ok");
    assert!(Repo::by_name(&body, "贝塔")["description"].is_null());
    assert_eq!(Repo::by_name(&body, "贝塔")["load"]["state"], "ok");

    // 没有 SKILL.md 的目录：name 缺省用目录名，load.state=problem 并带原文 detail。
    let broken = Repo::by_name(&body, "gamma");
    assert_eq!(broken["name"], "gamma");
    assert_eq!(broken["load"]["state"], "problem");
    let detail = broken["load"]["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("SKILL.md"), "problem 的 detail 要说清缺什么：{detail}");
}

/// 判据 2：hidden 的技能仍在列表里，只带 hidden:true。
#[tokio::test]
async fn endpoint_keeps_hidden_skills_in_the_list() {
    let repo = Repo::fixture();
    let body = get_skills(&repo.root).await;
    assert_eq!(body["skills"].as_array().unwrap().len(), 3);
    assert_eq!(Repo::by_name(&body, "贝塔")["hidden"], true);
    assert_eq!(Repo::by_name(&body, "阿尔法")["hidden"], false);
}

/// 判据 3：attachments 数两个；node_modules 里的不算。
#[tokio::test]
async fn endpoint_counts_attachments_without_node_modules() {
    let repo = Repo::fixture();
    let body = get_skills(&repo.root).await;
    assert_eq!(Repo::by_name(&body, "阿尔法")["attachments"], 2);
}

/// 判据 4：categories 去重；判据 5：tracked 恒 null。
#[tokio::test]
async fn endpoint_deduplicates_categories_and_leaves_tracked_null() {
    let repo = Repo::fixture();
    let body = get_skills(&repo.root).await;
    let categories = body["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 1, "两个技能都挂 工具/搜索，词表只留一条");
    assert_eq!(categories[0], "工具/搜索");
    for skill in body["skills"].as_array().unwrap() {
        assert!(skill["tracked"].is_null(), "tracked 必须是 null，不是 false：{skill}");
    }
}

/// 判据 6：跑完真扫描后整个目录的 sha256 不变（独立入口 `sha256sum`，不看产品代码）。
#[tokio::test]
async fn endpoint_never_writes_to_the_private_repo() {
    let repo = Repo::fixture();
    let before = repo_fingerprint(&repo.root);

    let _ = get_skills(&repo.root).await;

    let after = repo_fingerprint(&repo.root);
    assert_eq!(after, before, "GET /v1/skills 改了私库里的文件");
}

fn repo_fingerprint(root: &std::path::Path) -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let output = std::process::Command::new("sha256sum").arg(&path).output().unwrap();
            assert!(output.status.success(), "sha256sum 失败");
            let sha = String::from_utf8(output.stdout).unwrap();
            (path.display().to_string(), sha.split_whitespace().next().unwrap().to_string())
        })
        .collect()
}

/// 判据 6：repo 块只给 root + present（#63 形状），没有 git 状态字段。
#[tokio::test]
async fn endpoint_repo_block_has_only_root_and_present() {
    let repo = Repo::fixture();
    let body = get_skills(&repo.root).await;
    let block = body["repo"].as_object().expect("repo 是对象");
    let mut keys: Vec<_> = block.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["present", "root"], "repo 块只给 root + present，不给 git 状态");
    assert_eq!(block["root"], Value::String(repo.root.display().to_string()));
    assert_eq!(block["present"], true);
}
