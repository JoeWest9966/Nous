//! `assign_json()`：把一份值**逐叶**写进文档，让一次的提交成本跟着新文本走。
//!
//! 上游：`packages/durable/src/harness/json.ts`（62 行）。
//!
//! ## 为什么不能整块 `set`
//!
//! chord 把「给容器赋值」记成一条完整的 `s`。流式回答每一段 delta 都会让整条消息变长，
//! 整块写就等于每次提交都存一份完整消息、发布一份完整消息 —— 100 ms 一次节流下，一条
//! 16 秒的回答能滚出几百 KB。字符串叶子被**重新赋值成一个更长的字符串**时 chord 才发 `a`，
//! 但它要比较整条字符串才知道能发。如果调用方知道「这个字符串涨了 `text` 这么多」，并且
//! 「已提交的字符串 + `text`」的长度正好等于新字符串，那就直接 `append_text()`，
//! 一次比较都不做。
//!
//! [`AssignPlan`] 就是这个「调用方知道」的表达：[`AssignPlan::Keep`] 表示「这块别动，
//! 调用方自己写」（工具参数就是这么走的 —— 它由 [`crate::harness::partial`] 的改动逐条应用），
//! [`AssignPlan::Fields`] 往下传，[`AssignPlan::Append`] 落到字符串叶子上。
//!
//! ## 数组追加为什么是 `splice` 而不是 `set(index == len)`
//!
//! 四条理由，第 4 条是决定性的：
//!
//! 1. **一条 op 表达一次插入。** `set(len, x)` 追加 N 个元素要 N 条 op，`splice(len, 0, items)`
//!    一条。op 数直接是 `#22` 的出帧体积和 `#25` 的重放成本。
//! 2. **`set(index == len)` 的「追加」是越界边界规则**（`nous-chord/src/delta/apply.rs:177`
//!    `if index == items.len() { items.push(value) }`）。靠规则的副作用表达意图，规则一收紧
//!    就静默变成 `IndexOutOfRange`。
//! 3. **`splice` 的 `start` 会被 clamp**（`apply.rs:164` `start.min(length)`），所以
//!    `splice(len, …)` 永不报错。追加语义本来就与绝对下标无关 —— 数组变短了也照样往末尾插；
//!    而 `set(index)` 用的是绝对位置，数组一缩就写错位或直接失败。
//! 4. **上游的 diff 自己就这么发。** `packages/chord/src/delta/diff.ts:402`：数组结构变动
//!    （含追加）发 `p`（splice）；`:437` 的 `emitSet` 只用于对象键。用 `set(index == len)`
//!    表达追加，等于让「边写边记的 op」和「从值差重新推导的 op」在同一个值上产出不同形状的
//!    批次；两套一旦不一致，`#13` 那条「publication 的 `value` 等于直接读回」的判据就会在
//!    某些路径上对不上，而那种不一致极难查。
//!
//! ## 今天从这里能观察到什么（第一版判据就是撞在这上面才改的）
//!
//! `Draft::prepare` 今天先用 `diff_revisions(base, value)` 兜底，再让 append 记录覆盖**同路径**的
//! 字符串 op。所以非追加的 op 形状由 diff 决定（数组结构变动 → `p`，对象键 → `s`，与上游
//! `diff.ts:402/437` 一致），**本模块唯一决定的是「这个字符串发 `a` 还是 `s`」**。两条由此而来
//! 的纪律：
//!
//! - 追加记录按**路径**覆盖。路径一旦对不上 diff 发的那条 op，追加就白记了（值还对，成本回去了）
//!   —— 所以 `diff.rs::diff_array` 的「1 对 1 区域递归」是这条路径的命根子，见
//!   `docs/chord-delta-notes.md` §1.6/§1.7。
//! - `Draft::splice()` 会作废该路径及其子路径上的全部 append 记录（`draft.rs:115-124`，结构变了、
//!   下标可能挪位）。所以数组长出来时必须**先插尾巴、再递归前缀**，否则刚记下的「正文追加」会被
//!   紧接着的插入清掉。
//!
//! 第二条说清楚它今天值多少：在这个形状下，账本清没清**不改变 ops、也不改变值** —— 数组从 1 项长到
//! 2 项时变动区域不是 1 对 1，diff 整段发一条 `p`，那条 `p` 覆盖掉一切。所以这个顺序是防御性的，
//! 唯一能观测到它的是 `Draft::appends()` 本身：`growing_an_array_keeps_the_append_record` 断言那条
//! 记录还在。它守的不是今天的输出，是「文本只存增量」这个前提 —— 前提一旦靠 diff 的巧合支撑，
//! §1.7 的缺口（semantic LCS 没提货）一动就会静默回退成每次提交带全文，而值全对、没有东西红。
//!
//! ## 与上游的两处有意差别
//!
//! - **`current === value` 用值比较。** 上游比的是 JS 引用：两个结构相同但不同引用的对象也算
//!   「变了」，于是多发一条 `s`。结果值相同，本库比内容，少发一条 op。
//! - **`__proto__` 特例不需要。** 上游要绕开「读到 `Object.prototype` 上的同名成员」的原型链，
//!   Rust 的 `serde_json::Map` 没有这条链。

use std::collections::BTreeMap;

use nous_chord::delta::draft::Draft;
use nous_chord::delta::{OpError, Seg};
use serde_json::Value;

/// 一个值该怎么写进文档，而不是一片叶子一片叶子地整体赋值。
#[derive(Clone, Debug, PartialEq)]
pub enum AssignPlan {
    /// 这个字符串比上次赋值时长出了 `text`：长度对得上就追加。
    Append { text: String },
    /// 原样留着，调用方自己写（工具参数走这条）。
    Keep,
    /// 容器里的某些成员/元素怎么写。没列到的成员整值写。
    Fields(BTreeMap<Seg, AssignPlan>),
}

/// 把 `value` 逐叶写到 `draft` 的 `path` 上。
///
/// `path` 必须非空：根只能整根替换（`r`），而这里写的是文档里的某个容器。上游的调用点也只有
/// 一个 —— `assign_json(generation, "message", …)`，即路径 `["message"]`。
pub fn assign_json(
    draft: &mut Draft,
    path: &[Seg],
    value: &Value,
    plan: Option<&AssignPlan>,
) -> Result<(), OpError> {
    if matches!(plan, Some(AssignPlan::Keep)) {
        return Ok(());
    }
    let fields = match plan {
        Some(AssignPlan::Fields(fields)) => Some(fields),
        _ => None,
    };
    // 先克隆出来：下面要 `&mut draft`，不能同时持有它内部值的引用。
    let current = value_at(draft.value(), path).cloned();
    match (current, value) {
        (Some(Value::Object(current)), Value::Object(wanted)) => {
            // 上游先删「当前有、新值没有」的成员，再逐个成员递归。
            let vanished: Vec<Seg> = current
                .keys()
                .filter(|name| !wanted.contains_key(*name))
                .map(|name| Seg::Key(name.clone()))
                .collect();
            for segment in vanished {
                draft.remove(&child(path, &segment))?;
            }
            for (name, member) in wanted {
                let segment = Seg::Key(name.clone());
                assign_json(
                    draft,
                    &child(path, &segment),
                    member,
                    fields.and_then(|fields| fields.get(&segment)),
                )?;
            }
            Ok(())
        }
        // 只在「不长于新数组」时逐项写：数组变短了没法逐项对，整块赋值更省。
        (Some(Value::Array(current)), Value::Array(wanted)) if current.len() <= wanted.len() => {
            // **先插尾巴，再递归前缀。** `Draft::splice()` 会作废该路径及其子路径上的 append 记录
            // （`draft.rs:115-124`），顺序反过来「正文追加」刚记下就被这次插入清掉。
            // 注意它在今天**不改变 ops 也不改变值**：区域不是 1 对 1，diff 自己就发一条 `p`。
            // 这条顺序守的是「文本只存增量」的前提，能红它的是 `growing_an_array_keeps_the_append_record`
            // 对 `Draft::appends()` 的断言 —— 别删。理由见模块文档第二条。
            if wanted.len() > current.len() {
                // 长出来的部分一次插入（理由见模块注释第 1 条）。
                let tail = wanted[current.len()..].to_vec();
                draft.splice(path, current.len(), 0, tail)?;
            }
            for index in 0..current.len() {
                let segment = Seg::index(index);
                assign_json(
                    draft,
                    &child(path, &segment),
                    &wanted[index],
                    fields.and_then(|fields| fields.get(&segment)),
                )?;
            }
            Ok(())
        }
        (Some(Value::String(current)), Value::String(wanted)) => {
            if can_append(plan, &current, wanted, draft, path) {
                let Some(AssignPlan::Append { text }) = plan else {
                    unreachable!("can_append 已经确认 plan 是 Append");
                };
                draft.append_text(path, text)?;
                return Ok(());
            }
            if current != *wanted {
                draft.set(path, value.clone())?;
            }
            Ok(())
        }
        (current, wanted) => {
            if current.as_ref() != Some(wanted) {
                draft.set(path, value.clone())?;
            }
            Ok(())
        }
    }
}

/// 这条 `Append` 计划能不能用。
///
/// 上游的判据只有一句「已提交的字符串加上 `text` 的长度等于新字符串的长度」。数组元素除外：
/// `appendText()` 在上游只能寻址对象成员，所以数组里的字符串走整值赋值（也就不用比长度了）。
fn can_append(
    plan: Option<&AssignPlan>,
    current: &str,
    wanted: &str,
    draft: &Draft,
    path: &[Seg],
) -> bool {
    let Some(AssignPlan::Append { text }) = plan else {
        return false;
    };
    if parent_is_array(draft.value(), path) {
        return false;
    }
    if current.len() + text.len() != wanted.len() {
        return false;
    }
    // 上游不比前缀，它信任计划来自真 delta 事件。这里多一道只在 debug 里开的检查：
    // 计划给错就当场炸，而不是把值悄悄改成「旧值 + 错的尾巴」。
    debug_assert!(
        wanted.starts_with(current) && wanted[current.len()..] == *text,
        "Append 计划与值不符：{current:?} + {text:?} != {wanted:?}"
    );
    true
}

fn parent_is_array(root: &Value, path: &[Seg]) -> bool {
    let Some((_, parent)) = path.split_last() else {
        return false;
    };
    matches!(value_at(root, parent), Some(Value::Array(_)))
}

pub(super) fn child(path: &[Seg], segment: &Seg) -> Vec<Seg> {
    let mut extended = Vec::with_capacity(path.len() + 1);
    extended.extend_from_slice(path);
    extended.push(segment.clone());
    extended
}

pub(super) fn value_at<'a>(root: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    let mut node = root;
    for segment in path {
        node = match (node, segment) {
            (Value::Object(map), Seg::Key(name)) => map.get(name)?,
            (Value::Array(items), Seg::Index(index)) => items.get(*index as usize)?,
            _ => return None,
        };
    }
    Some(node)
}
