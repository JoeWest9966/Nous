//! #87 第一片：树语义的纯领域判据（原子移动 / 环检测 / 墓碑）。
//! HTTP + 库双向断言在 server/tests/documents_tree.rs，这里是领域不变量的前哨。

use nous_documents::kinds::tree::{BoardTree, NodeKind, TreeNode};

fn node(id: &str, parent: Option<&str>, order: i64) -> TreeNode {
    TreeNode {
        id: id.into(),
        title: id.into(),
        kind: NodeKind::Folder,
        parent: parent.map(str::to_string),
        order,
        children: vec![],
        trashed_at: None,
        extra: Default::default(),
    }
}

#[test]
fn move_rejects_cycles_and_self_reference_without_mutating_the_tree() {
    let mut tree = BoardTree::default();
    tree.create(node("root", None, 0)).unwrap();
    tree.create(node("a", Some("root"), 0)).unwrap();
    tree.create(node("b", Some("a"), 0)).unwrap();
    let before = tree.clone();

    // 自引用与拖到自己的后代下，都必须被拒且树原样不变。
    let self_ref = tree.move_to("a", Some("a".into()), 0);
    let cycle = tree.move_to("a", Some("b".into()), 1);
    assert!(self_ref.is_err(), "自引用必须写不进树：{:?}", self_ref);
    assert!(cycle.is_err(), "环必须写不进树：{:?}", cycle);
    assert_eq!(tree, before, "4xx 之前树不能变（移动要原子）");
    assert!(format!("{:?}", cycle).contains("b"), "错误要带命中节点");
}

#[test]
fn trash_restore_purge_keep_the_tree_consistent() {
    let mut tree = BoardTree::default();
    tree.create(node("root", None, 0)).unwrap();
    tree.create(node("folder", Some("root"), 0)).unwrap();
    tree.create(node("doc", Some("folder"), 0)).unwrap();

    assert_eq!(tree.trash("folder", 1000).unwrap(), vec!["doc", "folder"]);
    assert!(tree.visible().iter().all(|n| n.id != "folder" && n.id != "doc"));
    assert_eq!(tree.trashed().len(), 2);
    assert_eq!(tree.nodes["root"].children, Vec::<String>::new(), "回收要点掉源父的孩子");

    assert_eq!(tree.restore("folder", Some("root".into())).unwrap(), vec!["doc", "folder"]);
    assert_eq!(tree.visible().len(), 3);
    assert!(tree.trashed().is_empty());

    tree.trash("folder", 2000).unwrap();
    assert_eq!(tree.purge("folder").unwrap(), vec!["doc", "folder"]);
    assert_eq!(tree.nodes.len(), 1, "清除是彻底删，不是另一个墓碑");
    tree.check_integrity().unwrap();
}

#[test]
fn roundtrip_preserves_relations_and_unknown_fields() {
    let mut tree = BoardTree::default();
    tree.create(node("root", None, 0)).unwrap();
    let mut child = node("child", Some("root"), 0);
    child.extra.insert("futureField".into(), serde_json::json!({"x": 1}));
    tree.create(child).unwrap();
    let value = tree.to_value();
    let back = BoardTree::from_value(value).unwrap();
    assert_eq!(back.nodes["child"].extra["futureField"], serde_json::json!({"x": 1}));
    back.check_integrity().unwrap();
}

#[test]
fn archived_tree_json_maps_groups_docs_and_gone_dirs_to_tombstones() {
    use std::collections::BTreeSet;
    let raw = serde_json::json!({
        "v": 1, "name": "desk", "version": 3,
        "root": {
            "id": "root", "name": "根目录",
            "children": [
                {"id": "group-a", "name": "甲组", "children": [
                    {"id": "note-1", "name": "笔记一", "value": 1}
                ]},
                {"id": "old-block", "name": "旧块", "trashed": true}
            ]
        }
    });
    let map = BTreeSet::from(["root".into(), "group-a".into(), "note-1".into()]);
    let gone = BTreeSet::from(["deleted-one".into()]);
    let (tree, report) = nous_documents::kinds::tree_migrate::import_tree_json(&raw, &map, &gone).unwrap();
    assert_eq!(report.nodes, 4);
    assert_eq!(report.folders, 2);
    assert_eq!(report.trashed, 2, "旧 trash 标记 + .gone 目录都要变墓碑");
    assert_eq!(tree.nodes["note-1"].parent.as_deref(), Some("group-a"));
    assert!(tree.nodes["deleted-one"].trashed_at.is_some());
    assert_eq!(tree.nodes["root"].title, "根目录");
    tree.check_integrity().unwrap();
}
