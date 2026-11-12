//! 「这条对话的设置」—— 输入区左下角那颗 DNA 图标点出来的东西。
//!
//! 双栏：左边七项（模型 / 思考档位 / 技能 / 模板 / 机器 / 目录 / 只读），右边是当前那一项的清单。
//! 左栏每项都带着当前值，扫一眼就知道这条对话开了什么。
//!
//! 跟「新对话」盘面的关系：内容层是同一套做法（模型两列铁路图、技能和模板按类别分组、
//! 目录点一层进一层），但那边改的是**要新建的那条**，这里改的是**屏幕上这条** ——
//! 落点是 `POST /v1/sessions/:id/config`。回包是**生效值**，调用方必须拿它覆盖本地会话，
//! 不能自己把改的值写上去（比如换了模型，服务端会把档位按新模型重夹一次）。
//!
//! 这个模块不碰网络、不存数据，只画。谁拉数据、谁发请求，都在 `app.rs`。

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::*;

use nous_engine::types::{Dirs, Machine, Machines, Models, Prompts, Session, Skill, Skills};

// ── 面板尺寸（照 demo S3 量的）──────────────────────────────────────────
const PANEL_W: f32 = 560.;
const PANEL_H: f32 = 400.;
const SIDE_W: f32 = 158.;

// 外观
const LINE: u32 = 0xeeeeee;
const SIDE_BG: u32 = 0xfafafb;
const FG: u32 = 0x333333;
const MUTED: u32 = 0x999999;
const FAINT: u32 = 0xb0b0b8;
const HOT: u32 = 0x07c160;
const HOT_BG: u32 = 0xf0fbf4;
const MONO: &str = "Consolas";

/// 左栏七项。顺序就是左栏从上到下的顺序。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tab {
    Model,
    Level,
    Skills,
    Template,
    Machine,
    Dir,
    ReadOnly,
}

impl Tab {
    pub(crate) const ALL: [Tab; 7] = [
        Tab::Model,
        Tab::Level,
        Tab::Skills,
        Tab::Template,
        Tab::Machine,
        Tab::Dir,
        Tab::ReadOnly,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Tab::Model => "模型",
            Tab::Level => "思考档位",
            Tab::Skills => "技能",
            Tab::Template => "模板",
            Tab::Machine => "机器",
            Tab::Dir => "目录",
            Tab::ReadOnly => "只读",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Tab::Model => IconName::Cpu,
            Tab::Level => IconName::Gauge,
            Tab::Skills => IconName::Sparkles,
            Tab::Template => IconName::FileText,
            Tab::Machine => IconName::Monitor,
            Tab::Dir => IconName::Folder,
            Tab::ReadOnly => IconName::Lock,
        }
    }

    /// 左栏那行当前值的小字。窄，所以要短：模型只要 modelId，目录只要末段。
    pub(crate) fn value(self, s: &Session) -> String {
        match self {
            Tab::Model => s
                .model
                .as_deref()
                .map(|m| m.split_once('/').map(|(_, id)| id).unwrap_or(m).to_string())
                .unwrap_or_else(|| "—".into()),
            Tab::Level => s
                .thinking_level
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("—")
                .to_string(),
            Tab::Skills => format!("{}/{}", s.skills.len(), 5),
            Tab::Template => s.template.clone().unwrap_or_else(|| "none".into()),
            Tab::Machine => s.machine.clone().unwrap_or_else(|| "本机".into()),
            Tab::Dir => s
                .cwd
                .as_deref()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                .unwrap_or_else(|| "—".into()),
            Tab::ReadOnly => {
                if s.read_only {
                    "只读".into()
                } else {
                    "可写".into()
                }
            }
        }
    }
}

// ── 回调 ─────────────────────────────────────────────────────────────
//
// 面板只管画，选中了要干什么由 `app.rs` 决定。全部用 `Rc<dyn Fn>`：
// `cx.listener` 拿到的闭包本来就是这个形状，模块之间就不用互相认识对方的类型。
// 参数一律传 `&T`：`cx.listener` 给的闭包就是这个形状（`|this, arg: &T, window, cx|`），
// 按值传的话调用方还得再包一层。
pub(crate) type OnTab = Rc<dyn Fn(&Tab, &mut Window, &mut App)>;
pub(crate) type OnLevel = Rc<dyn Fn(&String, &mut Window, &mut App)>;
/// `(provider, modelId)` —— 拆开传，省得再拼一次 `"p/m"` 又拆回来。
/// `modelId` 为空 = 只换了供应商，模型要跟着换成这家的第一个。
pub(crate) type OnModel = Rc<dyn Fn(&(String, String), &mut Window, &mut App)>;
pub(crate) type OnSkill = Rc<dyn Fn(&String, &mut Window, &mut App)>;
pub(crate) type OnTemplate = Rc<dyn Fn(&Option<String>, &mut Window, &mut App)>;
pub(crate) type OnMachine = Rc<dyn Fn(&Option<String>, &mut Window, &mut App)>;
/// 进入某个目录（`{"path": ...}` 那次请求由调用方发）。
pub(crate) type OnDir = Rc<dyn Fn(&String, &mut Window, &mut App)>;
pub(crate) type OnReadOnly = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

pub(crate) struct Hooks {
    pub tab: OnTab,
    pub level: OnLevel,
    pub model: OnModel,
    pub skill: OnSkill,
    pub template: OnTemplate,
    pub machine: OnMachine,
    pub dir: OnDir,
    pub readonly: OnReadOnly,
}

/// 面板自己的状态。**配置值不在这里** —— 那是会话的真相，在 `sessions` 里。
/// 这里只放「清单拉到了没」「当前看哪一页」这类纯界面状态。
#[derive(Default)]
pub(crate) struct Panel {
    pub open: bool,
    pub tab: Option<Tab>,
    pub models: Option<Models>,
    pub prompts: Option<Prompts>,
    pub machines: Option<Machines>,
    pub dirs: Option<Dirs>,
    pub loading: bool,
    /// 有一个清单挂了就把话说出来，别让人对着白框猜。
    pub error: Option<String>,
    /// 正在写 `/config`：这期间把清单点灰，防连点。
    pub busy: bool,
}

impl Panel {
    pub(crate) fn tab(&self) -> Tab {
        self.tab.unwrap_or(Tab::Model)
    }
}

// ── 小组件 ───────────────────────────────────────────────────────────

fn icon(name: IconName, size: f32, color: u32) -> impl IntoElement {
    Icon::new(name).size(px(size)).text_color(rgb(color))
}

fn side_row(tab: Tab, s: &Session, active: bool, hooks: &Hooks) -> AnyElement {
    let on_tab = hooks.tab.clone();
    div()
        .id(ElementId::Name(format!("cfg-side-{:?}", tab).into()))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(9.))
        .py(px(7.))
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(12.5))
        .when(active, |el| el.bg(rgb(HOT_BG)).text_color(rgb(0x0a7a3f)))
        .when(!active, |el| el.text_color(rgb(0x444444)))
        .hover(|s| s.bg(rgb(0xf0f0f3)))
        .on_click(move |_, window, cx| on_tab(&tab, window, cx))
        .child(icon(
            tab.icon(),
            16.,
            if active { 0x0a7a3f } else { 0x8a8f96 },
        ))
        .child(div().flex_none().child(tab.title()))
        .child(
            div()
                .ml_auto()
                .flex_none()
                .max_w(px(58.))
                .truncate()
                .font_family(MONO)
                .text_size(px(10.))
                .text_color(rgb(if active { 0x3d9a68 } else { FAINT }))
                .child(tab.value(s)),
        )
        .into_any_element()
}

/// 清单里的一行。`mark` 是左边那个小方块里的字符（勾 / 圆点 / 空）。
fn item_row(
    id: String,
    mark: &'static str,
    round: bool,
    on: bool,
    name: String,
    desc: String,
    trailing: Option<String>,
    tag: Option<&'static str>,
    busy: bool,
    on_click: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .flex()
        .items_start()
        .gap(px(9.))
        .w_full()
        .px(px(13.))
        .py(px(7.))
        // 选中/悬停上底色；行本身铺白，整行都是可点区域。
        .bg(rgb(if on { HOT_BG } else { 0xffffff }))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0xf7f7f8)))
        .on_click(move |ev, window, cx| on_click(ev, window, cx))
        .child(
            div()
                .mt(px(2.))
                .flex_none()
                .size(px(15.))
                .rounded(px(if round { 7.5 } else { 4. }))
                .border_1()
                .border_color(rgb(if on { HOT } else { 0xcfcfd6 }))
                .when(on, |el| el.bg(rgb(HOT)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.))
                .line_height(px(9.))
                .text_color(rgb(0xffffff))
                .child(mark),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_family(MONO)
                        .text_size(px(12.5))
                        .text_color(rgb(if busy { 0xb0b0b8 } else { FG }))
                        .child(name),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(rgb(0x8f8f97))
                        .child(desc),
                )
                .when_some(trailing, |el, t| {
                    el.child(
                        div()
                            .truncate()
                            .font_family(MONO)
                            .text_size(px(10.))
                            .text_color(rgb(FAINT))
                            .child(t),
                    )
                }),
        )
        .children(tag.map(|t| {
            div()
                .flex_none()
                .mt(px(3.))
                .px(px(5.))
                .rounded(px(4.))
                .bg(rgb(0xeaf9f0))
                .text_size(px(9.5))
                .text_color(rgb(0x0a7a3f))
                .child(t)
        }))
        .into_any_element()
}

/// 类别小标题。`sub` = 子类那层（前面带个 `└`）。
fn group_head(text: String, count: usize, sub: bool) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(7.))
        .pt(px(9.))
        .pb(px(4.))
        .when(sub, |el| el.pl(px(24.)))
        .when(!sub, |el| el.px(px(13.)))
        .text_size(px(if sub { 10.5 } else { 11. }))
        .font_weight(if sub {
            FontWeight::NORMAL
        } else {
            FontWeight::SEMIBOLD
        })
        .text_color(rgb(if sub { 0xb0b0b8 } else { 0x9a9aa2 }))
        .child(if sub { format!("└ {text}") } else { text })
        .child(
            div()
                .ml_auto()
                .font_family(MONO)
                .font_weight(FontWeight::NORMAL)
                .text_color(rgb(0xbcbcc4))
                .child(count.to_string()),
        )
        .into_any_element()
}

fn empty_note(text: &str) -> AnyElement {
    div()
        .py(px(26.))
        .w_full()
        .text_center()
        .text_size(px(12.))
        .text_color(rgb(FAINT))
        .child(text.to_string())
        .into_any_element()
}

// ── 各页 ─────────────────────────────────────────────────────────────

/// 模型：两列铁路图（供应商 → 模型）。
///
/// 档位本来在「新对话」盘面里是第三列，这里拆出去独立成页了 —— 面板右栏只有 377 宽，
/// 三列的话模型名会全被截成 `deepseek-r1-distill-qwe…`，读不出是哪一版。
fn page_model(m: &Models, s: &Session, hooks: &Hooks, busy: bool) -> AnyElement {
    const PITCH: f32 = 27.;
    const PAD: f32 = 8.;
    const W: f32 = PANEL_W - SIDE_W - 24.;

    // 供应商：保持清单里第一次出现的顺序，不排序 —— 排序会让常用的掉到后面去。
    let mut providers: Vec<&str> = Vec::new();
    for it in &m.items {
        if !providers.contains(&it.provider.as_str()) {
            providers.push(it.provider.as_str());
        }
    }
    let cur = s
        .model
        .as_deref()
        .and_then(|x| x.split_once('/'))
        .map(|(p, id)| (p.to_string(), id.to_string()))
        .unwrap_or_default();
    let models: Vec<&str> = m
        .items
        .iter()
        .filter(|it| it.provider == cur.0)
        .map(|it| it.model_id.as_str())
        .collect();

    let rows = providers.len().max(models.len()).max(1);
    let height = rows as f32 * PITCH + PAD * 2.;
    let col = [W * 0.34, W * 0.66];
    let xs = [0., col[0]];

    let mut layer = div().relative().h(px(height)).w_full();
    let trunk = rgb(0xe2e2e6);
    let hot = rgb(HOT);

    let columns: [(&[&str], usize); 2] = [(&providers, 0), (&models, 1)];
    for (items, ci) in columns {
        let x = xs[ci];
        let n = items.len();
        if n > 1 {
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(x + 12.))
                    .top(px(PAD + PITCH / 2.))
                    .w(px(1.))
                    .h(px(PITCH * (n as f32 - 1.)))
                    .bg(trunk),
            );
        }
        for (row, item) in items.iter().enumerate() {
            let y = PAD + row as f32 * PITCH;
            let sel = if ci == 0 {
                cur.0 == *item
            } else {
                cur.1 == *item
            };
            layer = layer
                .child(
                    div()
                        .absolute()
                        .left(px(x + 10.))
                        .top(px(y + PITCH / 2. - 2.))
                        .size(px(5.))
                        .rounded_full()
                        .bg(if sel { hot } else { trunk }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x + 17.))
                        .top(px(y + PITCH / 2.))
                        .w(px(7.))
                        .h(px(1.))
                        .bg(if sel { hot } else { trunk }),
                );

            let label = (*item).to_string();
            let (provider, model_id) = (cur.0.clone(), label.clone());
            let on_model = hooks.model.clone();
            layer = layer.child(
                div()
                    .id(ElementId::Name(format!("cfg-model-{ci}-{row}").into()))
                    .absolute()
                    .left(px(x + 26.))
                    .top(px(y))
                    .w(px(col[ci] - 30.))
                    .h(px(PITCH - 6.))
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if sel { hot } else { rgb(0xe6e6ea) })
                    .bg(if sel { rgb(HOT_BG) } else { rgb(0xfafafb) })
                    .text_size(px(10.5))
                    .text_color(rgb(if sel { 0x0a7a3f } else { 0x444444 }))
                    .truncate()
                    .font_family(MONO)
                    .when(!busy, |el| {
                        el.cursor_pointer().hover(|s| s.bg(rgb(0xf0f0f3)))
                    })
                    .on_click(move |_, window, cx| {
                        if !busy {
                            if ci == 0 {
                                // 换供应商：模型跟着换成这家第一个，不然会停在别家的
                                // modelId 上，服务端直接 400。
                                on_model(&(provider.clone(), String::new()), window, cx);
                            } else {
                                on_model(&(provider.clone(), model_id.clone()), window, cx);
                            }
                        }
                    })
                    .child(label),
            );
        }
    }

    div()
        .w_full()
        .px(px(8.))
        .py(px(6.))
        .child(layer)
        .into_any_element()
}

/// 思考档位：单选。档位本来就是独立字段（`thinkingLevel`）。
fn page_level(s: &Session, hooks: &Hooks, busy: bool) -> AnyElement {
    let cur = s
        .thinking_level
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let levels: Vec<String> = if s.thinking_levels.is_empty() {
        // 内核没给就退回五个标准档 —— 但正常情况不会走到这里。
        ["off", "minimal", "low", "medium", "high"]
            .iter()
            .map(|x| x.to_string())
            .collect()
    } else {
        s.thinking_levels
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };

    let mut list = div().flex().flex_col().w_full();
    for (i, lv) in levels.iter().enumerate() {
        let on = cur == *lv;
        let name = lv.clone();
        let on_level = hooks.level.clone();
        list = list.child(item_row(
            format!("cfg-level-{i}"),
            if on { "●" } else { "" },
            true,
            on,
            lv.clone(),
            level_note(lv).to_string(),
            None,
            None,
            busy,
            Rc::new(move |_, window, cx| on_level(&name, window, cx)),
        ));
    }
    list.into_any_element()
}

fn level_note(lv: &str) -> &'static str {
    match lv {
        "off" => "关掉思考",
        "minimal" => "几乎不思考",
        "low" => "想一下",
        "medium" => "默认",
        "high" => "往深里想",
        _ => "",
    }
}

/// 技能：多选，按类别分组（顶层 + `└ 子类`）。
fn page_skills(s: &Session, skills: Option<&Skills>, hooks: &Hooks, busy: bool) -> AnyElement {
    let Some(list) = skills else {
        return empty_note("技能清单加载中…");
    };
    // 跟新对话盘面同一条规矩：隐藏的、没加载成功的都不列出来 ——
    // 选了也不生效，列出来只会让人白点一次。
    let visible: Vec<&Skill> = list
        .skills
        .iter()
        .filter(|x| !x.hidden && x.loadable())
        .collect();
    if visible.is_empty() {
        return empty_note("这个盘上还没装技能");
    }

    let mut out = div().flex().flex_col().w_full();
    // 按顶层类别归组，组内先直接成员、再子类。不能按「顶层|子类」当键 ——
    // 那样「工程」和「工程/事故」会各开一组，看着像两个工程。
    let mut tops: Vec<String> = Vec::new();
    for sk in &visible {
        let top = sk.category.first().cloned().unwrap_or_default();
        if !tops.contains(&top) {
            tops.push(top);
        }
    }
    for (gi, top) in tops.iter().enumerate() {
        let direct: Vec<&&Skill> = visible
            .iter()
            .filter(|x| {
                let t = x.category.first().cloned().unwrap_or_default();
                let sub = if x.category.len() > 1 {
                    x.category.last().cloned().unwrap_or_default()
                } else {
                    String::new()
                };
                &t == top && (sub.is_empty() || &sub == top)
            })
            .collect();
        let mut subs: Vec<(String, Vec<&&Skill>)> = Vec::new();
        for sk in &visible {
            let t = sk.category.first().cloned().unwrap_or_default();
            if &t != top || sk.category.len() <= 1 {
                continue;
            }
            let sub = sk.category.last().cloned().unwrap_or_default();
            if &sub == top {
                continue;
            }
            match subs.iter_mut().find(|(k, _)| *k == sub) {
                Some((_, v)) => v.push(sk),
                None => subs.push((sub, vec![sk])),
            }
        }
        let total = direct.len() + subs.iter().map(|(_, v)| v.len()).sum::<usize>();
        out = out.child(group_head(
            if top.is_empty() {
                "未分类".into()
            } else {
                top.clone()
            },
            total,
            false,
        ));
        for (ri, sk) in direct.iter().enumerate() {
            out = out.child(skill_row(sk, format!("cfg-sk-{gi}-d-{ri}"), s, hooks, busy));
        }
        for (si, (sub, items)) in subs.iter().enumerate() {
            out = out.child(group_head(sub.clone(), items.len(), true));
            for (ri, sk) in items.iter().enumerate() {
                out = out.child(skill_row(
                    sk,
                    format!("cfg-sk-{gi}-{si}-{ri}"),
                    s,
                    hooks,
                    busy,
                ));
            }
        }
    }
    out.into_any_element()
}

fn skill_row(sk: &&Skill, id: String, s: &Session, hooks: &Hooks, busy: bool) -> AnyElement {
    let on = s.skills.contains(&sk.name);
    let name = sk.name.clone();
    let on_skill = hooks.skill.clone();
    item_row(
        id,
        if on { "✓" } else { "" },
        false,
        on,
        sk.name.clone(),
        sk.description
            .clone()
            .unwrap_or_default()
            .replace(['\n', '\r'], " "),
        None,
        sk.tracked.then_some("git"),
        busy,
        Rc::new(move |_, window, cx| on_skill(&name, window, cx)),
    )
}

/// 模板：单选，圆框。再点一次取消 —— 「明确不要模板」也是一个有效选择。
fn page_template(s: &Session, prompts: Option<&Prompts>, hooks: &Hooks, busy: bool) -> AnyElement {
    let Some(list) = prompts else {
        return empty_note("模板清单加载中…");
    };
    if list.prompts.is_empty() {
        return empty_note("这个盘上还没有提示词模板");
    }

    let cur = s.template.clone().unwrap_or_default();
    let mut tops: Vec<String> = Vec::new();
    for p in &list.prompts {
        let top = p.category.first().cloned().unwrap_or_default();
        if !tops.contains(&top) {
            tops.push(top);
        }
    }

    let mut out = div().flex().flex_col().w_full();
    for (gi, top) in tops.iter().enumerate() {
        let group: Vec<_> = list
            .prompts
            .iter()
            .filter(|x| &x.category.first().cloned().unwrap_or_default() == top)
            .collect();
        out = out.child(group_head(
            if top.is_empty() {
                "未分类".into()
            } else {
                top.clone()
            },
            group.len(),
            false,
        ));
        for (ri, p) in group.iter().enumerate() {
            let on = cur == p.name;
            // 再点一次取消：把「不要」也做成一个能走过去的状态。
            let next = if on { None } else { Some(p.name.clone()) };
            let on_template = hooks.template.clone();
            let desc = p
                .description
                .clone()
                .or_else(|| p.summary.clone())
                .unwrap_or_default()
                .replace(['\n', '\r'], " ");
            let is_default = p.name == "smart-ass";
            out = out.child(item_row(
                format!("cfg-tpl-{gi}-{ri}"),
                if on { "●" } else { "" },
                true,
                on,
                p.name.clone(),
                desc,
                None,
                is_default.then_some("默认"),
                busy,
                Rc::new(move |_, window, cx| on_template(&next, window, cx)),
            ));
        }
    }
    out.into_any_element()
}

/// 机器：本机在最上面，`None` 就是它。
fn page_machine(s: &Session, machines: Option<&Machines>, hooks: &Hooks, busy: bool) -> AnyElement {
    let note_local = "qrd3wh 跑在哪儿就是哪儿".to_string();
    let mut rows: Vec<(Option<String>, String, String)> = vec![(None, "本机".into(), note_local)];
    if let Some(list) = machines {
        for Machine {
            name, addr, note, ..
        } in &list.machines
        {
            rows.push((
                Some(name.clone()),
                name.clone(),
                note.clone().or_else(|| addr.clone()).unwrap_or_default(),
            ));
        }
    }

    let mut out = div().flex().flex_col().w_full();
    for (i, (value, label, note)) in rows.into_iter().enumerate() {
        let on = s.machine == value;
        let on_machine = hooks.machine.clone();
        out = out.child(item_row(
            format!("cfg-machine-{i}"),
            if on { "●" } else { "" },
            true,
            on,
            label,
            note,
            None,
            None,
            busy,
            Rc::new(move |_, window, cx| on_machine(&value, window, cx)),
        ));
    }
    out.into_any_element()
}

/// 目录：**导航**，不是勾选。点一层进一层，当前选中的那格用边框表示。
fn page_dir(s: &Session, dirs: Option<&Dirs>, hooks: &Hooks) -> AnyElement {
    let at = dirs.and_then(|d| d.path.clone()).or_else(|| s.cwd.clone());
    let mut out = div().flex().flex_col().w_full();

    if let Some(at) = at {
        let up = at
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "/".into());
        let on_dir = hooks.dir.clone();
        let target = up.clone();
        let label = up.clone();
        out = out.child(
            div()
                .id(ElementId::Name("cfg-dir-up".into()))
                .flex()
                .items_center()
                .gap(px(9.))
                .px(px(13.))
                .py(px(6.))
                .hover(|s| s.bg(rgb(0xf7f7f8)))
                .cursor_pointer()
                .font_family(MONO)
                .text_size(px(12.))
                .on_click(move |_, window, cx| on_dir(&target, window, cx))
                .child(div().text_color(rgb(MUTED)).child(".."))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_color(rgb(MUTED))
                        .child(label),
                ),
        );
    }

    let entries = dirs.map(|d| d.dirs.clone()).unwrap_or_default();
    if entries.is_empty() {
        out = out.child(empty_note("这层没有子目录"));
    }
    for (i, d) in entries.into_iter().enumerate() {
        let path = d.path.clone();
        let selected = s.cwd.as_deref() == Some(path.as_str());
        let on_dir = hooks.dir.clone();
        out = out.child(
            div()
                .id(ElementId::Name(format!("cfg-dir-{i}").into()))
                .flex()
                .items_center()
                .gap(px(9.))
                .px(px(13.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(if selected { rgb(HOT) } else { rgba(0x00000000) })
                .when(selected, |el| el.bg(rgb(HOT_BG)))
                .hover(|s| s.bg(rgb(0xf7f7f8)))
                .cursor_pointer()
                .font_family(MONO)
                .text_size(px(12.))
                .on_click(move |_, window, cx| on_dir(&path, window, cx))
                .child(div().text_color(rgb(MUTED)).child("›"))
                .child(div().flex_1().truncate().child(d.name.clone())),
        );
    }
    out.into_any_element()
}

/// 只读：开关 + 一句「凭什么」。
fn page_readonly(s: &Session, hooks: &Hooks, busy: bool) -> AnyElement {
    let on = s.read_only;
    let next = !on;
    let on_ro = hooks.readonly.clone();
    div()
        .w_full()
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(9.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().text_size(px(13.)).text_color(rgb(FG)).child("只读"))
                .child(
                    div()
                        .id(ElementId::Name("cfg-ro-switch".into()))
                        .ml_auto()
                        .w(px(34.))
                        .h(px(20.))
                        .rounded(px(10.))
                        .bg(rgb(if on { HOT } else { 0xd9d9de }))
                        .relative()
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            if !busy {
                                on_ro(&next, window, cx)
                            }
                        })
                        .child(
                            div()
                                .absolute()
                                .top(px(2.))
                                .left(px(if on { 16. } else { 2. }))
                                .size(px(16.))
                                .rounded_full()
                                .bg(rgb(0xffffff)),
                        ),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .line_height(px(18.))
                .text_color(rgb(0x8f8f97))
                .child(
                    "只读不是「拦住」写操作，是根本不把会写的工具递给模型 —— \
                     它压根不知道自己有这能力。",
                ),
        )
        .into_any_element()
}

// ── 面板本体 ─────────────────────────────────────────────────────────

pub(crate) fn render(p: &Panel, skills: Option<&Skills>, s: &Session, hooks: &Hooks) -> AnyElement {
    let tab = p.tab();
    let busy = p.busy;

    // 左栏
    let mut side = div()
        .w(px(SIDE_W))
        .flex_none()
        .h_full()
        .bg(rgb(SIDE_BG))
        .border_r_1()
        .border_color(rgb(LINE))
        .p(px(6.))
        .flex()
        .flex_col()
        .gap(px(1.));
    for t in Tab::ALL {
        side = side.child(side_row(t, s, t == tab, hooks));
    }

    // 右栏正文。**按页单独判**，不看整包 ready()：
    // 曾经用「三份清单全到位才画」做总闸，结果内核偶发 502 时 `error` 把整块右栏
    // 换成一屏 HTML 报错 —— 连模型两列铁路图都不画了，而模型那份其实早就拉到了。
    // 一份挂了不该把别的拖着一起灰（真机截图抓到过）。
    let body: AnyElement = match tab {
        Tab::Model => match p.models.as_ref() {
            Some(m) => page_model(m, s, hooks, busy),
            None => empty_note(
                p.error
                    .as_deref()
                    .filter(|e| e.contains("模型"))
                    .unwrap_or("模型清单加载中…"),
            ),
        },
        Tab::Level => page_level(s, hooks, busy),
        Tab::Skills => page_skills(s, skills, hooks, busy),
        Tab::Template => page_template(s, p.prompts.as_ref(), hooks, busy),
        Tab::Machine => page_machine(s, p.machines.as_ref(), hooks, busy),
        Tab::Dir => page_dir(s, p.dirs.as_ref(), hooks),
        Tab::ReadOnly => page_readonly(s, hooks, busy),
    };

    let main = div()
        .w(px(PANEL_W - SIDE_W - 1.))
        .flex_none()
        .h_full()
        // 面板是白的，右栏也铺白。
        .bg(rgb(0xffffff))
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(13.))
                .py(px(9.))
                .border_b_1()
                .border_color(rgb(LINE))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(tab.title()),
                )
                .child(
                    div()
                        .ml_auto()
                        .max_w(px(260.))
                        .truncate()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if tab == Tab::Model { 0x0a7a3f } else { FG }))
                        .font_family(if tab == Tab::Model {
                            MONO
                        } else {
                            "Microsoft YaHei UI"
                        })
                        .child(full_value(tab, s)),
                ),
        )
        .child(
            div()
                .id(ElementId::Name("cfg-body".into()))
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .overflow_y_scroll()
                .child(body),
        );

    div()
        .id(ElementId::Name("config-panel".into()))
        // absolute 浮层要 occlude，别让下层的聊天区抢鼠标（库里 dialog 也这么写）。
        .occlude()
        .w(px(PANEL_W))
        .h(px(PANEL_H))
        .flex()
        .flex_row()
        .bg(rgb(0xffffff))
        .rounded(px(12.))
        .border_1()
        .border_color(rgba(0x00000014))
        .overflow_hidden()
        .child(side)
        .child(main)
        .into_any_element()
}

/// 右栏标题右侧那个值。比左栏长，因为这里放得下。
fn full_value(tab: Tab, s: &Session) -> String {
    match tab {
        Tab::Model => s.model.clone().unwrap_or_else(|| "—".into()),
        Tab::Level => tab.value(s),
        Tab::Skills => format!("{} 个", s.skills.len()),
        Tab::Template => s.template.clone().unwrap_or_else(|| "none".into()),
        Tab::Machine => s.machine.clone().unwrap_or_else(|| "本机".into()),
        Tab::Dir => s.cwd.clone().unwrap_or_else(|| "—".into()),
        Tab::ReadOnly => tab.value(s),
    }
}
