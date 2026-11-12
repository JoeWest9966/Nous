//! 左栏底部的「≡ 更多」菜单，和它打开的设置页（占右栏整页）。
//!
//! 菜单只放**真能点**的条目：设置 / 重新连接 / 打开配置文件夹 / 版本信息。
//! 不出「消息通知」「快捷键」「退出登录」这种假入口 —— 没实现的东西摆在那儿，
//! 用户点一次就等于被晃一次。
//!
//! 设置里只有 `host` 和 `board` 两个可编辑项。引擎地址**不**给输入框：
//! 它只能从 `__nous/resolve` 拿，写进配置就有了第二个真相。

use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use gpui_kit::component::{Icon, Sizable as _, Size};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{Desktop, WECHAT_GREEN};
use crate::config::Config;

const MUTED: u32 = 0x999999;
const FAINT: u32 = 0xaaaaaa;
const BAD: u32 = 0xd32f2f;
const GOOD: u32 = 0x079c50;
const RAIL_DARK: u32 = 0x2e2e2e;

/// 设置页左边那两项。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    Connection,
    About,
}

impl Desktop {
    // ── 菜单状态 ────────────────────────────────────────────

    pub(crate) fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu_open {
            self.menu_open = false;
            cx.notify();
        }
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.settings_open = true;
        self.settings_msg = None;
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.settings_msg = None;
        cx.notify();
    }

    // ── 菜单 ────────────────────────────────────────────────

    /// 菜单浮层：一层透明罩子（点空白收起）+ 菜单本体。
    ///
    /// 罩子必须排在菜单**之前**画，GPUI 是后者盖前者。
    pub(crate) fn render_menu_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let scrim = div()
            .id(ElementId::Name("menu-scrim".into()))
            .absolute()
            .inset_0()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_menu(cx)));

        div()
            .absolute()
            .inset_0()
            .child(scrim)
            .child(self.menu_body(cx))
            .into_any_element()
    }

    fn menu_body(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .left(px(58.))
            .bottom(px(14.))
            .w(px(266.))
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .rounded(px(9.))
            .border(px(1.))
            .border_color(rgb(0xe6e6e6))
            .shadow_lg()
            .overflow_hidden()
            .child(self.menu_header())
            .child(
                div()
                    .p(px(6.))
                    .child(group_label("应用"))
                    .child(menu_row(
                        "menu-settings",
                        IconName::Settings,
                        "设置",
                        true,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.open_settings(cx)),
                    )),
            )
            .child(
                div()
                    .p(px(6.))
                    .border_t(px(1.))
                    .border_color(rgb(0xf0f0f0))
                    .child(group_label("连接"))
                    .child(menu_row(
                        "menu-reconnect",
                        IconName::RefreshCw,
                        if self.loading { "连接中…" } else { "重新连接" },
                        !self.loading,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.connect(cx)),
                    ))
                    .child(menu_row(
                        "menu-reveal-config",
                        IconName::FolderOpen,
                        "打开配置文件夹",
                        true,
                        |_, _, _| reveal_config(),
                    )),
            )
            .child(
                div()
                    .p(px(6.))
                    .border_t(px(1.))
                    .border_color(rgb(0xf0f0f0))
                    .child(group_label("关于"))
                    .child(info_row(
                        "版本",
                        format!("v{}", env!("CARGO_PKG_VERSION")),
                    ))
                    .child(info_row(
                        "引擎",
                        self.engine_node.clone().unwrap_or_else(|| "未知".into()),
                    )),
            )
            .into_any_element()
    }

    fn menu_header(&self) -> AnyElement {
        let (text, color) = if self.loading {
            ("连接中…".to_string(), MUTED)
        } else if self.error.is_some() {
            ("连不上 · 去设置看看".to_string(), BAD)
        } else {
            (format!("已连接 · {}", self.conn.board), GOOD)
        };

        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(14.))
            .bg(rgb(0xf7f7f7))
            .border_b(px(1.))
            .border_color(rgb(0xeeeeee))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(6.))
                    .bg(rgb(RAIL_DARK))
                    .text_color(rgb(0xffffff))
                    .child("N"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child("NousDesk"))
                    .child(div().text_size(px(11.)).text_color(rgb(color)).child(text)),
            )
            .into_any_element()
    }

    // ── 设置页（右栏整页） ──────────────────────────────────

    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .bg(rgb(0xffffff))
            .child(self.settings_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.settings_nav(cx))
                    .child(self.settings_pane(cx)),
            )
            .into_any_element()
    }

    fn settings_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(48.))
            .flex_none()
            .px(px(14.))
            .bg(rgb(0xf7f7f7))
            .border_b(px(1.))
            .border_color(rgb(0xe2e2e2))
            .child(
                // 返回：回到聊天区。左栏列表一直在，这里只收起右栏这一页。
                div()
                    .id(ElementId::Name("settings-back".into()))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_settings(cx)))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(rgb(0x666666))
                    .hover(|el| el.bg(rgb(0xeeeeee)))
                    .child(Icon::new(IconName::ArrowLeft).with_size(Size::Size(px(14.))))
                    .child("返回聊天"),
            )
            .child(div().w(px(8.)))
            .child(div().text_size(px(13.)).child("设置"))
            .child(div().flex_1())
            .into_any_element()
    }

    fn settings_nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = [
            (SettingsTab::Connection, "连接"),
            (SettingsTab::About, "关于"),
        ];
        let items: Vec<AnyElement> = tabs
            .into_iter()
            .map(|(tab, label)| {
                let on = self.settings_tab == tab;
                div()
                    .id(ElementId::Name(format!("settings-tab-{label}").into()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.settings_tab = tab;
                        cx.notify();
                    }))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .when(on, |el| {
                        el.bg(rgb(0xe8f7ee))
                            .text_color(rgb(GOOD))
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!on, |el| {
                        el.text_color(rgb(0x555555))
                            .hover(|h| h.bg(rgb(0xeeeeee)))
                    })
                    .child(label)
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(170.))
            .flex_none()
            .p(px(10.))
            .bg(rgb(0xfafafa))
            .border_r(px(1.))
            .border_color(rgb(0xeeeeee))
            .children(items)
            .into_any_element()
    }

    fn settings_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let body: AnyElement = match self.settings_tab {
            SettingsTab::Connection => self.connection_pane(cx),
            SettingsTab::About => self.about_pane(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .id(ElementId::Name("settings-scroll".into()))
            .overflow_y_scroll()
            .p(px(22.))
            .child(body)
            .into_any_element()
    }

    fn connection_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let target = Config::save_target()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| e);

        let msg = self.settings_msg.as_ref().map(|(text, bad)| {
            div()
                .text_size(px(11.5))
                .text_color(rgb(if *bad { BAD } else { GOOD }))
                .child(text.clone())
                .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .max_w(px(460.))
            .child(div().text_size(px(15.)).child("连接"))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(MUTED))
                    .child("改完保存会写到下面那个文件，然后重新 resolve 一次。"),
            )
            .child(field("宿主地址 host", Input::new(&self.host_input)))
            .child(field("Board", Input::new(&self.board_input)))
            .child(readonly(
                "引擎地址（只读）",
                self.engine_url
                    .clone()
                    .unwrap_or_else(|| "未连接 —— 先 resolve 到才会显示".into()),
                "由 <host>/p/<board>/<self>/__nous/resolve/<engine> 现拿，不写进配置。",
            ))
            .child(readonly(
                "配置文件（保存写这里）",
                target,
                "查找顺序：NOUS_DESKTOP_CONFIG → exe 旁 desktop.toml → %APPDATA%\\nousdesk\\",
            ))
            .children(msg)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(pill(
                        "settings-save",
                        "保存并重连",
                        true,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.save_settings(cx)),
                    ))
                    .child(pill(
                        "settings-reconnect",
                        if self.loading { "连接中…" } else { "重新连接" },
                        false,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.connect(cx)),
                    )),
            )
            .into_any_element()
    }

    fn about_pane(&self) -> AnyElement {
        let log_path = dirs::data_local_dir()
            .map(|d| d.join(crate::config::APP_DIR).join("desktop.log").display().to_string())
            .unwrap_or_else(|| "（拿不到日志目录）".into());
        let cfg_path = Config::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "（没有配置文件，用的是默认值）".to_string());

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .max_w(px(520.))
            .child(div().text_size(px(15.)).child("关于"))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(MUTED))
                    .child("引擎版本来自 GET /v1/device 的 node —— 出问题先对版本。"),
            )
            .child(kv("客户端", format!("v{}", env!("CARGO_PKG_VERSION"))))
            .child(kv(
                "引擎",
                self.engine_node.clone().unwrap_or_else(|| "未知".into()),
            ))
            .child(kv(
                "引擎地址",
                self.engine_url.clone().unwrap_or_else(|| "未连接".into()),
            ))
            .child(kv("配置文件", cfg_path))
            .child(kv("日志", log_path))
            .into_any_element()
    }

    // ── 保存 ────────────────────────────────────────────────

    /// 把输入框里的两个值写回配置文件，然后重连。
    ///
    /// 校验只挡「一定错」的：空值、host 不带协议、board 带斜杠。剩下的交给 resolve 报错 ——
    /// 客户端猜地址对不对就等于有了第二个真相。
    pub(crate) fn save_settings(&mut self, cx: &mut Context<Self>) {
        let host = self.host_input.read(cx).value().trim().to_string();
        let board = self.board_input.read(cx).value().trim().to_string();

        let bad = |msg: String| Some((msg, true));
        let problem = if host.is_empty() {
            bad("host 不能为空".into())
        } else if !(host.starts_with("http://") || host.starts_with("https://")) {
            bad("host 要以 http:// 或 https:// 开头".into())
        } else if board.is_empty() {
            bad("board 不能为空".into())
        } else if board.contains('/') {
            bad("board 里不能有 /".into())
        } else {
            None
        };
        if let Some(msg) = problem {
            self.settings_msg = Some(msg);
            cx.notify();
            return;
        }

        let next = Config { host, board };
        match next.save() {
            Ok(path) => {
                self.conn = next;
                self.settings_msg = Some((format!("已写入 {}", path.display()), false));
                self.connect(cx);
            }
            Err(e) => {
                self.settings_msg = Some((e, true));
                cx.notify();
            }
        }
    }
}

// ── 小件 ────────────────────────────────────────────────────

/// 设置页的按钮。不用 `component::button::Button` —— 它的 variant 会把自定义底色
/// 压成淡色（实测 `#07c160` 出来是 20% 透明的一层淡绿），而这个项目的皮本来
/// 就是 div 手搭的，颜色自己拿着更省事。
fn pill(
    id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (bg, fg, hover) = if primary {
        (WECHAT_GREEN, 0xffffff, 0x06ad56)
    } else {
        (0xf0f0f0, 0x444444, 0xe4e4e4)
    };
    div()
        .id(ElementId::Name(id.into()))
        .on_click(handler)
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.))
        .px(px(14.))
        .rounded(px(5.))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_size(px(12.))
        .hover(move |el| el.bg(rgb(hover)))
        .child(label.into())
        .into_any_element()
}

fn group_label(text: &'static str) -> AnyElement {
    div()
        .px(px(10.))
        .pt(px(4.))
        .pb(px(5.))
        .text_size(px(10.))
        .text_color(rgb(FAINT))
        .child(text)
        .into_any_element()
}

fn menu_row(
    id: &'static str,
    icon: IconName,
    label: impl Into<SharedString>,
    enabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .when(enabled, |el| el.on_click(handler))
        .when(enabled, |el| el.hover(|h| h.bg(rgb(0xf2f2f2))))
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(32.))
        .px(px(10.))
        .rounded(px(6.))
        .when(!enabled, |el| el.opacity(0.45))
        .child(
            Icon::new(icon)
                .with_size(Size::Size(px(15.)))
                .text_color(rgb(0x777777)),
        )
        .child(div().text_size(px(12.5)).child(label.into()))
        .into_any_element()
}

fn info_row(label: &'static str, value: String) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(30.))
        .px(px(10.))
        .text_size(px(12.))
        .child(div().text_color(rgb(0x666666)).child(label))
        .child(div().flex_1())
        .child(div().text_size(px(11.5)).text_color(rgb(MUTED)).child(value))
        .into_any_element()
}

fn field(label: &'static str, input: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .w_full()
        .child(div().text_size(px(11.5)).text_color(rgb(0x777777)).child(label))
        .child(div().w_full().child(input))
        .into_any_element()
}

fn readonly(label: &'static str, value: String, note: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(div().text_size(px(11.5)).text_color(rgb(0x777777)).child(label))
        .child(
            div()
                .p(px(8.))
                .rounded(px(5.))
                .bg(rgb(0xf6f6f6))
                .border(px(1.))
                .border_color(rgb(0xdddddd))
                .text_size(px(11.))
                .text_color(rgb(0x666666))
                .child(value),
        )
        .child(div().text_size(px(10.5)).text_color(rgb(FAINT)).child(note))
        .into_any_element()
}

fn kv(label: &'static str, value: String) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .pb(px(8.))
        .border_b(px(1.))
        .border_color(rgb(0xf0f0f0))
        .child(div().text_size(px(11.5)).text_color(rgb(0x777777)).child(label))
        .child(div().text_size(px(12.)).child(value))
        .into_any_element()
}

/// 在文件管理器里打开配置文件所在目录。Windows 只发便携包，`explorer` 够用；
/// explorer 的退出码不可靠（成功也常常非 0），一律不看结果。
fn reveal_config() {
    let Ok(path) = Config::save_target() else { return };
    let Some(dir) = path.parent().map(|d| d.to_path_buf()) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
}
