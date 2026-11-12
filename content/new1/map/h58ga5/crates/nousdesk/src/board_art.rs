//! 「New Manual」盘面的美术件：颜色、尺寸、那张图。
//!
//! 外观照抄 qrd3wh 的面板页（`/p/<board>/hb7k3j/`，Pele / Flexoki dark）。
//! 这里只放**能被抄错**的东西：色值、尺寸换算、Cynefin 那几条路径。
//!
//! 两条纪律：
//! - 颜色只用下面这些常量，别在渲染代码里现编色值 —— 那个页面整套色是一张调色板，
//!   漏一个就会「看着像但不是」。
//! - 尺寸一律先换算成**右格的 cqmin** 再落到 px：盘面现在只占聊天区那一格，
//!   不再按整个窗口的 vmin 算 —— 否则在 280px 列表 + 60px 图标条旁边会挤成一团。

use gpui_kit::*;

// ── Flexoki dark（和页面 :root 一一对应）──────────────────────────────
pub(crate) const BG: u32 = 0x100f0f;
pub(crate) const PANEL: u32 = 0x1c1b1a;
pub(crate) const SURFACE: u32 = 0x282726;
pub(crate) const SURFACE_ALT: u32 = 0x343331;
pub(crate) const BORDER: u32 = 0x403e3c;
/// cliff 那条实线，也是正文色。
pub(crate) const LINE: u32 = 0xcecdc3;
pub(crate) const FG: u32 = 0xcecdc3;
pub(crate) const MUTED: u32 = 0x878580;
pub(crate) const FAINT: u32 = 0x575653;
pub(crate) const ACCENT: u32 = 0x4385be;
pub(crate) const S1: u32 = 0xd14d41;
pub(crate) const S2: u32 = 0xda702c;
pub(crate) const S3: u32 = 0xd0a215;
pub(crate) const S4: u32 = 0x879a39;
pub(crate) const S5: u32 = 0x3aa99f;
/// 象限色是 14% 叠在 panel 上的，不是实色。
pub(crate) const QUADRANT_ALPHA: f32 = 0.14;
/// 中央椭圆的底色（青，18%）。
pub(crate) const HUB_ALPHA: f32 = 0.18;

/// 等宽字。Windows 上 `Consolas` 一定有；`Cascadia Code` 不一定装了，别写在前面。
pub(crate) const MONO: &str = "Consolas";

/// 尺寸换算。**基准是盘面（右格）自己的短边**，等价于 CSS 的 `1cqmin`。
/// 以前这里吃 `window.viewport_size()`（整窗 vmin）；盘面塞进右格之后必须换成
/// 右格实测 bounds，不然字和椭圆按整窗算、在窄右栏里会挤爆。
#[derive(Clone, Copy)]
pub(crate) struct M {
    pub cqmin: f32,
    /// 右格宽高。那个页面里 `32%` 这种是**相对盘面**的，不是相对 cqmin ——
    /// 中央椭圆就吃这个，用 cqmin 算会变成一个圆。
    pub w: f32,
    pub h: f32,
}

/// 盘面所在的那一格 = 窗口减去图标条和左栏。
/// 左栏宽度按当前 tab 算（聊天一栏 / 通讯录两栏），**常量直接引用 app.rs 的**，
/// 改宽度时不用再来同步这份表。
pub(crate) fn pane_width(win_w: f32, tab: crate::app::Tab) -> f32 {
    use crate::app::{ICON_BAR_W, PRESET_TREE_W, SESSION_LIST_W, TEMPLATE_LIST_W, Tab};
    let left = match tab {
        Tab::Chats => ICON_BAR_W + SESSION_LIST_W,
        Tab::Templates => ICON_BAR_W + PRESET_TREE_W + TEMPLATE_LIST_W,
    };
    (win_w - left).max(1.)
}

impl M {
    /// `size` = 整个窗口的 viewport。盘面打开时它**顶替** `render_chat`
    /// （连那条 48px 的会话标题栏一起换掉），所以右格高度就是整窗高度。
    pub fn new(size: Size<Pixels>, tab: crate::app::Tab) -> Self {
        let w = pane_width(f32::from(size.width), tab);
        let h = f32::from(size.height).max(1.);
        Self {
            cqmin: w.min(h).max(1.),
            w,
            h,
        }
    }

    /// `12cqmin` 这种写法：单位是容器短边的 1%。
    pub fn v(&self, cqmin: f32) -> Pixels {
        px(cqmin * self.cqmin / 100.)
    }

    /// `clamp(lo, kcqmin, hi)`。那个页面所有字号都长这样。
    pub fn f(&self, cqmin: f32, lo: f32, hi: f32) -> Pixels {
        px((cqmin * self.cqmin / 100.).clamp(lo, hi))
    }
}

// ── 那张图（原样抄自 hb7k3j 的 <svg class="frame">）─────────────────────
//
// viewBox 是 `0 0 640 440`，且 `preserveAspectRatio="none"` —— 也就是**直接拉伸**：
// x 乘 w/640，y 乘 h/440，不做等比缩放。所以这里的四块色和三条虚线在窄窗口里会变形，
// 那是原设计本来的行为，不是 bug。

pub(crate) const VB_W: f32 = 640.;
pub(crate) const VB_H: f32 = 440.;

/// 左上：Model
const D_R1: &str = "M0,0L314.04,0C315.06,27.5,322.13,27.5,323.15,55C315.14,82.5,335.41,82.5,327.39,110C328.99,137.5,322.51,137.5,324.11,165C319.92,192.5,324.19,192.5,320,220C280,225.07,280,210.71,240,215.78C200,204.86,200,227.13,160,216.22C120,223.83,120,218.6,80,226.21C40,222.27,40,221.52,0,217.58Z";
/// 右上：Context
const D_R2: &str = "M314.04,0L640,0L640,222.23C600,231.87,600,212.95,560,222.59C520,213.69,520,227.54,480,218.63C440,224.66,440,213.77,400,219.8C360,218.1,360,221.69,320,220C324.19,192.5,319.92,192.5,324.11,165C322.51,137.5,328.99,137.5,327.39,110C335.41,82.5,315.14,82.5,323.15,55C322.13,27.5,315.06,27.5,314.04,0Z";
/// 左下：Extensions
const D_R4: &str = "M0,217.58C40,221.52,40,222.27,80,226.21C120,218.6,120,223.83,160,216.22C200,227.13,200,204.86,240,215.78C280,210.71,280,225.07,320,220C339.2,264,291.2,341,329.6,385C300.8,407,325.76,429,320,440L0,440Z";
/// 右下：Permissions
const D_R3: &str = "M320,220C360,221.69,360,218.1,400,219.8C440,213.77,440,224.66,480,218.63C520,227.54,520,213.69,560,222.59C600,212.95,600,231.87,640,222.23L640,440L320,440C325.76,429,300.8,407,329.6,385C291.2,341,339.2,264,320,220Z";

/// 中间那条竖虚线（上）
const D_B1: &str = "M314.04,0C315.06,27.5,322.13,27.5,323.15,55C315.14,82.5,335.41,82.5,327.39,110C328.99,137.5,322.51,137.5,324.11,165C319.92,192.5,324.19,192.5,320,220";
/// 中间那条横虚线（左）
const D_B2: &str = "M0,217.58C40,221.52,40,222.27,80,226.21C120,218.6,120,223.83,160,216.22C200,227.13,200,204.86,240,215.78C280,210.71,280,225.07,320,220";
/// 中间那条横虚线（右）
const D_B3: &str = "M320,220C360,221.69,360,218.1,400,219.8C440,213.77,440,224.66,480,218.63C520,227.54,520,213.69,560,222.59C600,212.95,600,231.87,640,222.23";
/// 那条崖（2px 实线）：图上唯一一条「跨不过去」的线。
const D_CLIFF: &str = "M320,220C339.2,264,291.2,341,329.6,385C300.8,407,325.76,429,320,440";

/// 一条要画的东西：路径 + 颜色。
pub(crate) struct Shape {
    pub path: Path<Pixels>,
    pub color: Hsla,
}

/// 把那张图按当前尺寸摊成一串 `Shape`。
///
/// 画在 `canvas` 的 paint 里（`window.paint_path`）—— 那个页面的图本来就是矢量，
/// 用 `svg()` 元素反而画不了：它只走 alpha 蒙版，一个颜色，四块色会糊成一块。
pub(crate) fn shapes(bounds: Bounds<Pixels>) -> Vec<Shape> {
    let sx = f32::from(bounds.size.width) / VB_W;
    let sy = f32::from(bounds.size.height) / VB_H;
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let mut out = Vec::with_capacity(8);

    for (d, color) in [(D_R1, S1), (D_R2, S2), (D_R3, S3), (D_R4, S4)] {
        if let Some(path) = parse(d, sx, sy, ox, oy, PathBuilder::fill()) {
            out.push(Shape {
                path,
                color: rgb(color).opacity(QUADRANT_ALPHA).into(),
            });
        }
    }
    for d in [D_B1, D_B2, D_B3] {
        // 虚线是 6 3、1px，`vector-effect: non-scaling-stroke` 等价于「线宽不跟着拉伸」。
        let b = PathBuilder::stroke(px(1.)).dash_array(&[px(6.), px(3.)]);
        if let Some(path) = parse(d, sx, sy, ox, oy, b) {
            out.push(Shape {
                path,
                color: rgb(BORDER).into(),
            });
        }
    }
    if let Some(path) = parse(D_CLIFF, sx, sy, ox, oy, PathBuilder::stroke(px(2.))) {
        out.push(Shape {
            path,
            color: rgb(LINE).into(),
        });
    }
    out
}

/// 把一条 SVG `d`（只认绝对 `M/L/C/Z`）摊成 `PathBuilder` 的调用。
///
/// 那个页面的路径全是 `M x,y` / `L x,y` / `C c1x,c1y c2x,c2y x,y` / `Z`，
/// 没有相对坐标、没有弧、没有简写 —— 所以这里不引 SVG 解析库，手写一个够用的。
fn parse(d: &str, sx: f32, sy: f32, ox: f32, oy: f32, style: PathBuilder) -> Option<Path<Pixels>> {
    // 按命令字母切段，每段里按 `,` / 空白切数字。
    let mut segments: Vec<(char, Vec<f64>)> = Vec::new();
    let mut cur_cmd = 'M';
    let mut cur = String::new();
    let flush = |cmd: char, s: &str, out: &mut Vec<(char, Vec<f64>)>| {
        let nums: Vec<f64> = s
            .split([',', ' '])
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse::<f64>().ok())
            .collect();
        if !nums.is_empty() {
            out.push((cmd, nums));
        }
    };
    for ch in d.chars() {
        if ch.is_ascii_alphabetic() {
            flush(cur_cmd, &cur, &mut segments);
            cur.clear();
            cur_cmd = ch;
        } else {
            cur.push(ch);
        }
    }
    flush(cur_cmd, &cur, &mut segments);

    let (sx, sy) = (sx as f64, sy as f64);
    let (ox, oy) = (ox as f64, oy as f64);
    let mut b = style;
    for (cmd, mut nums) in segments {
        let p = |n: &mut Vec<f64>| -> Point<Pixels> {
            let x = if n.is_empty() { 0. } else { n.remove(0) };
            let y = if n.is_empty() { 0. } else { n.remove(0) };
            point(px((x * sx + ox) as f32), px((y * sy + oy) as f32))
        };
        match cmd {
            // 一条 `d` 里可能有多个坐标对（`L 1,2 3,4` 等价于两次 line_to），逐个吃完。
            'M' => {
                while nums.len() >= 2 {
                    b.move_to(p(&mut nums));
                }
            }
            'L' => {
                while nums.len() >= 2 {
                    b.line_to(p(&mut nums));
                }
            }
            'C' => {
                while nums.len() >= 6 {
                    let c1 = p(&mut nums);
                    let c2 = p(&mut nums);
                    let to = p(&mut nums);
                    b.cubic_bezier_to(to, c1, c2);
                }
            }
            'Z' => b.close(),
            _ => {}
        }
    }
    b.build().ok()
}
