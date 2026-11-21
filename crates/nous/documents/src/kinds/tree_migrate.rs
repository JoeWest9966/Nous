//! #87 迁移：2026-11-18 归档的旧 `tree.json` → documents 的树字段（只读）。
//!
//! 归档在仓外，运行时没有也不该有 `content/` 目录：这里只吃**已经读出来的**
//! `tree.json` JSON 值，加上可选的 `map/<id>/` 目录名清单（用来认墓碑）。
//! 解析完的 BoardTree 由调用方经 documents 写端落 `nous.db` —— 没有第二条写通道。

use std::collections::BTreeSet;

use serde_json::Value;

use super::tree::{BoardTree, NodeKind, TreeNode};

/// 迁移报告：命中行由测试逐项断言，不拿「函数存在」冒充迁移成功。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MigrationReport {
    pub nodes: usize,
    pub folders: usize,
    pub trashed: usize,
    pub skipped: Vec<String>,
}

/// 从旧 `tree.json` 的值构造一棵树。旧节点形状 `{id, name, children?, value?}`；
/// 有 `children` 数组的就是组（folder），否则是块（doc）。
pub fn import_tree_json(raw: &Value, map_dirs: &BTreeSet<String>, gone_dirs: &BTreeSet<String>)
    -> Result<(BoardTree, MigrationReport), String>
{
    let Some(root) = raw.get("root") else {
        return Err("tree.json has no root".into());
    };
    let mut tree = BoardTree::default();
    let mut report = MigrationReport::default();
    walk(root, None, &mut tree, &mut report)?;

    // 归档里 .gone/ 的节点已经从树里摘掉；map/ 里残留的目录只作为墓碑补进树，
    // 不重建父子关系（旧树是唯一真相，目录只是派生）。
    for id in gone_dirs.iter().chain(map_dirs.iter()) {
        if tree.nodes.contains_key(id) {
            continue;
        }
        if !valid_id(id) {
            report.skipped.push(id.clone());
            continue;
        }
        tree.nodes.insert(id.clone(), TreeNode {
            id: id.clone(), title: id.clone(), kind: NodeKind::Doc,
            parent: None, order: tree.nodes.len() as i64, children: vec![],
            trashed_at: Some(0), extra: Default::default(),
        });
        report.trashed += 1;
    }
    tree.check_integrity().map_err(|e| e.message())?;
    Ok((tree, report))
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn walk(
    raw: &Value, parent: Option<&str>, tree: &mut BoardTree, report: &mut MigrationReport,
) -> Result<(), String> {
    let obj = raw.as_object().ok_or_else(|| "tree node is not an object".to_string())?;
    let id = obj.get("id").and_then(Value::as_str)
        .ok_or_else(|| "tree node has no id".to_string())?;
    if !valid_id(id) {
        return Err(format!("invalid old node id {id}"));
    }
    let name = obj.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let is_folder = obj.get("children").is_some_and(Value::is_array);
    let mut node = TreeNode {
        id: id.into(), title: name, kind: if is_folder { NodeKind::Folder } else { NodeKind::Doc },
        parent: parent.map(str::to_string), order: tree.nodes.len() as i64,
        children: vec![], trashed_at: None, extra: Default::default(),
    };
    // 旧节点可能带 `.trashed` / `.gone` 标记（导出时保留），进来就是墓碑。
    if obj.get("trashed").and_then(Value::as_bool).unwrap_or(false)
        || obj.get("gone").and_then(Value::as_bool).unwrap_or(false) {
        node.trashed_at = Some(0);
        report.trashed += 1;
    }
    tree.create(node).map_err(|e| e.message())?;
    report.nodes += 1;
    if is_folder { report.folders += 1; }
    if let Some(kids) = obj.get("children").and_then(Value::as_array) {
        for kid in kids {
            walk(kid, Some(id), tree, report)?;
        }
    }
    Ok(())
}
