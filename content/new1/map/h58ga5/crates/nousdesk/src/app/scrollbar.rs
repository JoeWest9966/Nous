//! 滚动条：**鼠标进内容区才浮出来的那根细黑条**（demo 里的 3 号，macOS 那种）。
//!
//! 这里只定「长什么样、什么时候出现」。滚动、拖拽、命中区域都是 `Scrollbar` 元素
//! 自己做的 —— 自己再画一根是重复造滑轮，而且拖起来一定比它差。
//!
//! 用法：父元素 `.relative()`（顺手也是滚动视口），滚动容器和 `overlay(..)`
//! 是同一个父元素下的兄弟。
//!
//! 三个踩过的坑：
//!
//! 1. **不要用 `.viewport_from_layout()`**。`Scrollbar` 的 `request_layout` 把自己
//!    设成 `absolute` + 100% x 100%，尺寸要等布局，而 `prepaint` 里算蒙版时就要用
//!    这个尺寸 —— 拿到 0 就整块被裁掉，表现是「一条线都不画」，看着像没生效。
//!    改成把滚动句柄的 `bounds()` 递进去（上一帧的真实像素），立刻就有了。
//! 2. 库里那个 `ScrollbarMode::Hover` 在我们这套布局里没反应（悬停状态是它自己
//!    在鼠标事件里更新的，等不到）。所以显不显示由外面决定，这里固定 `Always`。
//! 3. 元素留不留由 `shown` 决定：看不见的时候**必须**真的不渲染，因为透明的它
//!    会插一块铺满整页的 hitbox，把底下内容的悬停/点击全挡掉。

use std::time::Duration;

use gpui_kit::AnimationExt as _;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarMode};
use gpui_kit::*;

/// 淡入淡出时长 —— 和 demo 里那 0.25 秒的 css transition 对齐。
pub const FADE: Duration = Duration::from_millis(250);

/// 浮层滚动条。
///
/// `id` 必须一个面板一个：默认 id 取调用点的源码位置，而我们只有这一个调用点，
/// 几个面板会撞成同一个 —— 撞了的表现是「另一个面板的滚动条不动」。
///
/// * `shown`：元素在不在场（含淡出那 250ms）
/// * `hovered`：鼠标在不在这个面板上（决定淡入还是淡出）
pub fn overlay(id: &'static str, handle: &ScrollHandle, shown: bool, hovered: bool) -> AnyElement {
    if !shown {
        return div().into_any_element();
    }
    div()
        .absolute()
        .inset_0()
        // id 里带 in/out：动画只在 id 变化时重播，带方向才会「进来淡入、出去淡出」。
        .with_animation(
            ElementId::Name(format!("{id}-{}", if hovered { "in" } else { "out" }).into()),
            Animation::new(FADE),
            move |el, t| el.opacity(if hovered { t } else { 1. - t }),
        )
        .child(
            Scrollbar::new(handle)
                .id(id)
                .axis(ScrollbarAxis::Vertical)
                .mode(ScrollbarMode::Always)
                .styles(|s| {
                    s
                        // 没有轨道。macOS 那根就是这样：只有滑块，没有一条槽。
                        .track(|t| t.bg(hsla(0., 0., 0., 0.)))
                        .track_hover(|t| t.bg(hsla(0., 0., 0., 0.)))
                        .thumb(|t| {
                            t.bg(rgba(0x00000047)) // 黑 28%
                                .width(px(7.))
                                .inset(px(3.))
                                .radius(px(3.5))
                                // 内容再长也留 28px 抓得住，不然会话一多滑块细成一根线点不着。
                                .min_length(px(28.))
                        })
                        .thumb_hover(|t| t.bg(rgba(0x00000066))) // 黑 40%
                        .thumb_active(|t| t.bg(rgba(0x00000080))) // 黑 50%，拖的时候最深
                })
                .viewport_bounds(handle.bounds()),
        )
        .into_any_element()
}
