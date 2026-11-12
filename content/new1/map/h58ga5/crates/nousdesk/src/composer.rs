//! 输入区：工具行 / 多行输入 / 发送·停止。
//!
//! 外观照新版微信（4.0）：白圆角卡片浮在聊天区底部、四周一圈浅灰留白；
//! 写字区在上、工具条压在卡片内底部、右下角一颗小「发送」。
//! 鼠标悬停写字区时第一行淡入一行浅灰提示字，移开 250ms 淡出 ——
//! 「在不在场 / 该淡入还是淡出」由 `app::Desktop`（`composer_hover` / `composer_hint`）
//! 算好递进来，这里只照着画，不自己存状态。
//!
//! **demo2 的 1 号是规格书**：颜色、字号、尺寸、间距逐项对齐（见下面的注释），
//! 不凭感觉。对照表：外层 `8/10/10` 留白 + `#f2f2f2`；白卡圆角 12、阴影
//! `0 1px 3px 黑8%`、边框 `1px 黑5%`；写字区 `10/12/4`；文字 13.5px 行高 1.6；
//! 工具条 `2/8/8`、图标 28×28、图标本身 18px `#4d5156`；剪刀那颗 34px 宽带 ⌄；
//! 右侧声纹是**灰色图标**（不是蓝圆）、28×26、距发送键 6px；发送 26 高、圆角 4、
//! 字号 12.5。
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │  浅灰留白                                 │
//! │  ┌────────────────────────────────────┐  │
//! │  │  文本区（1~8 行自动长高，无边框）    │  │
//! │  │  （悬停淡入：Enter 发送 · Shift+…） │  │
//! │  │  🙂  ⬡  📁  ✂⌄  🎤       ≋  [发送] │  │
//! │  └────────────────────────────────────┘  │
//! └──────────────────────────────────────────┘
//! ```
//!
//! **这一层不持有状态、不碰 HTTP、不碰 Desktop 的字段**：调它的人把
//! 「能不能发」「在不在生成」「提示字在不在场 / 淡入还是淡出」和几个回调递进来。
//!
//! 工具行的图标这一轮**只有样子**：表情 / 更多 / 文件 / 截图 / 语音都还没接功能，
//! 点了没反应（没有 `on_click`）。等以后真做附件再说。

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::*;

use crate::app::scrollbar::FADE;

// 跟 demo2 的 1 号一套的颜色。
const WELL_BG: u32 = 0xf2f2f2; // 卡片四周的浅灰留白
const CARD_BG: u32 = 0xffffff; // 白卡片
const ICON_IDLE: u32 = 0x4d5156; // 工具图标（含右侧声纹）
const HINT_TEXT: u32 = 0xb8bdc4; // 悬停提示的浅灰字
const GEN_HINT: u32 = 0x8a8a8a; // 生成中的灰字提示
const SEND_BG: u32 = 0x07c160; // 发送可用
const SEND_BG_IDLE: u32 = 0xe9e9e9; // 发送不可用
const SEND_TEXT_IDLE: u32 = 0xb2b2b2;
const STOP_BG: u32 = 0xf2f2f2;
const STOP_TEXT: u32 = 0xe64340;

// demo 里 textarea / hint 都是 13.5px。行高是 Textarea 组件写死的 1.25rem = 20px，
// 不随字号变（gpui 的 rem 基准固定 16px），所以这里不能按 1.6 倍字号自己算。
const TEXT_SIZE: f32 = 13.5;
const LINE_HEIGHT: f32 = 20.0;

/// 调用方告诉输入区的事。全是**只读**快照，没有回调之外的交互。
pub struct State {
    /// 正在生成。true 时右下角是「停止」，工具行变淡。
    pub streaming: bool,
    /// 输入框里有非空白文字。false 时「发送」是灰的（视觉上按不动）。
    pub can_send: bool,
}

/// 画输入区。
///
/// `hint_shown` / `hovered` 决定那行悬停提示字：不在场就不渲染（渲染一个透明的会
/// 挡输入框的点击）；在场时按 `hovered` 决定淡入还是淡出。
/// `on_hover` / `on_send` / `on_stop` 由调用方用 `cx.listener(...)` 递进来。
pub fn render(
    input: &Entity<gpui_kit::component::input::TextareaState>,
    state: State,
    hint_shown: bool,
    hovered: bool,
    on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
    on_send: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_stop: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    // 输入区**上方**那张 `/` 技能面板。`None` = 不渲染，也不占高度。
    // 由调用方按输入框内容现算好递进来（composer 自己不读输入框、不存状态）。
    slash_panel: Option<AnyElement>,
    // 工具行最左边那颗图标。它现在点开「这条对话的设置」，有开关状态、有回调，
    // 所以不在这里硬编码了 —— 由调用方连状态一起造好递进来。
    first_tool: AnyElement,
) -> impl IntoElement {
    let dimmed = state.streaming;

    div()
        .flex()
        .flex_col()
        .w_full()
        .bg(rgb(WELL_BG))
        .relative()
        .pt(px(8.))
        .px(px(10.))
        .pb(px(10.))
        // `/` 面板是**跟着输入框走**的：占位、把卡片往上推，跟微信那种一致。
        .children(slash_panel.map(|p| div().px(px(0.)).pb(px(6.)).child(p).into_any_element()))
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .bg(rgb(CARD_BG))
                .rounded(px(12.))
                // demo 的 box-shadow: 0 1px 3px 黑 8%。
                .shadow(vec![
                    BoxShadow::new(px(0.), px(1.), hsla(0., 0., 0., 0.08)).blur_radius(px(3.)),
                ])
                // demo 的 border: 1px 黑 5%。
                .border(px(1.))
                .border_color(rgba(0x0000000D))
                // `on_hover` 在 `Stateful` 上：先给 id。
                .id("composer-card")
                .on_hover(on_hover)
                // ── 写字区 ──
                .child(
                    div()
                        .relative()
                        .px(px(12.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .child(
                            Textarea::new(input)
                                .appearance(false)
                                .bordered(false)
                                .text_size(px(TEXT_SIZE)),
                        )
                        .child(hint_layer(hint_shown, hovered, state.can_send)),
                )
                // ── 生成中的提示：回车这时不该发出去，得让人知道为什么 ──
                .when(state.streaming, |el| {
                    el.child(
                        div()
                            .px(px(12.))
                            .pb(px(2.))
                            .text_size(px(11.))
                            .text_color(rgb(GEN_HINT))
                            .child("正在生成，回车暂不发送（可以点「停止」，或者等它说完）"),
                    )
                })
                // ── 工具行 + 右下角发送 / 停止（都压在卡片内底部） ──
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .px(px(8.))
                        .pt(px(2.))
                        .pb(px(8.))
                        .child(
                            // 生成中工具图标整组变淡；声纹和发送/停止不跟着这一组。
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.))
                                .when(dimmed, |el| el.opacity(0.4))
                                .child(first_tool)
                                .child(tool_icon(IconName::Hexagon))
                                .child(tool_icon(IconName::Folder))
                                .child(tool_icon_with_caret(IconName::Scissors))
                                .child(tool_icon(IconName::Mic)),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .when(dimmed, |el| el.opacity(0.4))
                                .child(voice_button()),
                        )
                        .child(if state.streaming {
                            action_button(
                                "composer-stop",
                                "停 止",
                                STOP_BG,
                                STOP_TEXT,
                                true,
                                12.,
                                on_stop,
                            )
                            .into_any_element()
                        } else {
                            action_button(
                                "composer-send",
                                "发送",
                                if state.can_send {
                                    SEND_BG
                                } else {
                                    SEND_BG_IDLE
                                },
                                if state.can_send {
                                    0xffffff
                                } else {
                                    SEND_TEXT_IDLE
                                },
                                state.can_send,
                                14.,
                                on_send,
                            )
                            .into_any_element()
                        }),
                ),
        )
}

/// 那行悬停提示字。
///
/// `shown` = 元素在不在场（含淡出那 250ms）；`hovered` 决定淡入还是淡出；
/// 输入框里有字就不该再叠一行提示（`can_send` 一旦 true 直接撤，不用等淡出 ——
/// 文字已经盖住那个位置了）。
fn hint_layer(shown: bool, hovered: bool, can_send: bool) -> AnyElement {
    if !shown || can_send {
        return div().into_any_element();
    }
    div()
        .absolute()
        // 写字区容器 pt(10)/px(12)，而 Textarea 组件自带内边距（Size=Medium：上下 8px、
        // 左右 10px），输入文字实际在 (12+10, 10+8)。hint 要跟输入文字**精确重叠**，
        // 就把这层内边距补上；行高用组件的 20px。
        .top(px(18.))
        .left(px(22.))
        .right(px(12.))
        .text_size(px(TEXT_SIZE))
        .line_height(px(LINE_HEIGHT))
        .text_color(rgb(HINT_TEXT))
        // id 里带 in/out：动画只在 id 变化时重播，带方向才会「进来淡入、出去淡出」。
        .with_animation(
            ElementId::Name(format!("composer-hint-{}", if hovered { "in" } else { "out" }).into()),
            Animation::new(FADE),
            move |el, t| el.opacity(if hovered { t } else { 1. - t }),
        )
        .child("Enter 发送 · Shift+Enter 换行")
        .into_any_element()
}

/// 工具行里的一颗图标。**这一轮没有功能**：hover 只变鼠标指针。
///
/// 颜色和尺寸必须自己给：直接 `.child(IconName::X)` 时用的是**窗口默认文字样式**
/// （本项目里是白字），压在浅色卡片上就完全看不见 —— 第一次画就是这么踩的。
fn tool_icon(icon: IconName) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .id(ElementId::Name(format!("tool-{icon:?}").into()))
        .hover(|style| style.cursor_pointer())
        .child(Icon::new(icon).text_color(rgb(ICON_IDLE)).size(px(18.)))
}

/// 剪刀那颗：demo 里它比别的图标宽（34px），剪刀右边跟着一个小 ⌄。纯样子。
fn tool_icon_with_caret(icon: IconName) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(34.))
        .h(px(28.))
        .id(ElementId::Name(format!("tool-{icon:?}").into()))
        .hover(|style| style.cursor_pointer())
        .child(Icon::new(icon).text_color(rgb(ICON_IDLE)).size(px(18.)))
        .child(
            Icon::new(IconName::ChevronDown)
                .text_color(rgb(ICON_IDLE))
                .size(px(10.)),
        )
}

/// 右侧那颗声纹（语音输入）。跟其余图标一样**灰色、只画不接** —— demo 里它不是蓝圆。
fn voice_button() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.))
        .h(px(26.))
        .mr(px(6.))
        .id(ElementId::Name("tool-voice".into()))
        .hover(|style| style.cursor_pointer())
        .child(
            Icon::new(IconName::AudioLines)
                .text_color(rgb(ICON_IDLE))
                .size(px(18.)),
        )
}

/// 底行按钮。用 `div` 手搓而不是 `Button`：这里的尺寸/圆角/四态色值都是
/// 微信的，套组件反而要一层层覆盖样式。`pad_x` 是左右内边距（发送 14、停止 12，
/// 跟 demo 对齐）。
fn action_button(
    id: &'static str,
    label: &'static str,
    bg: u32,
    fg: u32,
    enabled: bool,
    pad_x: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.))
        .px(px(pad_x))
        .rounded(px(4.))
        .bg(rgb(bg))
        .text_size(px(12.5))
        .text_color(rgb(fg))
        .id(ElementId::Name(id.into()))
        .when(enabled, |el| el.cursor_pointer())
        .on_click(on_click)
        .child(label)
}
