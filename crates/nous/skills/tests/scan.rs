//! #71：`SKILL.md` 的盘上真相。测试打在 `nous_skills` 的公开 API 接缝上。
//!
//! 每个断言的名字就是判据的领域陈述；前缀 `rejects_` 的是负向哨兵的靶子。

use std::path::PathBuf;

use nous_skills::{scan, SkillEntry};

/// 临时技能根：三个技能 —— 正常 / 缺 description / 没有 SKILL.md。
struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn three() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let ok = root.join("with-description");
        std::fs::create_dir_all(&ok).unwrap();
        std::fs::write(
            ok.join("SKILL.md"),
            "---\nname: 带描述的技能\ndescription: 从 frontmatter 读出来的那句话\ncategory: [工具, 搜索]\n---\n\n# 正文\n",
        )
        .unwrap();

        let no_desc = root.join("without-description");
        std::fs::create_dir_all(&no_desc).unwrap();
        std::fs::write(no_desc.join("SKILL.md"), "---\nname: 没描述的技能\n---\n\n# 正文\n").unwrap();

        let missing = root.join("missing-file");
        std::fs::create_dir_all(&missing).unwrap();

        Self { _dir: dir, root }
    }

    fn entry<'a>(skills: &'a [SkillEntry], dir_name: &str) -> &'a SkillEntry {
        skills
            .iter()
            .find(|skill| skill.dir.file_name().and_then(|s| s.to_str()) == Some(dir_name))
            .unwrap_or_else(|| panic!("找不到技能 {dir_name}，实际：{skills:?}"))
    }
}

/// 判据 1：三个技能三条；description = 有值 / null / problem；load.state = ok/ok/problem。
#[test]
fn listing_reports_description_and_load_state_from_disk() {
    let repo = Repo::three();
    let scanned = scan(&repo.root);

    assert_eq!(scanned.skills.len(), 3, "盘上三个技能目录就该有三条");
    let ok = Repo::entry(&scanned.skills, "with-description");
    assert_eq!(ok.description.as_deref(), Some("从 frontmatter 读出来的那句话"));
    assert_eq!(ok.load.state, "ok");

    let no_desc = Repo::entry(&scanned.skills, "without-description");
    assert!(no_desc.description.is_none(), "frontmatter 没有 description 就该是 null");
    assert_eq!(no_desc.load.state, "ok", "缺 description 不是加载失败");

    let missing = Repo::entry(&scanned.skills, "missing-file");
    assert_eq!(missing.load.state, "problem", "没有 SKILL.md 不能冒充 ok");
    let detail = missing.load.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("SKILL.md"), "problem 要带原文细节，实际：{detail}");
    assert!(missing.name == "missing-file", "name 缺省用目录名");
}

/// 判据 2：hidden 的技能仍出现在列表里（隐藏是渲染层的事，不是过滤）。
#[test]
fn hidden_skills_are_listed_with_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join("hidden-one");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: 隐藏技能\ndescription: 别显示但别删\ndisable-model-invocation: true\n---\n",
    )
    .unwrap();

    let scanned = scan(dir.path());
    assert_eq!(scanned.skills.len(), 1, "hidden 不能把条目从列表里过滤掉");
    assert_eq!(scanned.skills[0].name, "隐藏技能");
    assert!(scanned.skills[0].hidden, "disable-model-invocation: true 应翻成 hidden");
}

/// 判据 3：attachments 只数「装得进去的东西」—— 跳过 node_modules 与 test。
#[test]
fn attachments_count_only_loadable_files() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join("counted");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\nname: 数附件\n---\n").unwrap();
    std::fs::write(skill.join("a.md"), "附件一").unwrap();
    std::fs::write(skill.join("b.py"), "print(1)").unwrap();

    let scanned = scan(dir.path());
    assert_eq!(scanned.skills[0].attachments, 2, "两个真附件");

    std::fs::create_dir_all(skill.join("node_modules")).unwrap();
    std::fs::write(skill.join("node_modules").join("x.js"), "").unwrap();
    let scanned = scan(dir.path());
    assert_eq!(scanned.skills[0].attachments, 2, "node_modules 里的文件不算附件");

    std::fs::create_dir_all(skill.join("test")).unwrap();
    std::fs::write(skill.join("test").join("t.rs"), "").unwrap();
    let scanned = scan(dir.path());
    assert_eq!(scanned.skills[0].attachments, 2, "test 目录里的文件不算附件");
}

/// 判据 4：categories 去重 —— 两个技能都挂 工具/搜索，词表里只留一条。
#[test]
fn categories_deduplicate_across_skills() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["one", "two"] {
        let skill = dir.path().join(name);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: x\ncategory: [工具/搜索, 写作]\n---\n",
        )
        .unwrap();
    }
    let scanned = scan(dir.path());
    assert_eq!(scanned.categories.len(), 2, "同一类别挂两个技能只留一条");
    assert!(scanned.categories.iter().filter(|c| *c == "工具/搜索").count() == 1);
    assert!(scanned.categories.iter().filter(|c| *c == "写作").count() == 1);
}

/// 判据 4 续：`categories.json` 存在时词表取它（零计数类别保留），不写回。
#[test]
fn categories_file_is_read_but_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let vocab = dir.path().join("categories.json");
    std::fs::write(&vocab, r#"["写作", "空类别"]"#).unwrap();
    let before = std::fs::read(&vocab).unwrap();
    let skill = dir.path().join("one");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\nname: x\ncategory: [工具/搜索]\n---\n").unwrap();

    let scanned = scan(dir.path());
    assert_eq!(scanned.categories.len(), 3, "词表 = 文件 + 技能 category 的并集");
    for wanted in ["工具/搜索", "写作", "空类别"] {
        assert!(scanned.categories.iter().any(|c| c == wanted), "词表缺 {wanted}");
    }
    assert_eq!(std::fs::read(&vocab).unwrap(), before, "categories.json 是只读的");
}

/// 判据 5：tracked 本票不判定 —— 所有条目都是 null，不是 false。
#[test]
fn tracked_is_null_because_this_ticket_never_asks_git() {
    let repo = Repo::three();
    let scanned = scan(&repo.root);
    assert_eq!(scanned.skills.len(), 3);
    for skill in &scanned.skills {
        assert!(
            skill.tracked.is_none(),
            "{} 的 tracked 必须是 null（本票不碰 git），实际 {:?}",
            skill.name,
            skill.tracked
        );
    }
}
