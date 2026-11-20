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
