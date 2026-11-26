//! #83：技能正文装配的纯函数判据（顺序、缺失、坏 frontmatter）。

use std::fs;

fn root(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("nous-skill-assemble-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn skill(dir: &std::path::Path, name: &str, body: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n\n{body}\n")).unwrap();
}

#[test]
fn selected_bodies_are_concatenated_in_selection_order_not_directory_order() {
    let repo = root("order");
    skill(&repo.join("alpha"), "alpha", "ALPHA-BODY");
    skill(&repo.join("beta"), "beta", "BETA-BODY");
    let text = library_skill::assemble_prompts(&repo, &["beta".into(), "alpha".into()]);
    let beta = text.find("BETA-BODY").expect("beta 正文进段");
    let alpha = text.find("ALPHA-BODY").expect("alpha 正文进段");
    assert!(beta < alpha, "选择顺序必须压过目录遍历顺序：{text}");
}

#[test]
fn a_missing_or_broken_skill_becomes_a_diagnostic_and_the_rest_still_assembles() {
    let repo = root("diagnostics");
    skill(&repo.join("good"), "good", "GOOD-BODY");
    fs::create_dir_all(repo.join("broken")).unwrap();
    fs::write(repo.join("broken/SKILL.md"), "no frontmatter here").unwrap();
    let text = library_skill::assemble_prompts(&repo, &["missing".into(), "broken".into(), "good".into()]);
    assert!(text.contains("skill: missing (missing)"), "缺技能要有诊断：{text}");
    assert!(text.contains("skill: problem (broken)"), "坏 frontmatter 要有诊断：{text}");
    assert!(text.contains("GOOD-BODY"), "坏条目不拖垮好条目：{text}");
}

#[test]
fn an_empty_selection_assembles_an_empty_section() {
    let repo = root("empty");
    skill(&repo.join("alpha"), "alpha", "ALPHA-BODY");
    assert_eq!(library_skill::assemble_prompts(&repo, &[]), "", "未选技能不该残留技能块");
}
