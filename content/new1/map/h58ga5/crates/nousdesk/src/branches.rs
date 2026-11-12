//! 分支视图：左列从「全部会话」切成「这条会话的分支」。
//!
//! 外观定的是**折叠树 + 圆钮**（demo 六选一里的 ③）：
//! - 展开键是 22px 圆钮（`+` / `−`），点击区 34px 宽 × 整行高 —— 不要求精准瞄准；
//! - 缩进 14px/层，**封顶 6 层**：再深就不缩了。280px 的左栏经不起无限缩进，
//!   封顶之后层级靠「从谁分叉」那行字读，不靠缩进读；
//! - 收起的节点右边挂 `+N`，N 是它底下所有后代（含更深层）的条数；
//! - 点圆钮 = 展开/收起，点行 = 打开这条会话。两个动作分开，不互相误触。
//!
//! 展开状态只存在内存里：它不是事实，是"我眼下看到哪"，重启就重置。

use std::collections::HashSet;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use nous_engine::types::Session;

use crate::app::{Desktop, LIST_BG, MUTED, activity_of};

/// 缩进宽度 / 层。封顶 [`MAX_DEPTH`] 层。
const INDENT: f32 = 14.;
/// 缩进封顶：第 7 层和第 6 层画在同一列。
const MAX_DEPTH: usize = 6;
/// 圆钮直径。
const GLYPH: f32 = 22.;
/// 圆钮的点击区（横向）。竖向是整行高。
const HIT: f32 = 34.;
/// 行高下限。
const ROW_H: f32 = 50.;

impl Desktop {
    /// 进分支视图：根锁在这条会话上。
    ///
    /// 默认展开两层（根 + 直接子）—— 只展开一层看不出树，全展开又看不完。
    pub(crate) fn enter_branches(&mut self, root: i64, cx: &mut Context<Self>) {
        log::info!("进分支视图：根 {}，后代 {} 条", root, self.descendant_count(root));
        self.branches = true;
        self.branch_of = Some(root);
        let mut expanded = HashSet::new();
        expanded.insert(root);
        for child in self.children_of(root) {
            expanded.insert(child.id);
        }
        self.expanded = expanded;
        cx.notify();
    }

    pub(crate) fn exit_branches(&mut self, cx: &mut Context<Self>) {
        log::info!("回全部视图");
        self.branches = false;
        cx.notify();
    }

    /// 某个会话的直接子，按活跃排（`last.id` 降序，跟主列表一个道理）。
    pub(crate) fn children_of(&self, id: i64) -> Vec<&Session> {
        let mut kids: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.parent.as_ref().map(|p| p.id) == Some(id))
            .collect();
        kids.sort_by(|a, b| activity_of(b).cmp(&activity_of(a)).then_with(|| b.id.cmp(&a.id)));
        kids
    }

    /// 底下所有后代（含更深层）的条数。`+N` 角标和标题都用它。
    pub(crate) fn descendant_count(&self, id: i64) -> usize {
        let mut n = 0usize;
        let mut stack = vec![id];
        let mut seen = HashSet::new();
        while let Some(parent) = stack.pop() {
            for kid in self
                .sessions
                .iter()
                .filter(|s| s.parent.as_ref().map(|p| p.id) == Some(parent))
            {
                // 引擎不该给环，但客户端别因为这个死循环。
                if seen.insert(kid.id) {
                    n += 1;
                    stack.push(kid.id);
                }
            }
        }
        n
    }

    fn toggle_expanded(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        cx.notify();
    }

    fn expand_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.branch_of else { return };
        let mut expanded = HashSet::new();
        expanded.insert(root);
        expanded.extend(self.sessions.iter().map(|s| s.id));
        self.expanded = expanded;
        cx.notify();
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.branch_of else { return };
        // 根留着展开：不然点「全收起」会连第一层都看不见，只剩空白。
        self.expanded = HashSet::from([root]);
        cx.notify();
    }

    /// 分支视图的左栏。
    pub(crate) fn render_branch_list(&self, root: i64, cx: &mut Context<Self>) -> AnyElement {
        let name = self
            .sessions
            .iter()
            .find(|s| s.id == root)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("会话 {root}"));
        let total = self.descendant_count(root);

        let mut rows: Vec<AnyElement> = Vec::new();
        if total == 0 {
            rows.push(
                div()
                    .p(px(24.))
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("还没有分支")
                    .into_any_element(),
            );
        } else {
            for kid in self.children_of(root) {
                self.push_subtree(kid, 0, &mut rows, cx);
            }
        }

        div()
            .flex()
            .flex_col()
            .w(px(280.))
            .h_full()
            .bg(rgb(LIST_BG))
            .child(
                div()
                    .p(px(12.))
                    .child(div().child("NousDesk"))
                    .child(
                        div()
                            .text_size(px(13.))
                            .child(format!("{name} 的分支")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(format!("{total} 条（含更深层）"))
                            .child(
                                div()
                                    // 全展开/全收起是文字按钮，别用图标让人猜。
                                    .id(ElementId::Name("expand-all".into()))
                                    .cursor_pointer()
                                    .text_color(rgb(0x0b6bcb))
                                    .child("全展开")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                        this.expand_all(cx)
                                    })),
                            )
                            .child(
                                div()
                                    .id(ElementId::Name("collapse-all".into()))
                                    .cursor_pointer()
                                    .text_color(rgb(0x0b6bcb))
                                    .child("全收起")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                        this.collapse_all(cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .children(rows),
            )
            .into_any_element()
    }

    fn push_subtree(
        &self,
        node: &Session,
        depth: usize,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        out.push(self.tree_row(node, depth, cx));
        if self.expanded.contains(&node.id) {
            for kid in self.children_of(node.id) {
                self.push_subtree(kid, depth + 1, out, cx);
            }
        }
    }

    fn tree_row(&self, s: &Session, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let id = s.id;
        let kids = self.descendant_count(id);
        let open = self.expanded.contains(&id);
        let selected = self.selected == Some(id);
        let indent = INDENT * (depth.min(MAX_DEPTH) as f32);

        // 圆钮：`+` / `−`。没有子会话时留空位（不出按钮），行的对齐才不会跳。
        let glyph = div()
            .size(px(GLYPH))
            .rounded(px(GLYPH / 2.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(0x333333))
            .when(kids > 0, |el| el.bg(rgb(0xfdfdfd)).border_1().border_color(rgb(0x8c8c8c)))
            .child(if kids == 0 {
                String::new()
            } else if open {
                "−".to_string()
            } else {
                "+".to_string()
            });

        // 点击区永远占着这一列（34px 宽 × 整行高），有没有子会话都在，行才对得齐。
        let mut caret = div()
            .flex_none()
            .w(px(HIT))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .id(ElementId::Name(format!("caret-{id}").into()))
            .children(if kids > 0 { Some(glyph) } else { None });
        if kids > 0 {
            caret = caret
                .cursor_pointer()
                .hover(|el| el.bg(rgb(0xe0e0e0)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    // 圆钮只管展开；点行才是打开会话。别让事件冒到行上去。
                    cx.stop_propagation();
                    this.toggle_expanded(id, cx);
                }));
        }

        let from = s
            .parent
            .as_ref()
            .and_then(|p| self.sessions.iter().find(|x| x.id == p.id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "?".to_string());
        let sub = match crate::app::when_text(s.updated_at) {
            Some(t) => format!("↳ 从 {from} 分叉 · {t}"),
            None => format!("↳ 从 {from} 分叉"),
        };

        div()
            .flex()
            .items_center()
            .min_h(px(ROW_H))
            .py(px(4.))
            .pr(px(10.))
            .border_b(px(1.))
            .border_color(rgb(0xe0e0e0))
            .when(selected, |el| el.bg(rgb(0xd9d9d9)))
            .child(div().flex_none().w(px(indent)))
            .child(caret)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().child(s.name.clone()))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(sub),
                    ),
            )
            .when(kids > 0 && !open, |el| {
                el.child(
                    div()
                        .flex_none()
                        .ml(px(6.))
                        .px(px(6.))
                        .rounded(px(8.))
                        .bg(rgb(0xfff1cf))
                        .text_size(px(11.))
                        .text_color(rgb(0x8a5a00))
                        .child(format!("+{kids}")),
                )
            })
            .id(ElementId::Name(format!("branch-{id}").into()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.choose_session(id, cx);
            }))
            .into_any_element()
    }
}
