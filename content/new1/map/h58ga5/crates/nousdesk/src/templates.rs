//! 通讯录（模板页）。从模板开新会话是客户端唯一「从零起步」的路。
//!
//! 版面是两栏：**类别树**（含段切换）+ **预设列表**。段有两条，同一个私仓按段放：
//!
//! - `Session-Template` 会话模板：一条 = 一组「开一条会话时用什么」（**能开**）
//! - `Ultimate-Template` 终极模板：一条 = 触发器 + 参数 + 引用的会话模板（**这一段只读**，
//!   启用触发器的动作用哪个端点还没定，别在客户端发明一个）
//!
//! 每段各有自己的类别词表（`categories.json` 住在段目录里），所以换段 = 换词表 + 换条目 +
//! 换仓状态。**零计数的类别必须画出来**（建了看不见比灰着更糟）。
//!
//! 判据全部来自引擎（`wire.md`）：`specError` 红、`resolved.*.state == "invalid"`
//! 开不了、`tracked === false` 盘上有仓里没。客户端不自己拼规则 ——
//! 这里唯一自己算的是**类别计数**，因为引擎不提供（算法：含后代、同一条预设按路径去重）。

use gpui_kit::component::{Icon, Sizable as _, Size};
// 完整的 lucide 目录在 assets 里（component 的 IconName 只是一小撮兼容子集）。
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use nous_engine::EngineError;
use nous_engine::types::{CreateSession, CreatedSession, Preset, Presets};

use crate::app::{Bar, Desktop, Tab};

/// 两段。`(段的 id, 段标题)` —— id 是 API 的路径段，标题只做人读的按钮字。
/// 标题引擎也会给（`Presets::title`），但按钮在数据到之前就得有字，所以这里留一份。
pub(crate) const SECTIONS: [(&str, &str); 2] = [
    ("Session-Template", "会话模板"),
    ("Ultimate-Template", "终极模板"),
];

/// 未分类不是词表里的类别，是我们自己的兜底桶。用这个哨兵值放进同一个「当前类别」槽。
const UNCATEGORIZED: &str = "∅";

/// 点类别树的一行要选什么。三种只有一种「不是路径」：未分类。
#[derive(Clone)]
enum Pick {
    All,
    Cat(String),
    Uncat,
}

/// 状态行的小字色。
const MUTED: u32 = 0x999999;
const BAD: u32 = 0xd32f2f;
const WARN: u32 = 0xc77700;
const SELECTED_BG: u32 = 0xd8ecd9;
const SELECTED_FG: u32 = 0x14602a;
/// 能开的行：微信通讯录那种「头像方块」的底色。
const AVATAR_BG: u32 = 0x7ba7d7;
/// 左栏（树）比列表深一档，才看得出是两块。
const TREE_BG: u32 = 0xf2f2f2;

/// 一段的全部状态：数据 / 在拉 / 拉失败。
pub(crate) struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub data: Option<Presets>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Section {
    pub(crate) fn new(i: usize) -> Self {
        let (id, title) = SECTIONS[i];
        Self {
            id,
            title,
            data: None,
            loading: false,
            error: None,
        }
    }
}

/// 类别树的一行。**顺序就是引擎给的顺序**（父在子前），客户端不重排。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatNode {
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub count: usize,
    pub has_children: bool,
}

impl Desktop {
    // ── 数据 ────────────────────────────────────────────────

    /// 切左栏入口。切到通讯录时顺手拉当前段（拉过就不重拉）。
    pub(crate) fn switch_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab == Tab::Templates {
            self.load_all_sections(false, cx);
        }
        cx.notify();
    }

    /// 拉当前段。已拉到且非 force 就不动。
    pub(crate) fn load_presets(&mut self, force: bool, cx: &mut Context<Self>) {
        self.load_section(self.section, force, cx);
    }

    /// 两段都拉一遍。段按钮下面要写真实条数，「—」看着像坏了。
    ///
    /// 副作用是想要的：引擎**连上之前**打开过通讯录会留下「还没连上引擎」，
    /// 这里跑一遍会把它清掉（`load_section` 开头就 `error = None`）。
    pub(crate) fn load_all_sections(&mut self, force: bool, cx: &mut Context<Self>) {
        for i in 0..SECTIONS.len() {
            self.load_section(i, force, cx);
        }
    }

    fn load_section(&mut self, i: usize, force: bool, cx: &mut Context<Self>) {
        let sec = &mut self.sections[i];
        if sec.loading {
            return;
        }
        if !force && sec.data.is_some() {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            self.sections[i].error = Some("还没连上引擎".into());
            cx.notify();
            return;
        };
        let section_id = SECTIONS[i].0;
        self.sections[i].loading = true;
        self.sections[i].error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let result: Option<Result<Presets, EngineError>> = match &handle {
                Some(h) => h
                    .spawn(async move { engine.presets(section_id).await })
                    .await
                    .ok(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.sections[i].loading = false;
                match result {
                    Some(Ok(p)) => {
                        // 切段时「能留就留」：数据到了才发现新段没有那个类别，就回「全部」。
                        if let Some(cat) = this.preset_cat.clone() {
                            if cat != UNCATEGORIZED {
                                let tree = facet_tree(&p.categories, &p.presets);
                                if !tree.iter().any(|n| n.path == cat) {
                                    this.preset_cat = None;
                                }
                            }
                        }
                        this.sections[i].data = Some(p);
                    }
                    Some(Err(e)) => this.sections[i].error = Some(e.to_string()),
                    None => this.sections[i].error = Some("任务没跑完".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 换段。段按钮只改「看哪一段」，不动别的栏。
    ///
    /// **能留就留**：新段里还有同一个类别路径就停在那儿，没有才回「全部」。
    /// 数据还没到就先留着，等 `load_section` 回来再判 —— 否则切过去总是被扔回列表顶。
    pub(crate) fn switch_section(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.section == i || i >= SECTIONS.len() {
            return;
        }
        self.section = i;
        self.preset_closed.clear();
        if let Some(d) = &self.sections[i].data {
            let tree = facet_tree(&d.categories, &d.presets);
            self.preset_cat = keep_category(&self.preset_cat, &tree);
        }
        self.load_section(i, false, cx);
        cx.notify();
    }

    /// 选类别（`None` = 全部，`Some("∅")` = 未分类）。
    fn pick_category(&mut self, cat: Option<String>, cx: &mut Context<Self>) {
        self.preset_cat = cat;
        cx.notify();
    }

    fn toggle_closed(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.preset_closed.remove(&path) {
            self.preset_closed.insert(path);
        }
        cx.notify();
    }

    /// 从模板开一条会话：`POST /v1/sessions { preset: "Session-Template/<name>", name }`。
    /// 成功后重拉会话列表、选中新会话、切回聊天页。
    fn launch_preset(&mut self, preset_name: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        // 防连点：开一条的时候别的行先不动。
        if self.creating.is_some() {
            return;
        }
        let section_id = self.sections[self.section].id;
        self.creating = Some(preset_name.clone());
        cx.notify();

        // wire.md：name 带时间，避免重名。
        let name = format!(
            "{} · {}",
            preset_name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() % 86400)
                .unwrap_or(0)
        );
        let body = CreateSession {
            preset: Some(format!("{section_id}/{preset_name}")),
            name: Some(name),
            ..Default::default()
        };

        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            // create → 紧跟一次 sessions()，一次任务里带回落库后的列表。
            let outcome: Option<
                Result<(CreatedSession, Vec<nous_engine::types::Session>), EngineError>,
            > = match &handle {
                Some(h) => h
                    .spawn(async move {
                        let created = engine.create(&body).await?;
                        let sessions = engine.sessions().await?.items;
                        Ok((created, sessions))
                    })
                    .await
                    .ok(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.creating = None;
                match outcome {
                    Some(Ok((created, sessions))) => {
                        let mut sessions = sessions;
                        sessions.sort_by(|a, b| b.id.cmp(&a.id));
                        this.sessions = sessions;
                        this.sections[this.section].error = None;
                        this.tab = Tab::Chats;
                        // 从模板建完也回聊天：盘面不该留在右格里。
                        this.newchat.open = false;
                        this.newchat.sheet = None;
                        this.select_session(created.id, true, cx);
                    }
                    Some(Err(e)) => {
                        this.sections[this.section].error = Some(format!("开不了：{e}"))
                    }
                    None => this.sections[this.section].error = Some("任务没跑完".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── 渲染：类别树 ────────────────────────────────────────

    /// 通讯录的左栏：段切换 + 类别树 + 段脚注。
    pub(crate) fn render_preset_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let sec = &self.sections[self.section];
        let tree = sec
            .data
            .as_ref()
            .map(|d| facet_tree(&d.categories, &d.presets))
            .unwrap_or_default();
        let uncat = sec
            .data
            .as_ref()
            .map(|d| d.presets.iter().filter(|p| p.category.is_empty()).count())
            .unwrap_or(0);
        let total = sec.data.as_ref().map_or(0, |d| d.presets.len());

        // 段按钮：两块并排，选中的白底。字给了两行 —— 名字 + 条数（只要数字，别写「真实」「演示」）。
        let segments = div()
            .flex()
            .flex_row()
            .gap(px(2.))
            .mx(px(10.))
            .mb(px(10.))
            .p(px(3.))
            .rounded(px(9.))
            .bg(rgb(0xe6e6e6))
            .children((0..SECTIONS.len()).map(|i| {
                let active = i == self.section;
                let count = self.sections[i]
                    .data
                    .as_ref()
                    .map_or("—".to_string(), |d| format!("{} 条", d.presets.len()));
                div()
                    .id(ElementId::Name(format!("section-{i}").into()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .flex_1()
                    .py(px(4.))
                    .rounded(px(7.))
                    .when(active, |el| el.bg(rgb(0xffffff)))
                    .when(!active, |el| el.hover(|h| h.bg(rgb(0xececec))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.switch_section(i, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(if active { 0x222222 } else { 0x6a6a6a }))
                            .child(SECTIONS[i].1),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if active { 0x4a8f5c } else { 0xa5a5a5 }))
                            .child(count),
                    )
            }));

        // 全部 + 类别 + 未分类
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(self.cat_row(
            "▦",
            "全部",
            total,
            0,
            self.preset_cat.is_none(),
            None,
            Pick::All,
            cx,
        ));
        for node in &tree {
            // 父级折了 → 整枝不画
            let hidden = ancestors(&node.path)
                .iter()
                .take(node.depth)
                .any(|a| self.preset_closed.contains(a));
            if hidden {
                continue;
            }
            let mine = self.preset_cat.as_deref() == Some(node.path.as_str());
            let caret = node
                .has_children
                .then(|| (node.path.clone(), self.preset_closed.contains(&node.path)));
            rows.push(self.cat_row(
                "",
                &node.label,
                node.count,
                node.depth,
                mine,
                caret,
                Pick::Cat(node.path.clone()),
                cx,
            ));
        }
        rows.push(self.cat_row(
            "·",
            "未分类",
            uncat,
            0,
            self.preset_cat.as_deref() == Some(UNCATEGORIZED),
            None,
            Pick::Uncat,
            cx,
        ));

        let repo_line = match (&sec.data, &sec.error) {
            (Some(d), _) => match &d.repo {
                Some(r) => {
                    let head: String = r.head.as_deref().unwrap_or("—").chars().take(7).collect();
                    if r.dirty {
                        (format!("● 有未提交的改动 · {head}"), WARN)
                    } else {
                        (format!("● 仓干净 · {head}"), 0x4a8f5c)
                    }
                }
                None => ("● 仓状态未知".to_string(), MUTED),
            },
            (None, Some(_)) => ("● 拉不到这一段".to_string(), BAD),
            (None, _) => (format!("● 正在拉 {}", sec.id), MUTED),
        };

        div()
            .flex()
            .flex_col()
            .w(px(super::app::PRESET_TREE_W))
            .h_full()
            .flex_none()
            .bg(rgb(TREE_BG))
            .border_r(px(1.))
            .border_color(rgb(0xe6e6e6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(14.))
                    .pt(px(14.))
                    .pb(px(8.))
                    .child(div().text_size(px(13.)).child("通讯录"))
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(11.))
                            .text_color(rgb(0xaaaaaa))
                            .child(format!("{} 类", tree.len())),
                    ),
            )
            .child(segments)
            .child(
                div()
                    .id("preset-tree-area")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.bar_hover(Bar::Tree, *hovered, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .size_full()
                            .id(ElementId::Name("preset-tree-scroll".into()))
                            .overflow_y_scroll()
                            .track_scroll(&self.tree_scroll)
                            .px(px(8.))
                            .pb(px(8.))
                            .children(rows),
                    )
                    .child(crate::app::scrollbar::overlay(
                        "preset-tree-scrollbar",
                        &self.tree_scroll,
                        self.tree_bar,
                        self.tree_hover,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t(px(1.))
                    .border_color(rgb(0xe6e6e6))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(repo_line.1))
                            .child(repo_line.0),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(0xb0b0b0))
                            .child(format!("{} · {}", sec.id, sec.title)),
                    ),
            )
            .into_any_element()
    }

    /// 类别树的一行。**两段各自一个 hitbox**：箭头管折叠，名字管选类别 ——
    /// 不做嵌套点击（谁吃事件说不清，见 `坑.md` 那条「两套 Div」的同源问题）。
    #[allow(clippy::too_many_arguments)]
    fn cat_row(
        &self,
        glyph: &str,
        label: &str,
        count: usize,
        depth: usize,
        is_current: bool,
        caret: Option<(String, bool)>,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let caret_cell: AnyElement = match caret {
            Some((path, closed)) => div()
                .id(ElementId::Name(format!("caret-{path}").into()))
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.))
                .h(px(20.))
                .rounded(px(4.))
                .hover(|h| h.bg(rgb(0xdedede)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_closed(path.clone(), cx)
                }))
                .child(
                    Icon::new(if closed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .with_size(Size::Size(px(12.)))
                    .text_color(rgb(0xaaaaaa)),
                )
                .into_any_element(),
            None => div().w(px(14.)).into_any_element(),
        };

        let name = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .flex_1()
            .min_w(px(0.))
            .when(!glyph.is_empty(), |el| {
                el.child(
                    div()
                        .w(px(14.))
                        .text_size(px(12.))
                        .text_color(rgb(0x9a9a9a))
                        .child(glyph.to_string()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_size(px(13.))
                    .text_color(rgb(if count == 0 {
                        0xb3b3b3
                    } else if is_current {
                        SELECTED_FG
                    } else {
                        0x444444
                    }))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(if count == 0 { 0xc9c9c9 } else { 0x999999 }))
                    .child(count.to_string()),
            );

        // **点击挂在整行上**（跟会话列表那些行同一个写法）：子元素上的 `on_click` 在这套布局里
        // 收不到事件（踩过，见 `坑.md` 37）。箭头是行的子元素，它自己那一小块也接点击 ——
        // 收不到就算了，主交互（选类别）不受影响。
        div()
            .id(ElementId::Name(format!("cat-{}-{depth}", label).into()))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .pl(px(8. + depth as f32 * 12.))
            .pr(px(8.))
            .h(px(28.))
            .rounded(px(6.))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match &pick {
                Pick::All => this.pick_category(None, cx),
                Pick::Uncat => this.pick_category(Some(UNCATEGORIZED.into()), cx),
                Pick::Cat(path) => this.pick_category(Some(path.clone()), cx),
            }))
            .when(is_current, |el| el.bg(rgb(SELECTED_BG)))
            .when(!is_current, |el| el.hover(|h| h.bg(rgb(0xe7e7e7))))
            .child(caret_cell)
            .child(name)
            .into_any_element()
    }

    // ── 渲染：预设列表 ──────────────────────────────────────

    /// 通讯录的右栏：类别标题 + 条目列表。
    pub(crate) fn render_templates(&self, cx: &mut Context<Self>) -> AnyElement {
        let sec = &self.sections[self.section];
        let uncat = sec
            .data
            .as_ref()
            .map(|d| d.presets.iter().filter(|p| p.category.is_empty()).count())
            .unwrap_or(0);

        let (title, count) = match &self.preset_cat {
            None => (
                "全部".to_string(),
                sec.data.as_ref().map_or(0, |d| d.presets.len()),
            ),
            Some(c) if c == UNCATEGORIZED => ("未分类".to_string(), uncat),
            Some(c) => (
                c.clone(),
                sec.data.as_ref().map_or(0, |d| {
                    d.presets.iter().filter(|p| in_category(p, c)).count()
                }),
            ),
        };

        let body: Vec<AnyElement> = if sec.loading && sec.data.is_none() {
            vec![note("拉模板中…")]
        } else if let Some(e) = &sec.error {
            vec![
                note(&format!("拉模板失败：{e}")),
                // 失败给个重试入口。
                div()
                    .id(ElementId::Name("presets-retry".into()))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.load_presets(true, cx);
                    }))
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(rgb(0x3a6ea5))
                    .child("重试")
                    .into_any_element(),
            ]
        } else {
            match &sec.data {
                None => vec![note("还没数据")],
                Some(d) => {
                    let rows: Vec<&Preset> = match &self.preset_cat {
                        None => d.presets.iter().collect(),
                        Some(c) if c == UNCATEGORIZED => {
                            d.presets.iter().filter(|p| p.category.is_empty()).collect()
                        }
                        Some(c) => d.presets.iter().filter(|p| in_category(p, c)).collect(),
                    };
                    if rows.is_empty() {
                        vec![empty_hint(&title, &sec.title, d.categories.len())]
                    } else {
                        rows.into_iter()
                            .map(|p| self.preset_row(p, cx).into_any_element())
                            .collect()
                    }
                }
            }
        };

        let creating_note = self
            .creating
            .as_ref()
            .map(|name| note(&format!("正在开「{name}」…")));

        div()
            .flex()
            .flex_col()
            .w(px(super::app::TEMPLATE_LIST_W))
            .h_full()
            .flex_none()
            .bg(rgb(super::app::LIST_BG))
            .border_r(px(1.))
            .border_color(rgb(0xe6e6e6))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(6.))
                    .px(px(14.))
                    .pt(px(14.))
                    .pb(px(4.))
                    .child(div().text_size(px(15.)).child(title.clone()))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(format!("{count} 条")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .items_center()
                    .px(px(14.))
                    .pb(px(10.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x9a9a9a))
                            .child(sec.title),
                    )
                    .when(self.preset_cat.is_some(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0xd5d5d5))
                                .child("·"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0x6a6a6a))
                                .child("含子级"),
                        )
                    }),
            )
            .child(
                div()
                    .id("presets-scroll-area")
                    .relative()
                    .flex_1()
                    // 和左栏/聊天区同一条规矩：flex 项不写 `min_h(0)` 就会被内容撑爆，
                    // 撑爆了就不需要滚，滚动条也就永远不出现。
                    .min_h(px(0.))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.bar_hover(Bar::Presets, *hovered, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .size_full()
                            .id(ElementId::Name("presets-scroll".into()))
                            .overflow_y_scroll()
                            .track_scroll(&self.presets_scroll)
                            .children(creating_note)
                            .children(body),
                    )
                    .child(crate::app::scrollbar::overlay(
                        "presets-scrollbar",
                        &self.presets_scroll,
                        self.presets_bar,
                        self.presets_hover,
                    )),
            )
            .into_any_element()
    }

    fn preset_row(&self, p: &Preset, cx: &mut Context<Self>) -> impl IntoElement {
        // 终极模板这一段只读：启用触发器的动作还没定，点它什么也不该发生。
        let can_open = self.sections[self.section].id == SECTIONS[0].0;
        let ok = can_open && launchable(p);
        let name = p.name.clone();
        let creating = self.creating.as_deref() == Some(p.name.as_str());
        let cat = self.preset_cat.clone().unwrap_or_default();

        // 状态字：判据全是引擎算好的，客户端只挑颜色。
        let status: Option<(&str, u32)> = if p.spec_error.is_some() {
            Some(("spec 写坏了", BAD))
        } else if has_invalid(p) {
            Some(("引用失效，现在开不了", BAD))
        } else if !p.tracked {
            Some(("盘上有、仓里没", MUTED))
        } else {
            None
        };

        div()
            .flex()
            .gap(px(10.))
            .p(px(10.))
            .border_b(px(1.))
            .border_color(rgb(0xe0e0e0))
            // 行永远是 Stateful：id 挂住，能开才接 on_click。
            .id(ElementId::Name(format!("preset-{}", p.name).into()))
            .when(ok, |el| {
                el.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.launch_preset(name.clone(), cx);
                }))
            })
            .when(ok, |el| el.hover(|h| h.bg(rgb(0xececec))))
            .when(creating, |el| el.bg(rgb(0xd9e8f7)))
            .child(
                // 头像方块：能开的用蓝底白字首字，开不了的置灰。
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(6.))
                    .bg(rgb(if ok { AVATAR_BG } else { 0xcccccc }))
                    .text_color(rgb(if ok { 0xffffff } else { 0x888888 }))
                    .text_size(px(16.))
                    .child(p.name.chars().next().unwrap_or('?').to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(
                        // 名字一行、摘要一行、chip 一行 —— 列表里不折行（折了行高就跳，扫不动）。
                        div()
                            .whitespace_nowrap()
                            .text_overflow(TextOverflow::Truncate("…".into()))
                            .child(p.name.clone()),
                    )
                    .when(!p.summary.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(0x6a6a6a))
                                .whitespace_nowrap()
                                .text_overflow(TextOverflow::Truncate("…".into()))
                                .child(p.summary.clone()),
                        )
                    })
                    .child(self.chips(p, &cat))
                    .children(
                        status
                            .map(|(text, color)| {
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(color))
                                    .child(text)
                                    .into_any_element()
                            })
                            .into_iter()
                            .collect::<Vec<_>>(),
                    ),
            )
            // 能开的行右侧画个「发起聊天」的箭头，像微信通讯录的入口。
            .when(ok, |el| {
                el.child(
                    div().flex_none().items_center().child(
                        Icon::new(IconName::ChevronRight)
                            .with_size(Size::Size(px(16.)))
                            .text_color(rgb(MUTED)),
                    ),
                )
            })
    }

    /// 一行小 chip：当前类别 / 也属于 / 只读 / 终极模板的触发器与引用。
    fn chips(&self, p: &Preset, current: &str) -> AnyElement {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .overflow_hidden();
        let mut any = false;

        if !current.is_empty() && p.category.iter().any(|c| c == current) {
            row = row.child(chip(current, SELECTED_BG, SELECTED_FG));
            any = true;
        } else if current.is_empty() {
            // 「全部」下不写「也属于」（那时没有当前类别可对照），只把类别平铺出来。
            for c in &p.category {
                row = row.child(chip(c, 0xeeeeee, 0x666666));
                any = true;
            }
        }
        for c in p
            .category
            .iter()
            .filter(|c| !current.is_empty() && c.as_str() != current)
        {
            row = row.child(chip(&format!("也属于 {c}"), 0xffeec9, 0x8a8a8a));
            any = true;
        }
        if read_only(p) {
            row = row.child(chip("只读", 0xe8e8e8, 0x777777));
            any = true;
        }
        if self.sections[self.section].id == SECTIONS[1].0 {
            if let Some(trigger) = spec_str(p, "trigger") {
                row = row.child(chip(&format!("⚡ {trigger}"), 0xfff3d6, 0x8a6d2f));
                any = true;
            }
            if let Some(session) = spec_str(p, "session") {
                row = row.child(chip(&format!("→ {session}"), 0xe8f2ea, 0x4a7a54));
                any = true;
            }
        }
        if !any {
            return div().into_any_element();
        }
        row.into_any_element()
    }
}

// ── 纯函数（能单测的都在这儿） ─────────────────────────────

/// `写作/周报` → `["写作", "写作/周报"]`（含自己）。
pub(crate) fn ancestors(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    (1..=parts.len()).map(|n| parts[..n].join("/")).collect()
}

/// `a` 是不是 `root` 或它的后代。
pub(crate) fn is_under(value: &str, root: &str) -> bool {
    value == root || value.starts_with(&format!("{root}/"))
}

fn depth_of(path: &str) -> usize {
    path.split('/')
        .filter(|s| !s.is_empty())
        .count()
        .saturating_sub(1)
}

fn last_seg(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// 扁平类别表 → 缩进树（带计数）。
///
/// **顺序跟着引擎走**：`categories` 已经是「父在子前、逐段 zh-CN」排好的，这里只补缺（预设上挂着、
/// 词表里没有的路径）。补进来的按「浅的在前、同层按字面」排 —— 只为稳定，不假装那是 zh-CN 排序。
///
/// 计数含后代、**同一条预设按路径去重**：一条预设同时挂 `工程` 和 `工程/审查`，`工程` 只算它一次。
/// 零计数的类别保留（词表里有、还没挂东西）—— 建了看不见比灰着更糟。
pub(crate) fn facet_tree(categories: &[String], presets: &[Preset]) -> Vec<CatNode> {
    let mut order: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = Default::default();
    for c in categories {
        for a in ancestors(c) {
            if seen.insert(a.clone()) {
                order.push(a);
            }
        }
    }
    let mut extra: Vec<String> = Vec::new();
    for p in presets {
        for c in &p.category {
            for a in ancestors(c) {
                if !seen.contains(&a) && !extra.contains(&a) {
                    extra.push(a);
                }
            }
        }
    }
    extra.sort_by(|a, b| depth_of(a).cmp(&depth_of(b)).then_with(|| a.cmp(b)));
    order.extend(extra);

    let mut counts: std::collections::HashMap<String, usize> = Default::default();
    for p in presets {
        let mut mine: std::collections::HashSet<String> = Default::default();
        for c in &p.category {
            for a in ancestors(c) {
                mine.insert(a);
            }
        }
        for a in mine {
            *counts.entry(a).or_default() += 1;
        }
    }

    order
        .iter()
        .map(|path| CatNode {
            path: path.clone(),
            label: last_seg(path),
            depth: depth_of(path),
            count: counts.get(path).copied().unwrap_or(0),
            has_children: order.iter().any(|o| o != path && is_under(o, path)),
        })
        .collect()
}

/// 切段时那个类别能不能留：新段的树里有同一个路径（或它就是未分类）就留，否则回「全部」。
fn keep_category(cat: &Option<String>, tree: &[CatNode]) -> Option<String> {
    match cat.as_deref() {
        None => None,
        Some(UNCATEGORIZED) => Some(UNCATEGORIZED.to_string()),
        Some(p) if tree.iter().any(|n| n.path == p) => Some(p.to_string()),
        _ => None,
    }
}

/// 这条预设算不算在 `cat` 这一类里（父级天然包含整枝）。
fn in_category(p: &Preset, cat: &str) -> bool {
    p.category.iter().any(|c| is_under(c, cat))
}

/// 这条预设是不是只读。`readOnly` 住在 `spec` 里（顶层没有这个字段）。
fn read_only(p: &Preset) -> bool {
    p.spec
        .as_ref()
        .and_then(|s| s.get("readOnly"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 从 `spec` 里读一个字符串字段（终极模板的 `trigger` / `session`）。
fn spec_str(p: &Preset, key: &str) -> Option<String> {
    p.spec
        .as_ref()
        .and_then(|s| s.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// 能开 = spec 没坏 && 没有失效引用。tracked 只影响显示，不影响可开。
fn launchable(p: &Preset) -> bool {
    p.spec_error.is_none() && !has_invalid(p)
}

/// `resolved` 里任何一项 `state == "invalid"` 就开不了。
fn has_invalid(p: &Preset) -> bool {
    p.resolved
        .as_ref()
        .and_then(|v| v.as_object())
        .map(|m| {
            m.values()
                .any(|x| x.get("state").and_then(|s| s.as_str()) == Some("invalid"))
        })
        .unwrap_or(false)
}

fn chip(text: &str, bg: u32, fg: u32) -> AnyElement {
    div()
        .px(px(5.))
        .h(px(16.))
        .rounded(px(3.))
        .bg(rgb(bg))
        .text_size(px(10.))
        .text_color(rgb(fg))
        .child(text.to_string())
        .into_any_element()
}

fn note(text: &str) -> AnyElement {
    div()
        .p(px(12.))
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text.to_string())
        .into_any_element()
}

/// 这一类/这一段里一条都没有时的出路。分三种归因，别都写成「没有数据」。
fn empty_hint(title: &str, section_title: &str, category_count: usize) -> AnyElement {
    let (head, sub) = if title == "全部" {
        (
            format!("「{section_title}」这一段还没有东西"),
            format!("词表里已经有 {category_count} 个类别 —— 去面板建第一条，它就会长出来"),
        )
    } else if title == "未分类" {
        (
            "每条都挂上类别了".to_string(),
            "未分类这一档空着挺好".to_string(),
        )
    } else {
        (
            format!("「{title}」下还没有预设"),
            "词表里先建的类别，还没挂东西".to_string(),
        )
    };
    div()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .items_center()
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(0x6a6a6a))
                .child(head),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(0x9a9a9a))
                .child(sub),
        )
        .into_any_element()
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    // **别写 `use super::*;`**：`super` 链上有 `gpui_kit::*`，它带一个 `test` 宏，
    // 会把 `#[test]` 抢过去（报「recursion limit reached while expanding #[test]」）。
    // `app.rs` 里的测试也是只挑要用的东西，同一个原因。
    use super::{Preset, UNCATEGORIZED, facet_tree, in_category, keep_category};

    fn preset(name: &str, categories: &[&str]) -> Preset {
        Preset {
            name: name.into(),
            summary: String::new(),
            description: String::new(),
            category: categories.iter().map(|c| c.to_string()).collect(),
            spec: None,
            spec_error: None,
            tracked: true,
            dir: String::new(),
            resolved: None,
            extra: Default::default(),
        }
    }

    fn cats(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| c.to_string()).collect()
    }

    /// 顺序完全跟引擎给的那份走（父在子前），客户端不重排。
    #[test]
    fn 树顺序跟引擎一样() {
        let tree = facet_tree(
            &cats(&["工程", "写作", "写作/周报", "Basics"]),
            &[preset("a", &["工程"])],
        );
        let paths: Vec<&str> = tree.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["工程", "写作", "写作/周报", "Basics"]);
        assert_eq!(tree[2].depth, 1);
        assert_eq!(tree[2].label, "周报");
        assert!(tree[1].has_children);
        assert!(!tree[2].has_children);
    }

    /// 零计数的类别也得在（Basics 原来是这种）。
    #[test]
    fn 零计数类别不丢() {
        let tree = facet_tree(&cats(&["工程", "Basics"]), &[preset("a", &["工程"])]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[1].count, 0);
    }

    /// 计数含后代、同一条预设按路径去重。
    #[test]
    fn 计数含后代且去重() {
        let presets = [
            preset("a", &["工程", "工程/审查"]),
            preset("b", &["工程"]),
            preset("c", &["写作/周报"]),
        ];
        let tree = facet_tree(&cats(&["工程", "工程/审查", "写作", "写作/周报"]), &presets);
        let count = |p: &str| tree.iter().find(|n| n.path == p).unwrap().count;
        assert_eq!(count("工程"), 2, "a 挂了父级和子级，只算一次");
        assert_eq!(count("工程/审查"), 1);
        assert_eq!(count("写作"), 1, "只挂子级的也算进父级");
        assert_eq!(count("写作/周报"), 1);
    }

    /// 预设上挂着、词表里没有的路径要补出来（引擎漏了也不能让条目消失）。
    #[test]
    fn 词表缺了也能补() {
        let tree = facet_tree(&cats(&["工程"]), &[preset("a", &["写作/周报"])]);
        let paths: Vec<&str> = tree.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["工程", "写作", "写作/周报"]);
        assert_eq!(tree[2].count, 1);
    }

    #[test]
    fn 父级筛选包含整枝() {
        let p = preset("a", &["写作/周报"]);
        assert!(in_category(&p, "写作"));
        assert!(in_category(&p, "写作/周报"));
        assert!(!in_category(&p, "写作/规格"));
        // 前缀相同但不是一层：`写作/周` 不能命中 `写作/周报`
        assert!(!in_category(&p, "写作/周"));
    }

    /// 切段：新段有同一个类别就留，没有就回全部；未分类是哨兵，永远能留。
    #[test]
    fn 切段时类别能留就留() {
        let tree = facet_tree(&cats(&["工程", "日常"]), &[]);
        assert_eq!(
            keep_category(&Some("工程".into()), &tree),
            Some("工程".into())
        );
        assert_eq!(keep_category(&Some("写作".into()), &tree), None);
        assert_eq!(
            keep_category(&Some(UNCATEGORIZED.into()), &tree),
            Some(UNCATEGORIZED.into())
        );
        assert_eq!(keep_category(&None, &tree), None);
    }
}
