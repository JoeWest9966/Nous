//! 输入区的 `/` 技能命令 —— 纯逻辑 + 面板渲染。
//!
//! ## 为什么这不是 pi CLI 那个 `/skill:name`
//!
//! 读过 pi CLI 的源码（`pi-coding-agent/dist/core/agent-session.js` 的
//! `_expandSkillCommand`）：它的 `/skill:name` 是**纯文本宏**，把 SKILL.md 全文 inline
//! 塞进这一条 user message，发完就完，不改任何状态 —— 只对下一条消息生效。
//!
//! Nous 不是这套。Nous 内核里 skill 是名牌 `skill:<名>`，也就是一个 extension；
//! `POST /v1/sessions/:id/config {skills:[...]}` 走 `conversation.configure({extensions})`
//! 把选择**持久写进会话的 AgentState**，从这一轮起往后每轮都带，直到再改。
//! 所以这里的 `/` 照搬的只是**交互**（打 `/` 弹面板、过滤、↑↓、↵ 选中），
//! 落地语义是「给这个会话挂上 / 卸下一个技能」。
//!
//! ## 这一层不管什么
//!
//! - 不碰 HTTP（`client.rs` 才发请求）。
//! - 不存输入框内容（query 从 `TextareaState::value()` 现读，只有一份真相）。
//! - 不存技能清单（`Desktop.skills` 是 `GET /v1/skills` 的结果，这里只读）。
//!
//! 唯一的状态是「面板眼下指到第几项 / 用户有没有按 Esc 收起 / 有没有请求在途」，
//! 这些是纯 UI 状态，重启就该没。

use std::rc::Rc;

use nous_engine::types::Skill;

/// 命令态的判断，和 query 的切法。
///
/// 规矩：**整段文本以 `/` 开头，且后面一个空白都没有**，才算在打命令。
/// - `/` → `Some("")`：空 query，列出全部技能。
/// - `/deep` → `Some("deep")`
/// - `/deep-dive 帮我查一下` → `None`：一旦打了空格就退出命令态，这段当普通消息。
///
/// 为什么不学 pi CLI 收参数（`args`）：Nous 的 `/` 是改会话配置、不是拼一条消息，
/// 拼出来的参数无处可去；而「写了一半才发现想发一句以 / 开头的话」必须留得出路。
/// 面板在打到空格那一刻消失，就是给用户的反馈。
pub fn command(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// 面板里能选的技能。
///
/// 两条过滤跟新对话盘面（`board.rs::skill_list`）完全一致，别自己另立一套：
/// - `loadable()`：`load.state != "ok"` 的技能加载不出来，挂上也是死的。
/// - `!hidden`：`hidden` = `disable-model-invocation`。Nous-extend 的 `render()` 里
///   也把 hidden 从 `<available_skills>` 剔掉了，所以挂上它模型根本看不见 ——
///   让用户勾一个不会有反应的东西是骗人。
///
/// query 同时匹配名字和描述，大小写不敏感；结果按名字排，顺序稳定才能用 ↑↓。
pub fn matches(skills: &[Skill], query: &str) -> Vec<Skill> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<Skill> = skills
        .iter()
        .filter(|s| s.loadable() && !s.hidden)
        .filter(|s| {
            q.is_empty()
                || s.name.to_lowercase().contains(&q)
                || s.description
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&q)
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 点一项 = 挂上 / 卸下。返回**完整目标数组**（`/config` 要的就是完整数组，
/// 服务端自己算 remove/add 的相对改法）。
///
/// 为什么是切换而不是「只留这一个」：会话的 `skills` 本来就是集合，新建会话那个
/// 盘面也是多选。要只留一个，先点掉别的就行。
pub fn toggle(current: &[String], name: &str) -> Vec<String> {
    let had = current.iter().any(|s| s == name);
    let mut out: Vec<String> = current
        .iter()
        .filter(|s| s.as_str() != name)
        .cloned()
        .collect();
    if !had {
        out.push(name.to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// 面板的几位纯 UI 状态。
#[derive(Default)]
pub struct Slash {
    /// ↑↓ 指到第几项（对 `matches()` 的结果下标）。
    pub cursor: usize,
    /// Esc 收起过：这段文本就别再弹了，直到内容变（订阅 `InputEvent::Change` 时清）。
    pub dismissed: bool,
    /// 有 config 请求在途：面板显示「应用中…」，并挡住连点。
    pub busy: bool,
    /// 写失败的一句话。挂在输入框上方，不占 `turn_error`（那是「这一轮生成失败」）。
    pub error: Option<String>,
}

impl Slash {
    /// 输入框内容变了：高亮回到第一项，Esc 的收起也作废。
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.dismissed = false;
        self.error = None;
    }

    /// 结果条数变了（过滤词变了 / 清单刷新了），cursor 不能越界。
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.cursor = 0;
        } else if self.cursor >= len {
            self.cursor = len - 1;
        }
    }

    /// ↑↓ 循环走。`delta` 只给 ±1。
    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let cur = self.cursor as isize;
        let next = (cur + delta).rem_euclid(len as isize);
        self.cursor = next as usize;
    }
}

// ── 面板渲染 ────────────────────────────────────────────────────────────
//
// 放在这儿而不是 composer.rs：composer 只管「输入区长什么样」，这一块是
// 另一件事（一份可点的技能清单）。数据全由调用方做成快照递进来，不持有
// Entity、不碰 Desktop 字段。

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const PANEL_BG: u32 = 0xffffff;
const LINE: u32 = 0xe6e6e6;
const TEXT: u32 = 0x2b2b2b;
const SUB: u32 = 0x9a9a9a;
const PICK: u32 = 0xf1f7f3;
const ON: u32 = 0x0a7a3f;

/// 点一行的回调。`&usize` 是被点那项的下标；`Rc` 是为了在每一行里复用同一个闭包。
pub type OnPick = Rc<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// 画面板。`items` 是已经过滤好的，`picked` 是当前会话挂着的技能名。
pub fn panel(
    items: Vec<Skill>,
    picked: Vec<String>,
    cursor: usize,
    busy: bool,
    error: Option<String>,
    // 一条都没有时给的话（清单没拉到 / 加载中），不写就用「没有匹配的技能」。
    empty_note: Option<String>,
    on_pick: OnPick,
) -> AnyElement {
    let empty = items.is_empty();
    let rows: Vec<AnyElement> = items
        .into_iter()
        .enumerate()
        .map(|(ix, skill)| {
            let active = ix == cursor;
            let enabled = picked.iter().any(|n| n == &skill.name);
            let desc = skill.description.clone().unwrap_or_default();
            let cb = Rc::clone(&on_pick);
            div()
                .id(ElementId::Name(format!("slash-row-{ix}").into()))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(34.))
                .px(px(10.))
                .when(active, |el| el.bg(rgb(PICK)))
                .hover(|s| s.bg(rgb(PICK)))
                .cursor_pointer()
                .on_click(move |_event: &ClickEvent, window, cx| cb(&ix, window, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(TEXT))
                                .child(skill.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(SUB))
                                .truncate()
                                .child(desc),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(rgb(if enabled { ON } else { SUB }))
                        .child(if enabled { "已启用" } else { "启用" }),
                )
                .into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_col()
        .w_full()
        .bg(rgb(PANEL_BG))
        .rounded(px(8.))
        .border(px(1.))
        .border_color(rgb(LINE))
        .shadow(vec![
            BoxShadow::new(px(0.), px(-2.), hsla(0., 0., 0., 0.10)).blur_radius(px(10.)),
        ])
        .overflow_hidden()
        // 标题：说清这是「挂到会话」，不是「下一条消息带一下」。
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(28.))
                .px(px(10.))
                .border_b(px(1.))
                .border_color(rgb(LINE))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(TEXT))
                        .child(if busy {
                            "技能 · 应用中…"
                        } else {
                            "技能 · 挂到当前会话"
                        }),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(SUB))
                        .child("↑↓ 选择 · ↵ 启用/停用 · Esc 取消"),
                ),
        )
        .when_some(error, |el, msg| {
            el.child(
                div()
                    .px(px(10.))
                    .py(px(5.))
                    .text_size(px(10.5))
                    .text_color(rgb(0xc0392b))
                    .bg(rgb(0xfff1f0))
                    .child(msg),
            )
        })
        .children(rows)
        .when(empty, |el| {
            el.child(
                div()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(rgb(SUB))
                    .child(empty_note.unwrap_or_else(|| "没有匹配的技能".into())),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // 精确导入，不用 `use super::*`：父模块顶上那句 `use gpui_kit::*` 会把
    // GPUI 的同名 `#[test]` 属性宏带进来，遮蔽 Rust 内置的 `#[test]` ——
    // 症状是「recursion limit reached while expanding #[test]」。
    use super::{Slash, command, matches, toggle};
    use nous_engine::types::Skill;
    use serde_json::json;

    fn skill(name: &str, desc: &str, hidden: bool, load: &str) -> Skill {
        serde_json::from_value(json!({
            "name": name,
            "description": desc,
            "hidden": hidden,
            "dir": "/x",
            "file": "/x/SKILL.md",
            "load": { "state": load },
        }))
        .unwrap()
    }

    #[test]
    fn 命令态只在纯_token时成立() {
        assert_eq!(command("/"), Some(""));
        assert_eq!(command("/deep"), Some("deep"));
        assert_eq!(command("/Deep-Dive"), Some("Deep-Dive"));
        assert_eq!(command(""), None);
        assert_eq!(command("你好 /deep"), None);
        assert_eq!(command("/deep 帮我查"), None);
        assert_eq!(command("/deep\n换行"), None);
        assert_eq!(command("/深 挖"), None);
    }

    #[test]
    fn 过滤_大小写_描述_和不可用的() {
        let all = vec![
            skill("deep-dive", "深挖一个技术问题", false, "ok"),
            skill("weekly-report", "写周报", false, "ok"),
            skill("ask-matt", "路由到别的技能", true, "ok"), // hidden，别让人勾
            skill("broken", "坏了的", false, "error"),       // load 不是 ok
        ];
        assert_eq!(
            matches(&all, "")
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["deep-dive", "weekly-report"]
        );
        assert_eq!(
            matches(&all, "DEEP")
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["deep-dive"]
        );
        // 描述里也搜得到
        assert_eq!(
            matches(&all, "周报")
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["weekly-report"]
        );
        assert!(
            matches(&all, "ask-matt").is_empty(),
            "hidden 的技能不该出现"
        );
        assert!(matches(&all, "broken").is_empty(), "加载失败的技能不该出现");
        assert!(matches(&all, "没有这个").is_empty());
    }

    #[test]
    fn 切换_挂上和卸下() {
        let cur = vec!["a".to_string(), "b".to_string()];
        assert_eq!(toggle(&cur, "c"), vec!["a", "b", "c"]);
        assert_eq!(toggle(&cur, "a"), vec!["b"]);
        assert_eq!(toggle(&[], "z"), vec!["z"]);
        assert_eq!(toggle(&["z".into()], "z"), Vec::<String>::new());
    }

    #[test]
    fn 光标_循环和越界() {
        let mut s = Slash::default();
        s.move_by(-1, 3);
        assert_eq!(s.cursor, 2, "从 0 往上要绕到最后一项");
        s.move_by(1, 3);
        assert_eq!(s.cursor, 0);
        s.cursor = 9;
        s.clamp(2);
        assert_eq!(s.cursor, 1);
        s.clamp(0);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn 输入变了就重置收起和高亮() {
        let mut s = Slash {
            cursor: 4,
            dismissed: true,
            error: Some("上次失败了".into()),
            ..Default::default()
        };
        s.reset();
        assert_eq!(s.cursor, 0);
        assert!(!s.dismissed);
        assert!(s.error.is_none());
    }
}
