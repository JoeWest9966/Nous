//! `assign_json` 的判据：**写出来的值**与「哪些路径能是追加」。
//!
//! 注意职责分界（第一版判据就是撞在这上面才改的）：`Draft::prepare` 今天先用
//! `diff_revisions(base, value)` 兜底、再让 append 记录覆盖同路径的字符串 op。也就是说
//! **非追加的 op 形状是 diff 决定的**（上游 `diff.ts:402`：数组结构变动发 `p`），
//! `assign_json` 唯一能决定的是「这个字符串是 `a` 还是 `s`」。所以这里判的是：
//!
//! - 写完的值（含 `Keep` 留下的坑）；
//! - 该是追加的路径上确实是 `a`、不该是追加的路径上没有 `a`；
//! - 重放这些 op 落到同一个值（`#13` 那条「publication 的 `value` 等于直接读回」的前提）。

use std::collections::BTreeMap;

use nous_chord::delta::apply::{apply_immutable_trusted, TrustedOps};
use nous_chord::delta::draft::Draft;
use nous_chord::delta::{Op, Path, Revision, Seg};
use nous_durable::harness::json::{assign_json, AssignPlan};
use serde_json::{json, Value};

fn message() -> Path {
    vec![Seg::key("message")]
}

fn draft_of(base: &Value) -> Draft {
    Draft::new(base, Revision::ZERO)
}

fn ops_of(draft: Draft) -> Vec<Op> {
    draft.prepare(Revision::ZERO).expect("修订号没变").into_ops()
}

/// 重放这些 op 必须得到给的值 —— 每条判据都过一遍这一关。
fn replayed(base: &Value, ops: &[Op]) -> Value {
    let trusted = TrustedOps::from_decoded(ops.to_vec()).expect("产出的 op 必须自己合法");
    apply_immutable_trusted(base, &trusted).expect("能应用")
}

fn fields(entries: Vec<(Seg, AssignPlan)>) -> AssignPlan {
    AssignPlan::Fields(entries.into_iter().collect::<BTreeMap<_, _>>())
}

fn append(text: &str) -> AssignPlan {
    AssignPlan::Append {
        text: text.to_string(),
    }
}

/// 对象：新成员用 `s`，消失的成员用 `d`。
#[test]
fn a_new_member_is_set_and_a_vanished_member_is_removed() {
    let base = json!({"message": {"a": 1, "b": 2}});
    let value = json!({"a": 1, "c": 3});
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, None).unwrap();
    let ops = ops_of(draft);
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
    assert!(
        !ops.iter().any(|op| matches!(op, Op::Append(..))),
        "对象成员的增删不该出现追加 op，实际是 {ops:?}"
    );
}

/// 字符串叶子按计划追加：一条 `a`，不是一条 `s`。
#[test]
fn a_grown_string_is_appended_instead_of_reassigned() {
    let base = json!({"message": {"content": [{"type": "text", "text": "Hel"}]}});
    let value = json!({"content": [{"type": "text", "text": "Hello"}]});
    let plan = fields(vec![(
        Seg::key("content"),
        fields(vec![(
            Seg::index(0),
            fields(vec![(Seg::key("text"), append("lo"))]),
        )]),
    )]);
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, Some(&plan)).unwrap();
    let ops = ops_of(draft);
    assert_eq!(
        ops,
        vec![Op::Append(
            vec![
                Seg::key("message"),
                Seg::key("content"),
                Seg::index(0),
                Seg::key("text")
            ],
            "lo".to_string()
        )]
    );
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
}

/// 计划说的增量与值对不上（长度不等）：退回整值 `s`，不能追加一条错的尾巴。
#[test]
fn a_mismatched_append_plan_falls_back_to_a_whole_set() {
    let base = json!({"message": {"text": "abc"}});
    let value = json!({"text": "abcdef"});
    let plan = fields(vec![(Seg::key("text"), append("xy"))]);
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, Some(&plan)).unwrap();
    // 计划给错了就不许记追加 —— 记了就是把「旧值 + 错的尾巴」写下去。
    assert!(
        draft.appends().is_empty(),
        "长度对不上的 Append 计划不许落到 append 记录里"
    );
    let ops = ops_of(draft);
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
}

/// `Keep`：这块一个字都不写，调用方自己写（工具参数就是这么走的）。
#[test]
fn keep_writes_nothing() {
    let base = json!({"message": {"content": [{"type": "toolCall", "arguments": {"a": 1}}]}});
    let value = json!({"content": [{"type": "toolCall", "arguments": {}}]});
    let plan = fields(vec![(
        Seg::key("content"),
        fields(vec![(
            Seg::index(0),
            fields(vec![(Seg::key("arguments"), AssignPlan::Keep)]),
        )]),
    )]);
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, Some(&plan)).unwrap();
    assert!(
        ops_of(draft).is_empty(),
        "Keep 的成员不许产生任何 op，连 `s` 也不许"
    );
}

/// 数组变长：`content` 一次插入长出来的那一段，**一条 `p`**，不是每个元素一条 `s`。
#[test]
fn a_growing_array_is_appended_with_one_splice() {
    let base = json!({"message": {"content": [{"type": "text", "text": "a"}]}});
    let value = json!({"content": [
        {"type": "text", "text": "a"},
        {"type": "text", "text": "b"},
        {"type": "text", "text": "c"},
    ]});
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, None).unwrap();
    let ops = ops_of(draft);
    assert_eq!(
        ops,
        vec![Op::Splice(
            vec![Seg::key("message"), Seg::key("content")],
            1,
            0,
            vec![
                json!({"type": "text", "text": "b"}),
                json!({"type": "text", "text": "c"}),
            ],
        )],
        "追加必须是单条 splice"
    );
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
}

/// 数组变短：逐项对不上，整块赋值。
#[test]
fn a_shrinking_array_is_written_whole() {
    let base = json!({"message": {"content": [1, 2, 3]}});
    let value = json!({"content": [1]});
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, None).unwrap();
    let ops = ops_of(draft);
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
    assert!(
        ops.iter().any(|op| matches!(op, Op::Splice(..))),
        "数组结构变动由 diff 发 `p`（上游 diff.ts:402），实际是 {ops:?}"
    );
}

/// 数组里的字符串即使给了 `Append` 计划也走 `s`：上游 `appendText()` 只寻址对象成员。
#[test]
fn a_string_inside_an_array_is_reassigned_even_with_an_append_plan() {
    let base = json!({"message": {"content": ["ab"]}});
    let value = json!({"content": ["abcd"]});
    let plan = fields(vec![(
        Seg::key("content"),
        fields(vec![(Seg::index(0), append("cd"))]),
    )]);
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, Some(&plan)).unwrap();
    // 我们不记追加（上游 `appendText()` 只寻址对象成员）。但**不为追加跟 diff 抢**：
    // diff 对「1 对 1 区域的字符串叶子」自己就会发 `a`（`diff.rs::diff_array` 的递归分支），
    // 那条 `a` 完全正确 —— 这里只是证明「计划不会绕过对象成员的限制」。
    assert!(
        draft.appends().is_empty(),
        "数组元素上的 Append 计划不许被记成追加，实际记了 {:?}",
        draft.appends()
    );
    let ops = ops_of(draft);
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
}

/// 结构相同但引用不同：不发 op（上游比引用会多发一条 `s`，本库比内容 —— 见模块注释）。
#[test]
fn a_structurally_equal_value_writes_nothing() {
    let base = json!({"message": {"content": [{"type": "text", "text": "same"}]}});
    let value = json!({"content": [{"type": "text", "text": "same"}]});
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, None).unwrap();
    assert!(ops_of(draft).is_empty());
}

/// 一层套一层：数组长出来 + 字符串追加 + 成员删掉 + 新成员，重放一次全对上。
///
/// 两个语义点写死在这里：
/// - **值里没有的成员删掉，`Keep` 不豁免删除**（上游 `assignJson` 先删再递归，`plan` 只影响
///   怎么写，不影响「这个键该不该存在」）；
/// - **新元素由 `p` 整体带进来**，对新元素的 `Keep` 没有意义（它不在已提交的数组里）。
#[test]
fn a_mixed_write_replays_to_exactly_the_value_it_was_given() {
    let base = json!({
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "正"}],
            "usage": {"input": 3, "output": 1},
        },
        "run": {"turns": 1},
    });
    let value = json!({
        "role": "assistant",
        "content": [
            {"type": "text", "text": "正文"},
            {"type": "thinking", "thinking": "想一想"},
        ],
        "usage": {"input": 3},
        "stopReason": "end",
    });
    let plan = fields(vec![
        (
            Seg::key("content"),
            fields(vec![
                (Seg::index(0), fields(vec![(Seg::key("text"), append("文"))])),
                (Seg::index(1), AssignPlan::Keep),
            ]),
        ),
        (
            Seg::key("usage"),
            fields(vec![(Seg::key("output"), AssignPlan::Keep)]),
        ),
    ]);
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, Some(&plan)).unwrap();
    assert_eq!(
        draft.appends().len(),
        1,
        "只有正文那一条被记成追加（`content` 的插入不许把它清掉），实际记了 {:?}",
        draft.appends()
    );
    let ops = ops_of(draft);
    // 这一格是**已知缺口**（`docs/chord-delta-notes.md` §1.7）：`content` 从 1 项变 2 项，
    // 变动区域不是 1 对 1，所以整段折成一条 `p`，那次提交带整个区域。块边界是有限次事件，
    // 不是每次提交 —— 每次提交的追加由 `two_blocks_where_the_first_one_grows_append_in_place` 保证。
    assert!(
        ops.iter().any(|op| matches!(op, Op::Splice(..))),
        "块边界这一次是 `p`（已知缺口），实际是 {ops:?}"
    );

    let replayed = replayed(&base, &ops);
    assert_eq!(replayed["message"]["content"][0]["text"], json!("正文"));
    assert_eq!(
        replayed["message"]["content"][1],
        json!({"type": "thinking", "thinking": "想一想"})
    );
    assert_eq!(replayed["message"]["role"], json!("assistant"));
    assert_eq!(replayed["message"]["stopReason"], json!("end"));
    assert_eq!(
        replayed["message"]["usage"],
        json!({"input": 3}),
        "值里没有 `output` 就要真删 —— `Keep` 不豁免删除"
    );
    assert_eq!(replayed["run"], json!({"turns": 1}));
}

/// 流式回答的常态：**首块在自己的 `text` 上变长、后面的块不动**。这一次提交必须只有一条 `a`。
#[test]
fn two_blocks_where_the_first_one_grows_append_in_place() {
    let base = json!({
        "message": {"content": [
            {"type": "text", "text": "第一段"},
            {"type": "text", "text": "第二段"},
        ]},
    });
    let value = json!({"content": [
        {"type": "text", "text": "第一段变长了"},
        {"type": "text", "text": "第二段"},
    ]});
    let plan = fields(vec![(
        Seg::key("content"),
        fields(vec![(
            Seg::index(0),
            fields(vec![(Seg::key("text"), append("变长了"))]),
        )]),
    )]);
    let mut draft = draft_of(&base);
    assign_json(&mut draft, &message(), &value, Some(&plan)).unwrap();
    let ops = ops_of(draft);
    assert_eq!(
        ops,
        vec![Op::Append(
            vec![
                Seg::key("message"),
                Seg::key("content"),
                Seg::index(0),
                Seg::key("text")
            ],
            "变长了".to_string()
        )],
        "每次 partial 提交必须只有一条追加，不许出现整块替换"
    );
    assert_eq!(replayed(&base, &ops), json!({"message": value}));
}
