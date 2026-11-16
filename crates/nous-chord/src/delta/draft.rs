//! `Draft`：一次事务作用域里的可写视图（上游 `draft.ts` 的 10 行类型 + `tracker` 的草稿操作）。
//!
//! ## 为什么不是 TS 的 `Proxy`
//!
//! 上游用 `Proxy` + `PropertyDescriptor` 把 JS 的 `draft.a.b = x` / `arr.push()` 语法接到
//! overlay 上，descriptor 陷阱只是为了让反射 API 看起来正常。本库的客户端发的是**操作
//! 事件**，不需要模拟 JS 属性语法，所以这里是显式的命令式 API：
//! `set` / `remove` / `append_text` / `splice` / `move`。
//!
//! ## `append_text` 是这条链上最要紧的一个
//!
//! 上游 README:215–249 记了一个数：2 万次追加 40 字到 800KB，`draft.text += chunk` 带检查
//! 2.4 s，无检查 1 ms。**SSE 逐 token 流正是这条路径**，所以 `append_text` 把 chunk 记在
//! [`Draft::appends`] 里，`prepare` 时直接发一条 `a` op，绝不逐次重算前缀。
//!
//! ## 与上游的差别（本票范围内）
//!
//! 上游 `tracker.ts`（2,319 行）里的 overlay 节点树、piece tree、位置缓存、竞争草稿与
//! 生命周期状态机**这一版没有**：`Draft` 建在一份克隆值上，`prepare` 用
//! `diff_revisions(base, value)` 兜底算出其余 op，再让 append 记录覆盖同路径的字符串 op。
//! 也就是说：**语义（发出去的 op 与重放结果）已经有了，性能结构与并发生命周期还没有**。
//! 票 #10 因此不关。

use std::collections::BTreeMap;

use serde_json::Value;

use super::apply::{apply_immutable_trusted, TrustedOps};
use super::diff::diff_revisions;
use super::op::validate_op;
use super::{Op, OpError, Path, Seg};

/// 一次变更的作用域视图。所有方法都返回 `Result` —— 写校验失败不能静默。
#[derive(Debug, Clone)]
pub struct Draft {
    base: Value,
    value: Value,
    /// 路径 → 已经追加过的字符串（按追加顺序拼接）。**只在 `append_text` 里增长。**
    appends: BTreeMap<Path, String>,
}

impl Draft {
    /// 从 base 开一份草稿（base 不被修改）。
    pub fn new(base: &Value) -> Self {
        Self {
            base: base.clone(),
            value: base.clone(),
            appends: BTreeMap::new(),
        }
    }

    /// 当前草稿值。
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// 草稿建立时的 base。
    pub fn base(&self) -> &Value {
        &self.base
    }

    /// 已记录的追加（路径 → 拼接后的 chunk 串）。`prepare` 直接用它们发 `a` op。
    pub fn appends(&self) -> &BTreeMap<Path, String> {
        &self.appends
    }

    pub fn set(&mut self, path: &[Seg], value: Value) -> Result<(), OpError> {
        let op = Op::Set(path.to_vec(), value);
        validate_op(&op)?;
        // 一次 `set` 覆盖该路径上此前的追加记录：再发 `a` 就是错的。
        self.appends.remove(path);
        self.apply_once(op)
    }

    pub fn remove(&mut self, path: &[Seg]) -> Result<(), OpError> {
        let op = Op::Remove(path.to_vec());
        validate_op(&op)?;
        self.appends.remove(path);
        self.apply_once(op)
    }

    /// 追加一段文本。**这是流式路径**：只在扁平 map 里记一笔，不做前缀比较。
    pub fn append_text(&mut self, path: &[Seg], chunk: impl AsRef<str>) -> Result<(), OpError> {
        let chunk = chunk.as_ref();
        let op = Op::Append(path.to_vec(), chunk.to_string());
        validate_op(&op)?;
        // 先确认当前值真的是字符串（否则 `a` 应用不上）。
        self.value_at(path)
            .and_then(Value::as_str)
            .ok_or_else(|| OpError::UnresolvablePath(path.to_vec()))?;
        self.appends
            .entry(path.to_vec())
            .or_default()
            .push_str(chunk);
        self.apply_once(op)
    }

    pub fn splice(
        &mut self,
        path: &[Seg],
        start: usize,
        delete: usize,
        items: Vec<Value>,
    ) -> Result<(), OpError> {
        let op = Op::Splice(path.to_vec(), start, delete, items);
        validate_op(&op)?;
        // 结构变动让该路径及其子路径上的追加记录失效。
        let affected: Vec<Path> = self
            .appends
            .keys()
            .filter(|keyed| keyed.starts_with(path))
            .cloned()
            .collect();
        for key in affected {
            self.appends.remove(&key);
        }
        self.apply_once(op)
    }

    pub fn move_permutation(
        &mut self,
        path: &[Seg],
        permutation: Vec<usize>,
    ) -> Result<(), OpError> {
        let op = Op::Move(path.to_vec(), permutation);
        validate_op(&op)?;
        let affected: Vec<Path> = self
            .appends
            .keys()
            .filter(|keyed| keyed.starts_with(path))
            .cloned()
            .collect();
        for key in affected {
            self.appends.remove(&key);
        }
        self.apply_once(op)
    }

    /// 结算：交出这一批 op（绝对可信，本 crate 自己产的）。
    ///
    /// 顺序：先 `diff_revisions(base, value)` 兜底，再把「有 append 记录的路径」上那条
    /// 字符串 op 换成一条合并好的 `a`。**前缀检测只在没有 append 记录时走** —— 有记录就
    /// 直接用记录，不去切字符串。
    pub fn prepare(self) -> TrustedOps {
        let diff = diff_revisions(&self.base, &self.value).into_ops();
        if self.appends.is_empty() {
            return TrustedOps::unchecked(diff);
        }

        let mut out = Vec::with_capacity(diff.len());
        for op in diff {
            match self.append_override(&op) {
                Some(replacement) => out.extend(replacement),
                None => out.push(op),
            }
        }
        TrustedOps::unchecked(out)
    }

    /// 这条 op 能不能被一条（或几条）追加 op 替掉。
    fn append_override(&self, op: &Op) -> Option<Vec<Op>> {
        let path = op.path()?;
        let chunks = self.appends.get(path)?;
        let before = self.base_at(path)?.as_str()?;
        let after = self.value_at(path)?.as_str()?;
        // 只有「新值 = 旧值 + 追加」时才替：否则记录已经不成立了，交给 diff。
        if !after.starts_with(before) || &after[before.len()..] != chunks.as_str() {
            return None;
        }
        // `Append` 之外，`diff` 也可能给出 `Truncate` + `Append` 或 `Set`（当公共前后缀
        // 让前缀判据不成立时）。字符串 op 都可被一条 `a` 替代 —— 前提是它的效果确实等于
        // 「旧值 + 记录」（上面已经验过）。
        match op {
            Op::Append(..) | Op::Set(..) | Op::Truncate(..) => {
                Some(vec![Op::Append(path.clone(), chunks.clone())])
            }
            _ => None,
        }
    }

    fn apply_once(&mut self, op: Op) -> Result<(), OpError> {
        let value = apply_immutable_trusted(&self.value, &TrustedOps::unchecked(vec![op]))?;
        self.value = value;
        Ok(())
    }

    fn value_at(&self, path: &[Seg]) -> Option<&Value> {
        resolve(&self.value, path)
    }

    fn base_at(&self, path: &[Seg]) -> Option<&Value> {
        resolve(&self.base, path)
    }
}

fn resolve<'a>(root: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    let mut node = root;
    for segment in path {
        node = match (node, segment) {
            (Value::Object(map), Seg::Key(key)) => map.get(key)?,
            (Value::Array(items), Seg::Index(index)) => items.get(*index as usize)?,
            _ => return None,
        };
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{path, utf16_len};
    use serde_json::json;

    #[test]
    fn a_single_set_round_trips_through_prepare_and_apply() {
        let base = json!({"a": 1});
        let mut draft = Draft::new(&base);
        draft.set(&[Seg::Key("b".into())], json!(2)).unwrap();
        let ops = draft.prepare();
        let value = apply_immutable_trusted(&base, &ops).unwrap();
        assert_eq!(value, json!({"a": 1, "b": 2}));
        assert_eq!(base, json!({"a": 1}), "base 不被修改");
    }

    #[test]
    fn no_change_prepares_an_empty_batch() {
        let base = json!({"a": 1});
        assert!(Draft::new(&base).prepare().is_empty());
    }

    #[test]
    fn twenty_thousand_chunks_become_one_append_op() {
        // 上游 README 里那 2 万次追加的场景：op 数必须是 1，不是 20,000。
        let base = json!({"text": ""});
        let mut draft = Draft::new(&base);
        let chunk = "0123456789";
        for _ in 0..20_000 {
            draft.append_text(&[Seg::Key("text".into())], chunk).unwrap();
        }
        let ops = draft.prepare();
        // 关键判据是**条数**（不是 op 形状）：2 万次追加不能变成 2 万条 op。
        assert_eq!(ops.len(), 1, "流式追加不能被拆成 N 条 op");
        // 重放结果与草稿一致。
        let value = apply_immutable_trusted(&base, &ops).unwrap();
        assert_eq!(
            value["text"].as_str().unwrap().len(),
            20_000 * chunk.len(),
            "重放后长度对得上"
        );
    }

    #[test]
    fn a_thousand_small_chunks_stay_one_single_append_op() {
        // 小到不触发「delta 比整根快照还大」的兜底，于是能看到真正的 op 形状。
        let base = json!({"text": ""});
        let mut draft = Draft::new(&base);
        for _ in 0..1_000 {
            draft.append_text(&[Seg::Key("text".into())], "abcde").unwrap();
        }
        let ops = draft.prepare();
        assert_eq!(ops.len(), 1);
        match &ops.as_ops()[0] {
            Op::Append(path, text) => {
                assert_eq!(path, &[Seg::Key("text".into())]);
                assert_eq!(text.len(), 5_000);
            }
            other => panic!("expected one append, got {}", other.verb()),
        }
    }

    #[test]
    fn a_recorded_append_beats_prefix_detection() {
        // 这个形状本来会走 `t` + `a`（公共后缀），有 append 记录时只发一条 `a`。
        let base = json!({"t": "hello world"});
        let mut draft = Draft::new(&base);
        draft.append_text(&[Seg::Key("t".into())], "!").unwrap();
        draft.append_text(&[Seg::Key("t".into())], "!").unwrap();
        let ops = draft.prepare();
        assert_eq!(
            ops.as_ops(),
            &[Op::Append(path(&["t"]), "!!".into())],
            "有 append 记录就不再走前缀/后缀检测"
        );
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"t": "hello world!!"})
        );
    }

    #[test]
    fn a_later_set_invalidates_the_append_record_for_that_path() {
        let base = json!({"t": "abc"});
        let mut draft = Draft::new(&base);
        draft.append_text(&[Seg::Key("t".into())], "d").unwrap();
        draft.set(&[Seg::Key("t".into())], json!("replaced")).unwrap();
        assert!(draft.appends().is_empty());
        let ops = draft.prepare();
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"t": "replaced"})
        );
    }

    #[test]
    fn a_structural_change_drops_append_records_below_it() {
        let base = json!({"xs": [{"t": "a"}]});
        let mut draft = Draft::new(&base);
        draft
            .append_text(&[Seg::Key("xs".into()), Seg::Index(0), Seg::Key("t".into())], "b")
            .unwrap();
        draft.splice(&[Seg::Key("xs".into())], 0, 1, vec![json!({"t": "c"})]).unwrap();
        assert!(draft.appends().is_empty(), "树变了，记录就不成立了");
        let ops = draft.prepare();
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"xs": [{"t": "c"}]})
        );
    }

    #[test]
    fn appending_to_a_non_string_is_an_error_not_a_silent_skip() {
        let base = json!({"n": 1});
        let mut draft = Draft::new(&base);
        assert!(draft.append_text(&[Seg::Key("n".into())], "x").is_err());
        // 不存在的路径同理。
        assert!(draft.append_text(&[Seg::Key("missing".into())], "x").is_err());
        assert!(draft.prepare().is_empty());
    }

    #[test]
    fn emoji_appends_keep_utf16_and_byte_lengths_straight() {
        let base = json!({"t": ""});
        let mut draft = Draft::new(&base);
        draft.append_text(&[Seg::Key("t".into())], "😀").unwrap();
        draft.append_text(&[Seg::Key("t".into())], "é").unwrap();
        let ops = draft.prepare();
        let value = apply_immutable_trusted(&base, &ops).unwrap();
        assert_eq!(value["t"], json!("😀é"));
        assert_eq!(utf16_len(value["t"].as_str().unwrap()), 3);
    }

    #[test]
    fn reserved_segments_are_rejected_at_the_draft_boundary_too() {
        let base = json!({});
        let mut draft = Draft::new(&base);
        let error = draft
            .set(&[Seg::Key("__proto__".into())], json!(true))
            .unwrap_err();
        assert_eq!(error, OpError::UnsafePath("__proto__".into()));
    }
}
