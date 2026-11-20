//! #87：盘面树的领域模型（#86 决策：树用 documents 表达，不复活文件形态）。
//!
//! 一盘面 = 一份 `tree` 文档。整棵树是文档字段，所以建/移动/回收/恢复/清除
//! 都是对同一份文档的一次写，落库时是一次 durable 提交 —— 不存在「父改了、
//! 孩子没搬」的中间态。这里是纯数据（无 IO、无时钟读取）：墓碑时间由调用方传入。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 节点类别。文件夹可以挂孩子，文档是叶子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Folder,
    Doc,
}

/// 树里的一个节点；`title` 可以为空，不生成默认名。`trashed_at` 是 epoch 毫秒，
/// 非空 = 在回收站（从正常树隐藏），空 = 在世。未知字段按 `Value` 收在 `extra` 里透传。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub title: String,
    pub kind: NodeKind,
    pub parent: Option<String>,
    pub order: i64,
    /// 直接孩子（只含在世节点，移动时同步维护）。
    #[serde(default)]
    pub children: Vec<String>,
    #[serde(default, rename = "trashedAt", skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<i64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// 一盘面的整棵树。版本由文档 etag 承担，不在这里另存版本号。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BoardTree {
    #[serde(default)]
    pub nodes: BTreeMap<String, TreeNode>,
    #[serde(default)]
    pub roots: Vec<String>,
}

/// 树上的非法操作，带出命中节点（判据要求 4xx 的 error 点出是哪条边）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    NotFound(String),
    Cycle(String),
    /// 拖到自己的后代下：`(被移动节点, 命中边上的后代)`。
    CycleEdge(String, String),
    Conflict(String),
    Invalid(String),
}

impl TreeError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound(id) => format!("tree node {id} not found"),
            Self::Cycle(id) => format!("tree cycle at node {id}: cannot move into own descendant"),
            Self::CycleEdge(id, hit) => format!("tree cycle: cannot move {id} under its descendant {hit}"),
            Self::Conflict(id) => format!("tree node {id} changed since read"),
            Self::Invalid(what) => format!("invalid tree operation: {what}"),
        }
    }
}

impl BoardTree {
    /// 新节点的 id 必须全树唯一（回收站里的也算，id 永不复用）。
    pub fn create(&mut self, node: TreeNode) -> Result<(), TreeError> {
        if node.id.is_empty() {
            return Err(TreeError::Invalid("empty node id".into()));
        }
        if self.nodes.contains_key(&node.id) {
            return Err(TreeError::Conflict(node.id));
        }
        if let Some(parent) = &node.parent {
            let p = self.nodes.get(parent).ok_or_else(|| TreeError::NotFound(parent.clone()))?;
            if p.kind != NodeKind::Folder {
                return Err(TreeError::Invalid(format!("parent {parent} is not a folder")));
            }
            if p.trashed_at.is_some() {
                return Err(TreeError::Invalid(format!("parent {parent} is trashed")));
            }
        }
        self.attach(&node.id, node.parent.clone(), node.order);
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    /// 一次操作改完源父、目标父、节点自身三处；先做全部校验，最后才 mutate。
    pub fn move_to(&mut self, id: &str, parent: Option<String>, order: i64) -> Result<(), TreeError> {
        let node = self.nodes.get(id).ok_or_else(|| TreeError::NotFound(id.into()))?.clone();
        if node.trashed_at.is_some() {
            return Err(TreeError::Invalid(format!("node {id} is trashed")));
        }
        if parent.as_deref() == Some(id) {
            return Err(TreeError::Cycle(id.into()));
        }
        let mut cursor = parent.clone();
        while let Some(ancestor) = cursor {
            if ancestor == id {
                return Err(TreeError::Cycle(id.into()));
            }
            let node = self.nodes.get(&ancestor)
                .ok_or_else(|| TreeError::NotFound(ancestor.clone()))?;
            if node.parent.as_deref() == Some(id) {
                return Err(TreeError::CycleEdge(id.into(), ancestor));
            }
            cursor = node.parent.clone();
        }
        if let Some(parent) = &parent {
            let p = self.nodes.get(parent).ok_or_else(|| TreeError::NotFound(parent.clone()))?;
            if p.kind != NodeKind::Folder {
                return Err(TreeError::Invalid(format!("parent {parent} is not a folder")));
            }
            if p.trashed_at.is_some() {
                return Err(TreeError::Invalid(format!("parent {parent} is trashed")));
            }
        }
        self.detach(id, node.parent.as_deref());
        self.attach(id, parent.clone(), order);
        self.nodes.get_mut(id).unwrap().parent = parent;
        self.nodes.get_mut(id).unwrap().order = order;
        Ok(())
    }

    /// 回收 = 给节点及全部后代打墓碑，并从正常树上摘掉根节点；恢复 = 回填。
    pub fn trash(&mut self, id: &str, at: i64) -> Result<Vec<String>, TreeError> {
        let mut affected = self.subtree(id)?;
        for node in &affected {
            if self.nodes[node].trashed_at.is_some() {
                return Err(TreeError::Conflict(node.clone()));
            }
        }
        // 墓碑整棵子树：恢复时一并回来，避免父在世而孩子全在回收站的半截状态。
        let parent = self.nodes[id].parent.clone();
        self.detach(id, parent.as_deref());
        for node in &affected {
            self.nodes.get_mut(node).unwrap().trashed_at = Some(at);
        }
        // 子树是根先行；对外统一按孩子先返回。
        affected.reverse();
        Ok(affected)
    }

    pub fn restore(&mut self, id: &str, parent: Option<String>) -> Result<Vec<String>, TreeError> {
        let node = self.nodes.get(id).ok_or_else(|| TreeError::NotFound(id.into()))?.clone();
        if node.trashed_at.is_none() {
            return Err(TreeError::Conflict(id.into()));
        }
        if let Some(parent) = &parent {
            let p = self.nodes.get(parent).ok_or_else(|| TreeError::NotFound(parent.clone()))?;
            if p.kind != NodeKind::Folder || p.trashed_at.is_some() {
                return Err(TreeError::Invalid(format!("parent {parent} is not a live folder")));
            }
        }
        let mut affected = self.subtree(id)?;
        affected.retain(|n| self.nodes[n].trashed_at.is_some());
        for node in &affected {
            self.nodes.get_mut(node).unwrap().trashed_at = None;
        }
        self.attach(id, parent.clone(), node.order);
        self.nodes.get_mut(id).unwrap().parent = parent;
        // 子树是根先行的顺序；恢复按相反顺序（孩子先）返回，便于逐条断言。
        affected.reverse();
        Ok(affected)
    }

    /// 清除不可撤销：墓碑子树从文档里彻底移除。
    pub fn purge(&mut self, id: &str) -> Result<Vec<String>, TreeError> {
        let parent = self.nodes.get(id).ok_or_else(|| TreeError::NotFound(id.into()))?
            .parent.clone();
        if self.nodes[id].trashed_at.is_none() {
            return Err(TreeError::Conflict(id.into()));
        }
        let mut affected = self.subtree(id)?;
        self.detach(id, parent.as_deref());
        for node in &affected {
            self.nodes.remove(node);
        }
        affected.reverse();
        Ok(affected)
    }

    /// 正常树（隐藏墓碑投影）：`{id, parent, children, ...}` 列表，按 parent / order 排好。
    pub fn visible(&self) -> Vec<&TreeNode> {
        let mut live: Vec<&TreeNode> = self.nodes.values()
            .filter(|node| node.trashed_at.is_none())
            .collect();
        live.sort_by(|a, b| a.parent.cmp(&b.parent).then(a.order.cmp(&b.order)).then(a.id.cmp(&b.id)));
        live
    }

    /// 墓碑投影（回收站），按打墓碑的时间再按 id 排。
    pub fn trashed(&self) -> Vec<&TreeNode> {
        let mut dead: Vec<&TreeNode> = self.nodes.values()
            .filter(|node| node.trashed_at.is_some())
            .collect();
        dead.sort_by(|a, b| a.trashed_at.cmp(&b.trashed_at).then(a.id.cmp(&b.id)));
        dead
    }

    fn subtree(&self, id: &str) -> Result<Vec<String>, TreeError> {
        if !self.nodes.contains_key(id) {
            return Err(TreeError::NotFound(id.into()));
        }
        let mut out = vec![id.to_string()];
        let mut i = 0;
        while i < out.len() {
            let parent = out[i].clone();
            let mut kids: Vec<String> = self.nodes.values()
                .filter(|n| n.parent.as_deref() == Some(parent.as_str()))
                .map(|n| n.id.clone()).collect();
            kids.sort();
            for kid in kids {
                if out.contains(&kid) {
                    return Err(TreeError::Cycle(kid));
                }
                out.push(kid);
            }
            i += 1;
        }
        Ok(out)
    }

    fn detach(&mut self, id: &str, parent: Option<&str>) {
        match parent {
            Some(p) => {
                if let Some(node) = self.nodes.get_mut(p) {
                    node.children.retain(|child| child != id);
                }
            }
            None => self.roots.retain(|root| root != id),
        }
    }

    fn attach(&mut self, id: &str, parent: Option<String>, order: i64) {
        match parent {
            Some(p) => {
                let pos = {
                    let node = self.nodes.get(&p).unwrap();
                    node.children.iter().position(|child| {
                        self.nodes.get(child).is_some_and(|c| c.order > order)
                    }).unwrap_or(node.children.len())
                };
                self.nodes.get_mut(&p).unwrap().children.insert(pos, id.to_string());
                // 维护兄弟的 order：插入后重新编号，免得重复 order 造成顺序含糊。
                let kids = self.nodes.get(&p).unwrap().children.clone();
                for (i, kid) in kids.iter().enumerate() {
                    if let Some(k) = self.nodes.get_mut(kid) { k.order = i as i64; }
                }
            }
            None => {
                let pos = self.roots.iter().position(|root| {
                    self.nodes.get(root).is_some_and(|c| c.order > order)
                }).unwrap_or(self.roots.len());
                self.roots.insert(pos, id.to_string());
                let roots = self.roots.clone();
                for (i, root) in roots.iter().enumerate() {
                    if let Some(r) = self.nodes.get_mut(root) { r.order = i as i64; }
                }
            }
        }
    }

    /// 折返到字段值（写端整份 base 覆盖用）。未知字段不丢。
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }

    pub fn from_value(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    /// 校验整棵树的引用完整性：没有悬空 parent/child、没有环、没有重复 id。
    pub fn check_integrity(&self) -> Result<(), TreeError> {
        let mut seen = BTreeSet::new();
        for (id, node) in &self.nodes {
            if !seen.insert(id.clone()) {
                return Err(TreeError::Conflict(id.clone()));
            }
            if let Some(parent) = &node.parent {
                if !self.nodes.contains_key(parent) {
                    return Err(TreeError::NotFound(parent.clone()));
                }
            }
        }
        for (id, node) in &self.nodes {
            let mut cursor = node.parent.clone();
            let mut hops = 0;
            while let Some(parent) = cursor {
                hops += 1;
                if hops > self.nodes.len() || parent == *id {
                    return Err(TreeError::Cycle(id.clone()));
                }
                cursor = self.nodes.get(&parent).and_then(|p| p.parent.clone());
            }
        }
        Ok(())
    }
}
