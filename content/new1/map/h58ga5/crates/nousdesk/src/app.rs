//! 应用主体。三栏：图标条 / 会话列表 / 聊天区。
//!
//! 微信桌面版的骨头。聊天区的流式（SSE）还没接，先做历史气泡。

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{Icon, Sizable as _, Size};
// 完整的 lucide 目录在 assets 里（component 的 IconName 只是一小撮兼容子集）。
use gpui_kit::assets::IconName;
// 可选中文本。`div().child(String)` 是死的 StyledText —— 鼠标拖不动，也就复制不出来。
// 用 gpui-base 那个原始 TextView，不是 gpui-component 的兼容包装：
// 包装层根本没开放 `code_block_highlighter`（它把高亮挂在自己的主题机制上，
// 而那个机制只在 tree-sitter 那条分支里才有），拿不到就没法给代码块上色。
use gpui_kit::base::text::TextView;
use gpui_kit::prelude::FluentBuilder as _;
// `on_hover` / `on_click` 这些不是 Div 自带的方法，是这条 trait 给的，得显式拿进来。
use gpui_kit::prelude::StatefulInteractiveElement as _;
use gpui_kit::*;
use nous_engine::EngineClient;
use nous_engine::pins::PinClient;
use nous_engine::resolve::resolve_engine;
use nous_engine::types::{ConfigPatch, Entry, Session, Unread};
use smol::Timer;

use crate::nav_ui::SessionNav;

pub(crate) mod scrollbar;

/// 哪块面板的滚动条。加新面板就往这儿加一个分支。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bar {
    Chat,
    List,
    Tree,
    Presets,
}

use crate::config::{Config, ENGINE_BLOCK, MOBILE_BLOCK, SELF_BLOCK};
use crate::live::Live;
use crate::md::{chat_md, code_copy_button, code_highlighter, md_style, strip_harness};

// 微信桌面版的几个固定色。
const SIDEBAR_DARK: u32 = 0x2e2e2e;
pub(crate) const LIST_BG: u32 = 0xf7f7f7;
const CHAT_BG: u32 = 0xededed;
const BUBBLE_ME: u32 = 0x95ec69;
const BUBBLE_OTHER: u32 = 0xffffff;
pub(crate) const MUTED: u32 = 0x999999;
/// 微信的绿：左栏选中图标就是这个颜色。
pub(crate) const WECHAT_GREEN: u32 = 0x07c160;

// 三栏的固定宽度。**右格宽度靠它们算出来**（`board_art::pane_width`），
// 所以改这里就必须同步那份表 —— 不然 New Manual 盘面的 cqmin 会算错。
pub(crate) const ICON_BAR_W: f32 = 60.;
pub(crate) const SESSION_LIST_W: f32 = 280.;
pub(crate) const PRESET_TREE_W: f32 = 224.;
pub(crate) const TEMPLATE_LIST_W: f32 = 340.;

actions!(nousdesk, [TogglePinAction]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Chats,
    Templates,
}

pub struct Desktop {
    pub(crate) engine: Option<Arc<EngineClient>>,
    pub(crate) sessions: Vec<Session>,
    pub(crate) selected: Option<i64>,
    entries: Vec<Entry>,
    entries_loading: bool,
    entries_error: Option<String>,
    pub(crate) error: Option<String>,
    /// **这一轮生成失败**（模型 400、task_failed、没答上来）：画在聊天区底部，
    /// 带「重试」。跟 `error`（连引擎失败、带「改哪个配置文件」提示）不是一条通道 ——
    /// 把「模型订阅过期」说成「连不上 + 去改配置」会把用户带到沟里（真踩过）。
    turn_error: Option<String>,
    pub(crate) loading: bool,
    input: Entity<TextareaState>,
    /// 正在生成的那条助手消息（流式）。
    live: Option<String>,
    /// 共享给 tokio 任务的流式状态。
    live_state: Live,
    /// 有没有一条流正在跑，防止重复点火。
    streaming: bool,
    /// 聊天区的滚动位置：新消息进来要跟着到底。
    scroll: ScrollHandle,
    /// 左栏会话列表的滚动句柄（列表以前是 `overflow_hidden`，压根不能滚）。
    list_scroll: ScrollHandle,
    /// 模板页（通讯录）列表的滚动句柄。
    pub(crate) presets_scroll: ScrollHandle,
    /// 通讯录类别树的滚动句柄。
    pub(crate) tree_scroll: ScrollHandle,
    /// 每个面板两位：鼠标在不在上面 / 滚动条在不在场（淡出那 250ms 里也得在场）。
    chat_hover: bool,
    chat_bar: bool,
    list_hover: bool,
    list_bar: bool,
    pub(crate) presets_hover: bool,
    pub(crate) presets_bar: bool,
    pub(crate) tree_hover: bool,
    pub(crate) tree_bar: bool,
    /// 输入区悬停提示两位：鼠标在不在输入区上 / 提示字在不在场（淡出那 250ms 里也得在场）。
    composer_hover: bool,
    composer_hint: bool,
    /// 左栏的搜索框。**单行**的 `InputState`（输入区那个是多行 `TextareaState`）。
    /// 空着 = 不过滤，列表跟没有搜索这回事一样。
    search: Entity<InputState>,
    /// 输入区 `/` 技能命令的纯 UI 状态（光标注标 / Esc / 在途 / 写失败）。
    slash: crate::slash::Slash,
    /// `GET /v1/skills` 的结果。`None` = 还没拿到 / 拿失败；面板此时给一行提示。
    skills: Option<nous_engine::types::Skills>,
    /// 面板当前过滤出的技能快照。每次 `render` 现算，键盘 ↵ 和点击都认它。
    slash_items: Vec<nous_engine::types::Skill>,
    /// `GET /v1/skills` 在不在路上。一次只发一笔，别在 render 里反复点火。
    skills_loading: bool,
    /// 清单拉失败的一句话。有它就不重试（否则「失败 → notify → 再拉」是个死循环）。
    skills_error: Option<String>,
    /// 输入区左下角那颗 DNA 图标点开的「这条对话的设置」。
    /// 这里只放界面状态（看哪页 / 清单 / 在途）；配置值本身在 `sessions` 里，那才是真相。
    cfg: crate::config_panel::Panel,
    /// 左栏当前页：聊天 / 通讯录（模板）。
    pub(crate) tab: Tab,
    /// 底部「≡ 更多」菜单开着没有。
    pub(crate) menu_open: bool,
    /// 右栏是不是在显示全局设置（整页替换聊天区，左栏不动）。
    pub(crate) settings_open: bool,
    /// 设置页左边选的那一项。
    pub(crate) settings_tab: crate::settings::SettingsTab,
    /// 设置页底部的一行反馈：文案 + 是不是错。
    pub(crate) settings_msg: Option<(String, bool)>,
    /// 启动时读到的配置（保存后跟着换）。
    ///
    /// 不叫 `cfg` —— 那个名字已经被 `config_panel` 的界面状态占了。
    pub(crate) conn: Config,
    pub(crate) host_input: Entity<InputState>,
    pub(crate) board_input: Entity<InputState>,
    /// resolve 拿到的引擎地址 —— 只用来显示，不存盘。
    pub(crate) engine_url: Option<String>,
    /// 引擎版本（`GET /v1/device` 的 `node`）。
    pub(crate) engine_node: Option<String>,
    /// 窗口在不在前台。轮询只在聚焦时跑，后台一律不动。
    focused: bool,
    /// 当前常连着 SSE 的会话。`None` = 没连。切会话就换一个，不叠加。
    stream_for: Option<i64>,
    /// 展开了的工具卡片，按 entry id 记。**纯内存**：它不是事实，是「我眼下想看哪块」。
    pub(crate) open_tools: std::collections::HashSet<String>,
    /// 正在生成的这一轮里调过的工具。只有实时流里有名字（历史里的 `pi.tool-result`
    /// 只存输出，见 `坑.md`）。
    pub(crate) live_tools: Vec<crate::live::ToolRun>,
    /// 每个会话的未读条数和水位。引擎按 EntryId 数好的，这边只画。
    pub(crate) unread: Unread,
    /// 左栏是不是「分支视图」。默认 false = 全部主线会话。
    pub(crate) branches: bool,
    /// 分支视图的根。进视图时锁住：点分支里的会话只换右侧，列表不跟着塌。
    pub(crate) branch_of: Option<i64>,
    /// 分支视图里展开着的会话。纯内存，重启重置（它不是事实，是「我眼下看到哪」）。
    pub(crate) expanded: std::collections::HashSet<i64>,
    /// 这一轮是**用户自己按的停止**。abort 会让流断开，那不是错误，别弹红条。
    user_stopped: bool,
    /// 手机端那块（bjes6b）的客户端：**只用来读写置顶**。
    /// `None` = 那个块没部署 / 连不上 —— 这时不显示置顶功能，绝不弹错。
    pins_client: Option<Arc<PinClient>>,
    /// 置顶的会话 id，**顺序就是显示顺序**（来自 `pins.json`）。
    pins: Vec<i64>,
    /// 通讯录的两个段（同一个私仓，两套词表、两套条目）。
    /// 索引 0 = 会话模板（`Session-Template`），1 = 终极模板（`Ultimate-Template`）。
    pub(crate) sections: [crate::templates::Section; 2],
    /// 当前选中的段。
    pub(crate) section: usize,
    /// 通讯录里选中的类别路径。`None` = 全部；`Some("∅")` = 未分类（不是词表里的类别）。
    pub(crate) preset_cat: Option<String>,
    /// 折叠着的类别路径。**纯内存**：它不是事实，是「我眼下展开到哪」。
    pub(crate) preset_closed: std::collections::HashSet<String>,
    /// 正在从模板开的那条（防连点 + 状态提示）。
    pub(crate) creating: Option<String>,
    _subs: Vec<Subscription>,
    /// 「New Manual」盘面。只占右侧聊天区那一格（外形照 qrd3wh 的 hb7k3j 页）。
    pub(crate) newchat: crate::board::NewChat,
    /// 会话内跳转：按天分组的目录 + 9 号方案的历史抽屉。纯内存的「我眼下看到哪」。
    pub(crate) nav: SessionNav,
}

impl Desktop {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 多行：3~8 行自动长高（最少 3 行：空框也占 3 行高，跟微信的写字区一样），
        // 超过就在框里滚。回车发送、Shift+回车换行。
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                // 回车=发送，不在这里插换行。少了这句，多行框里的回车会**先插一个换行**
                // 再发出去：文本被清空时看不出来，但 send() 提前返回时会留一个空行。
                .submit_on_enter(true)
            // 不设 placeholder：空框就该是空白。悬停的提示字由 composer 的 hint 层
            // 负责（Enter 发送 · Shift+Enter 换行），别在这里再叠一行永远挂着的灰字。
        });
        // 左栏的搜索框：单行，带搜索图标 + 一键清空。
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索会话"));

        // 配置在这里同步读掉：设置页要立刻显示 host / board，不能等 tokio。
        // 读坏了也先拿默认值把窗口支起来，错误交给 connect() 报。
        let conn = Config::load().unwrap_or_default();
        let host_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://127.0.0.1:4790")
                .default_value(conn.host.clone())
        });
        let board_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("new1")
                .default_value(conn.board.clone())
        });

        // 调试用：启动就直接打开某个浮层，省得每次手点。
        // `NOUS_DESKTOP_OPEN=menu` 或 `=settings`，别的值（包括不设）就是正常启动。
        let open_at_start = std::env::var("NOUS_DESKTOP_OPEN").unwrap_or_default();
        // `/` 面板的 ↑↓ / ↵ / Esc 要在**输入框组件之前**拿到。
        // 用 interceptor（`observe_keystrokes` 太晚，`on_key_down` 挂在祖先节点上也会被
        // input 的 key binding 抢先消费）：在它里面 `cx.stop_propagation()` 就同时挡掉了
        // 组件的 Enter action（否则回车会顺手把 `/deep` 当消息发出去）。
        // 只有输入框聚焦 + 面板在场时才拦，别把全窗口的方向键都吃掉。
        let weak = cx.weak_entity();
        let subs = vec![
            cx.observe_window_activation(window, |this, window, _cx| {
                // 在后台就别每 3 秒拉一次。
                this.focused = window.is_window_active();
            }),
            cx.subscribe_in(&input, window, {
                move |this, _, event: &InputEvent, window, cx| {
                    match event {
                        // 内容变了要重画，否则「发送」按钮永远是灰的。
                        // `/` 面板的高亮/收起也跟着重置（只认输入框当前内容这一份真相）。
                        InputEvent::Change => {
                            this.slash.reset();
                            cx.notify();
                        }
                        InputEvent::PressEnter { shift, .. } => {
                            if !*shift {
                                this.send(window, cx);
                            }
                        }
                        _ => {}
                    }
                }
            }),
            cx.subscribe_in(&search, window, {
                move |_, _, event: &InputEvent, _, cx| {
                    // 每敲一个字都要重排列表（纯本地过滤，不请求引擎）。
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }
            }),
            cx.intercept_keystrokes(move |ev, window, app| {
                let key = ev.keystroke.key.as_str();
                if !matches!(key, "up" | "down" | "enter" | "escape") {
                    return;
                }
                // 修饰键组合（Ctrl+Enter 之类）不归面板管。
                if ev.keystroke.modifiers.control
                    || ev.keystroke.modifiers.alt
                    || ev.keystroke.modifiers.platform
                {
                    return;
                }
                // 配置面板开着时 Esc 收起。它没有 ↑↓ 这类键，只看 Esc，
                // 也不要求输入框聚焦 —— 面板自己的清单才是焦点可能待着的地方。
                if key == "escape" {
                    let closed = weak
                        .update(app, |this, cx| {
                            if !this.cfg_visible() {
                                return false;
                            }
                            this.cfg.open = false;
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if closed {
                        app.stop_propagation();
                        return;
                    }
                }
                let handled = weak
                    .update(app, |this, cx| {
                        if !this.slash_visible(cx) || this.slash.busy {
                            return false;
                        }
                        if !this.input.read(cx).focus_handle(cx).is_focused(window) {
                            return false;
                        }
                        match key {
                            "up" | "down" => {
                                let delta = if key == "up" { -1 } else { 1 };
                                let len = this.slash_items.len();
                                this.slash.move_by(delta, len);
                            }
                            "escape" => this.slash.dismissed = true,
                            "enter" => {
                                let ix = this.slash.cursor;
                                this.apply_skill(ix, window, cx);
                            }
                            _ => unreachable!(),
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                // 拦下的键不许再走到 input 组件：尤其 Enter，不然会把 `/deep`
                // 当成一条用户消息发出去。
                if handled {
                    app.stop_propagation();
                }
            }),
        ];

        let mut this = Self {
            engine: None,
            sessions: vec![],
            selected: None,
            entries: vec![],
            entries_loading: false,
            entries_error: None,
            error: None,
            turn_error: None,
            loading: true,
            input,
            menu_open: open_at_start == "menu",
            settings_open: open_at_start == "settings",
            settings_tab: crate::settings::SettingsTab::Connection,
            settings_msg: None,
            conn,
            host_input,
            board_input,
            engine_url: None,
            engine_node: None,
            live: None,
            live_state: Live::new(),
            streaming: false,
            scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            presets_scroll: ScrollHandle::new(),
            tree_scroll: ScrollHandle::new(),
            chat_hover: false,
            chat_bar: false,
            list_hover: false,
            list_bar: false,
            presets_hover: false,
            presets_bar: false,
            tree_hover: false,
            tree_bar: false,
            composer_hover: false,
            composer_hint: false,
            search,
            slash: crate::slash::Slash::default(),
            skills: None,
            slash_items: vec![],
            skills_loading: false,
            skills_error: None,
            cfg: crate::config_panel::Panel::default(),
            tab: Tab::Chats,
            focused: true,
            stream_for: None,
            open_tools: std::collections::HashSet::new(),
            live_tools: vec![],
            unread: Unread::default(),
            branches: false,
            branch_of: None,
            expanded: std::collections::HashSet::new(),
            user_stopped: false,
            pins_client: None,
            pins: vec![],
            sections: [
                crate::templates::Section::new(0),
                crate::templates::Section::new(1),
            ],
            section: 0,
            preset_cat: None,
            preset_closed: std::collections::HashSet::new(),
            creating: None,
            newchat: crate::board::NewChat::new(window, cx),
            nav: SessionNav::new(),
            _subs: subs,
        };
        this.bootstrap(cx);
        this
    }

    /// 启动：起轮询 + 连一次。
    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        // 轮询是个死循环，只能起一次 —— 所以它留在这儿，不在 connect 里。
        self.start_poll(cx);
        self.connect(cx);
    }

    /// 连引擎。启动走一次；「重新连接」和设置页保存后也走这里。
    ///
    /// 只做「连上 + 重拉数据」，不碰轮询（见上）。
    pub(crate) fn connect(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.engine_url = None;
        cx.notify();

        let cfg = self.conn.clone();
        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let outcome = match handle {
                Some(h) => match h.spawn(load_engine(cfg)).await {
                    Ok(Ok(v)) => Ok(v),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(format!("任务挂了：{e}")),
                },
                None => Err("tokio 没起".to_string()),
            };

            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok((engine, sessions, node, url)) => {
                        this.engine = Some(engine);
                        this.sessions = sessions;
                        this.engine_node = node;
                        this.engine_url = Some(url);
                        // 换了盘之后，原来选中的那条可能不在新引擎上 —— 别留个空壳。
                        if this
                            .selected
                            .is_some_and(|id| !this.sessions.iter().any(|s| s.id == id))
                        {
                            this.selected = None;
                            this.entries.clear();
                            this.entries_error = None;
                        }
                        // 引擎连上之前如果点过通讯录，「还没连上引擎」那句得自己消掉。
                        this.load_all_sections(false, cx);
                        // 置顶单独去 bjes6b 拿（单源在它那儿）。
                        this.load_pins(cx);
                        // `/` 面板的技能清单：失败就认了（面板给提示）。
                        this.load_skills(cx);
                        // 调试用：`NOUS_OPEN_BOARD=1` 一启动就开在 New Manual 盘面上，
                        // 省得每次手点。只是开关，不是功能。
                        if std::env::var("NOUS_OPEN_BOARD").is_ok() && !this.newchat.open {
                            log::info!("NOUS_OPEN_BOARD：直接开在 New Manual 盘面上");
                            this.newchat.open = true;
                            this.board_load(cx);
                            // 再给一个梯子：直接开在某个弹层上，省得每次手点。
                            match std::env::var("NOUS_OPEN_SHEET").as_deref() {
                                Ok("model") => {
                                    this.newchat.sheet = Some(crate::board::Sheet::Model)
                                }
                                Ok("skills") => {
                                    this.newchat.sheet = Some(crate::board::Sheet::Skills)
                                }
                                Ok("prompt") => {
                                    this.newchat.sheet = Some(crate::board::Sheet::Prompt)
                                }
                                Ok("dir") => this.newchat.sheet = Some(crate::board::Sheet::Dir),
                                Ok("machine") => {
                                    this.newchat.sheet = Some(crate::board::Sheet::Machine)
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 连 bjes6b 拿置顶。**失败就静默降级**：`pins_client` 留 `None`，
    /// 界面上没有置顶功能，但不影响聊天 —— 客户端不该因为别人的块没装就红一片。
    fn load_pins(&mut self, cx: &mut Context<Self>) {
        let Some(host) = pins_host() else { return };
        let board = pins_board();
        cx.spawn(async move |this, cx| {
            let Some(handle) = crate::TOKIO.get().cloned() else {
                return;
            };
            let url = match handle
                .spawn(async move {
                    nous_engine::resolve::resolve_engine(&host, &board, SELF_BLOCK, MOBILE_BLOCK)
                        .await
                })
                .await
            {
                Ok(Ok(url)) => url,
                Ok(Err(e)) => {
                    log::info!("手机端那块 resolve 不到，置顶功能关掉：{e}");
                    return;
                }
                Err(e) => {
                    log::info!("resolve 任务挂了，置顶功能关掉：{e}");
                    return;
                }
            };
            let client = match PinClient::new(url.as_str(), Duration::from_secs(8)) {
                Ok(c) => Arc::new(c),
                Err(e) => {
                    log::warn!("置顶客户端建不起来：{e}");
                    return;
                }
            };
            let ids = {
                let c = client.clone();
                handle.spawn(async move { c.pinned_ids().await }).await
            };
            let _ = this.update(cx, |this, cx| match ids {
                Ok(Ok(ids)) => {
                    log::info!("置顶读到了：{ids:?}");
                    this.pins_client = Some(client);
                    this.pins = ids;
                    cx.notify();
                }
                Ok(Err(e)) => log::info!("置顶读不到，功能关掉：{e}"),
                Err(e) => log::info!("置顶任务挂了，功能关掉：{e}"),
            });
        })
        .detach();
    }

    /// 置顶 / 取消置顶。**乐观更新**：界面先动，写失败再退回去（手机端同一套语义）。
    fn toggle_pin(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(client) = self.pins_client.clone() else {
            return;
        };
        let want = !self.pins.contains(&id);
        let before = self.pins.clone();
        if want {
            self.pins.retain(|x| *x != id);
            self.pins.insert(0, id); // 新置顶的排最前 —— 和微信一致
        } else {
            self.pins.retain(|x| *x != id);
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let Some(handle) = crate::TOKIO.get().cloned() else {
                return;
            };
            let res = handle
                .spawn(async move { client.set_pin(id, want).await })
                .await;
            let failed = match res {
                Ok(Ok(())) => {
                    log::info!("置顶写进去了：{id} -> {want}");
                    false
                }
                Ok(Err(e)) => {
                    log::warn!("置顶没写进去：{id} -> {want}：{e}");
                    true
                }
                Err(e) => {
                    log::warn!("置顶任务挂了：{id}：{e}");
                    true
                }
            };
            if failed {
                // 回滚：绝不显示「已置顶」然后其实没存上。
                let _ = this.update(cx, |this, cx| {
                    this.pins = before;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 定时重读 `/v1/sessions`。聚焦才拉、流式中不拉。
    ///
    /// 内容没变就不 `cx.notify()` —— 每 3 秒重绘一次窗口是白烧 CPU。
    fn start_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(3)).await;
                let go = this
                    .update(cx, |this, _| {
                        this.focused && !this.streaming && this.engine.is_some()
                    })
                    .unwrap_or(false);
                if !go {
                    continue;
                }
                let (engine, selected) =
                    match this.update(cx, |this, _| (this.engine.clone(), this.selected)) {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                let Some(engine) = engine else { continue };
                let Some(h) = crate::TOKIO.get().cloned() else {
                    break;
                };
                // 会话和未读一起拉：两个都是小 JSON，分两次反而多一次 RTT。
                let got = h
                    .spawn(async move {
                        let sessions = engine.sessions().await;
                        let unread = engine.unread().await;
                        (sessions, unread)
                    })
                    .await;
                let Ok((sessions, unread)) = got else {
                    continue;
                };
                let Ok(list) = sessions else {
                    // 断一下就等下一轮，不打断 UI。
                    continue;
                };

                let mut next = list.items;
                sort_sessions(&mut next);
                let _ = this.update(cx, |this, cx| {
                    // 选中的那条被别人写了：重拉它的历史。
                    let stale_entries = selected.is_some_and(|id| {
                        let before = this.sessions.iter().find(|s| s.id == id).map(sig_of);
                        let after = next.iter().find(|s| s.id == id).map(sig_of);
                        before != after
                    });
                    // 未读变了也要重画（红点上不上、消不消）。
                    if let Ok(u) = unread
                        && this.unread != u
                    {
                        this.unread = u;
                        cx.notify();
                    }
                    if this.sessions.iter().map(sig_of).ne(next.iter().map(sig_of)) {
                        this.sessions = next;
                        // 日志里能对上：「列表怎么不动了」先看这行还在不在跳；
                        // 「怎么没跳到最上面」就看置顶那条和水位。
                        log::info!(
                            "列表刷新：{} 行，置顶「{}」（last.id={:?}, busy={:?}）",
                            this.sessions.len(),
                            this.sessions
                                .first()
                                .map(|s| s.name.as_str())
                                .unwrap_or("-"),
                            this.sessions.first().and_then(|s| sig_of(s).2),
                            this.sessions
                                .first()
                                .map(|s| s.busy.as_ref().map(|b| b.to_string()))
                                .unwrap_or(None),
                        );
                        cx.notify();
                    }
                    if stale_entries && let Some(id) = selected {
                        this.select_session(id, true, cx);
                    }
                    // 人正停在某个会话上：新进来的也算读过，红点不该一直挂着。
                    if let Some(id) = this.selected {
                        this.mark_read(id, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// 人在左栏点了一条会话 = 明确离开 New Manual 盘面。
    /// **只有人点才走这里**：poll / SSE / 停止按钮那些内部刷新仍然直接调
    /// `select_session`，否则后台每 3 秒一次刷新会把人正在配的盘面悄悄关掉。
    pub(crate) fn choose_session(&mut self, id: i64, cx: &mut Context<Self>) {
        self.board_close(cx);
        self.select_session(id, false, cx);
    }

    pub(crate) fn select_session(&mut self, id: i64, force: bool, cx: &mut Context<Self>) {
        if self.selected == Some(id) && !force {
            return;
        }
        self.selected = Some(id);
        self.nav.reset();
        self.entries.clear();
        self.entries_error = None;
        self.turn_error = None;
        self.entries_loading = true;
        self.streaming = false;
        self.cfg.open = false;
        self.live_tools.clear();
        self.open_tools.clear();
        // 选中就常连：别的写出方（另一个终端、面板页）进这一条，这边会自己长出来。
        self.start_stream(cx);
        self.mark_read(id, cx);
        cx.notify();

        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let result: Option<Result<nous_engine::types::Entries, nous_engine::EngineError>> =
                match (engine, handle) {
                    (Some(e), Some(h)) => h.spawn(async move { e.entries(id).await }).await.ok(),
                    _ => None,
                };
            let _ = this.update(cx, |this, cx| {
                this.entries_loading = false;
                match result {
                    Some(Ok(entries)) => {
                        // 引擎给的新的在前，聊天区要旧的在上。
                        this.entries = entries.items.into_iter().rev().collect();
                        // 目录默认高亮最新一轮：主视图停在底部，别从第一条亮起。
                        let groups = this.nav_groups();
                        this.nav.current = groups
                            .iter()
                            .flat_map(|g| g.rounds.iter())
                            .last()
                            .map(|r| r.index)
                            .unwrap_or(0);
                        this.scroll.scroll_to_bottom();
                    }
                    Some(Err(e)) => {
                        this.entries = vec![];
                        this.entries_error = Some(e.to_string());
                    }
                    None => this.entries_error = Some("任务没跑完".to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 「我读到这儿了」。有未读才推，别每 3 秒空推一次水位。
    ///
    /// 回执里带着引擎算好的 `unread: 0`，直接抄回表里 —— 水位的真相在服务端，
    /// 客户端不自己减。
    fn mark_read(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.unread.count(id) == 0 {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let Some(h) = crate::TOKIO.get().cloned() else {
            return;
        };
        log::info!("标已读：会话 {id}（{} 条未读）", self.unread.count(id));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(r)) = h.spawn(async move { engine.mark_read(id).await }).await {
                let _ = this.update(cx, |this, cx| {
                    this.unread.apply(&r);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // ── 输入区 `/` 技能命令 ─────────────────────────────────────────

    /// 面板在不在场。只看输入框当前内容这一份真相（不另存 query）：
    /// 整段以 `/` 开头、后面没有空白、且没被 Esc 收起过。
    fn slash_visible(&self, cx: &App) -> bool {
        !self.slash.dismissed
            && self.selected.is_some()
            && !self.newchat.open
            && crate::slash::command(&self.input.read(cx).value()).is_some()
    }

    /// `GET /v1/skills` 只拉一次。失败也不重试 —— render 里若每次失败都 notify，
    /// 就成了「失败 → 重画 → 再拉」的死循环；记一句话，面板给提示。
    fn load_skills(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if self.skills_loading || self.skills.is_some() || self.skills_error.is_some() {
            return;
        }
        self.skills_loading = true;
        cx.spawn(async move |this, cx| {
            let Some(h) = crate::TOKIO.get().cloned() else {
                return;
            };
            let got = h.spawn(async move { engine.skills().await }).await;
            let _ = this.update(cx, |this, cx| {
                this.skills_loading = false;
                match got {
                    Ok(Ok(list)) => {
                        log::info!("技能清单读到了：{} 条", list.skills.len());
                        this.skills = Some(list);
                    }
                    Ok(Err(e)) => this.skills_error = Some(format!("技能清单拉不到：{e}")),
                    Err(e) => this.skills_error = Some(format!("技能清单拉不到：{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 每次 render 重算一次过滤结果。放 render 里而不是订阅里：清单是异步回来的，
    /// 回来那一刻也得重算，render 天然覆盖两种时机。
    fn refresh_slash(&mut self, cx: &App) {
        if !self.slash_visible(cx) {
            self.slash_items.clear();
            return;
        }
        let value = self.input.read(cx).value().to_string();
        let q = crate::slash::command(&value).unwrap_or("");
        let all = self
            .skills
            .as_ref()
            .map(|s| s.skills.as_slice())
            .unwrap_or(&[]);
        self.slash_items = crate::slash::matches(all, q);
        self.slash.clamp(self.slash_items.len());
    }

    /// 当前会话挂着哪些技能。**以 `sessions` 为准**（3 秒轮询会把它刷成引擎的真相），
    /// 不自己另存一份 —— 那会跟真相分叉。
    fn session_skill_names(&self) -> Vec<String> {
        self.selected
            .and_then(|id| self.sessions.iter().find(|s| s.id == id))
            .map(|s| s.skills.clone())
            .unwrap_or_default()
    }

    /// 点一项 / ↵：算出目标数组 → POST /config → 拿回生效值覆盖本地。
    ///
    /// **不做乐观更新**：会话配置是真相，写失败就还是原样，别让界面先骗人。
    fn apply_skill(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.slash.busy {
            return;
        }
        let Some(skill) = self.slash_items.get(ix) else {
            return;
        };
        let Some(id) = self.selected else { return };
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let wanted = crate::slash::toggle(&self.session_skill_names(), &skill.name);
        self.slash.busy = true;
        self.slash.error = None;
        cx.notify();
        log::info!("技能配置：会话 {id} → {wanted:?}");

        cx.spawn_in(window, async move |this, cx| {
            let Some(h) = crate::TOKIO.get().cloned() else {
                return;
            };
            let patch = nous_engine::types::ConfigPatch {
                skills: Some(wanted),
                ..Default::default()
            };
            let got = h
                .spawn(async move { engine.config(id, &patch).await })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.slash.busy = false;
                match got {
                    Ok(Ok(back)) => {
                        // 回包是**生效值**。覆盖本地会话，界面立刻对上真相。
                        if let Some(s) = this.sessions.iter_mut().find(|s| s.id == id) {
                            s.skills = back.skills.clone();
                        }
                        // 选完把 `/xxx` 从输入框里清掉，别让它留着挡下一次输入。
                        this.input
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        this.slash.reset();
                        this.slash.dismissed = true;
                    }
                    Ok(Err(e)) => this.slash.error = Some(format!("没写进去：{e}")),
                    Err(e) => this.slash.error = Some(format!("没写进去：{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 画 `/` 面板（或 `None`）。
    fn slash_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.slash_visible(cx) {
            return None;
        }
        let empty_note = if let Some(e) = &self.skills_error {
            Some(e.as_str())
        } else if self.skills.is_none() {
            Some("技能清单加载中…")
        } else {
            None
        };
        let on_pick: crate::slash::OnPick = std::rc::Rc::new(
            cx.listener(|this, ix: &usize, window, cx| this.apply_skill(*ix, window, cx)),
        );
        Some(crate::slash::panel(
            self.slash_items.clone(),
            self.session_skill_names(),
            self.slash.cursor,
            self.slash.busy,
            self.slash.error.clone(),
            empty_note.map(str::to_string),
            on_pick,
        ))
    }

    // ── 输入区左下角那颗 DNA：这条对话的设置 ───────────────────────

    fn cur_session(&self) -> Option<&Session> {
        let id = self.selected?;
        self.sessions.iter().find(|s| s.id == id)
    }

    /// 面板在不在场。跟 `/` 面板一样：没选会话、或者在新对话盘面上，都不该出现。
    fn cfg_visible(&self) -> bool {
        self.cfg.open && self.selected.is_some() && !self.newchat.open
    }

    /// 开 / 关。打开时顺手拉清单，并把 `/` 那套收掉 —— 两个面板不共存。
    fn toggle_config(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.open {
            self.cfg.open = false;
            cx.notify();
            return;
        }
        if self.selected.is_none() || self.newchat.open {
            return;
        }
        self.cfg.open = true;
        // 每开一次都回到第一页：停在上一条会话看过的那页只会让人困惑。
        self.cfg.tab = None;
        self.cfg.error = None;
        self.slash.dismissed = true;
        self.cfg_load(cx);
        self.load_skills(cx);
        // 目录从这条会话自己的 cwd 起，不从面板上次待的地方起。
        let cwd = self.cur_session().and_then(|s| s.cwd.clone());
        self.cfg_enter_dir(cwd, cx);
        cx.notify();
    }

    /// 拉三个清单（技能跟 `/` 共用 `load_skills`，不重复拉第二遍）。
    /// 一次只发一批，失败也记一句话不重试 —— 否则 render 里就是「失败 → 重画 → 再拉」。
    fn cfg_load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if self.cfg.loading || self.cfg.models.is_some() {
            return;
        }
        self.cfg.loading = true;
        cx.spawn(async move |this, cx| {
            let Some(h) = crate::TOKIO.get().cloned() else {
                return;
            };
            let got = h
                .spawn(async move {
                    let models = engine.models().await;
                    let prompts = engine.prompts().await;
                    let machines = engine.machines().await;
                    (models, prompts, machines)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cfg.loading = false;
                match got {
                    Ok((m, p, mc)) => {
                        // **逐份独立保存**：以前只在「三份全 Ok」时才落盘，
                        // 结果内核偶发 502 时成功的那两份也一起丢了，右栏整块变报错。
                        let mut errs: Vec<String> = Vec::new();
                        match m {
                            Ok(m) => {
                                log::info!("配置面板：模型 {} 个", m.items.len());
                                this.cfg.models = Some(m);
                            }
                            Err(e) => errs.push(format!("模型：{e}")),
                        }
                        match p {
                            Ok(p) => {
                                log::info!("配置面板：模板 {} 条", p.prompts.len());
                                this.cfg.prompts = Some(p);
                            }
                            Err(e) => errs.push(format!("模板：{e}")),
                        }
                        match mc {
                            Ok(mc) => {
                                log::info!("配置面板：机器 {} 台", mc.machines.len());
                                this.cfg.machines = Some(mc);
                            }
                            Err(e) => errs.push(format!("机器：{e}")),
                        }
                        // 只留一句话给「那一页」用，别再拿它遮住整块右栏。
                        this.cfg.error = (!errs.is_empty())
                            .then(|| format!("清单拉不到 —— {}", errs.join("；")));
                    }
                    Err(e) => this.cfg.error = Some(format!("清单拉不到：{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 进某个目录。`None` = 从这条会话的 cwd 起。
    fn cfg_enter_dir(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let want = path.or_else(|| self.cur_session().and_then(|s| s.cwd.clone()));
        cx.spawn(async move |this, cx| {
            let Some(h) = crate::TOKIO.get().cloned() else {
                return;
            };
            let got = h
                .spawn(async move { engine.dirs(want.as_deref()).await })
                .await;
            let _ = this.update(cx, |this, cx| {
                match got {
                    Ok(Ok(d)) => this.cfg.dirs = Some(d),
                    // 目录读不到只影响目录那一页，别把模型那份 error 也污染了。
                    Ok(Err(e)) => log::warn!("目录读不到：{e}"),
                    Err(e) => log::warn!("目录读不到：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 把一份 patch 发给内核。回包是**生效值**，拿它覆盖本地会话 ——
    /// 不能自己把改的值写上去：换模型时服务端会把档位按新模型重夹一次，
    /// 那个结果只有回包里有。
    fn cfg_apply(&mut self, patch: ConfigPatch, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if self.cfg.busy {
            return;
        }
        self.cfg.busy = true;
        self.cfg.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let Some(h) = crate::TOKIO.get().cloned() else {
                return;
            };
            let got = h
                .spawn(async move { engine.config(id, &patch).await })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cfg.busy = false;
                match got {
                    Ok(Ok(back)) => {
                        if let Some(s) = this.sessions.iter_mut().find(|s| s.id == id) {
                            s.model = back.model.clone();
                            s.thinking_level = back.thinking_level.clone();
                            s.skills = back.skills.clone();
                            s.template = back.template.clone();
                            s.read_only = back.read_only;
                            s.cwd = back.cwd.clone();
                            s.machine = back.machine.clone();
                        }
                        log::info!(
                            "配置写进去了：会话 {id} → {}",
                            back.model.as_deref().unwrap_or("—")
                        );
                    }
                    Ok(Err(e)) => this.cfg.error = Some(format!("没写进去：{e}")),
                    Err(e) => this.cfg.error = Some(format!("没写进去：{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 工具行最左边那颗。以前是个死的笑脸图标，这里换成一个真按钮。
    fn cfg_first_tool(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.cfg_visible();
        div()
            .id(ElementId::Name("tool-config".into()))
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded(px(6.))
            .when(open, |el| el.bg(rgba(0x07c16022)))
            .hover(|s| s.cursor_pointer().bg(rgba(0x00000012)))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.toggle_config(window, cx);
            }))
            .child(
                Icon::new(IconName::Dna)
                    .size(px(18.))
                    .text_color(rgb(if open { 0x0a7a3f } else { 0x4d5156 })),
            )
            .into_any_element()
    }

    /// 把面板浮在聊天区左下角。挂在**整窗**根节点上，父容器够大，hitbox 不会被裁。
    fn cfg_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.cfg_panel(cx)?;
        Some(
            div()
                .absolute()
                .left(px(350.))
                .bottom(px(118.))
                .child(panel)
                .into_any_element(),
        )
    }

    /// 画面板（或 `None`）。回调全在这里接上，`config_panel` 只管画。
    fn cfg_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.cfg_visible() {
            return None;
        }
        let s = self.cur_session()?;

        let hooks = crate::config_panel::Hooks {
            tab: Rc::new(cx.listener(|this, t: &crate::config_panel::Tab, _w, cx| {
                this.cfg.tab = Some(*t);
                cx.notify();
            })),
            level: Rc::new(cx.listener(|this, v: &String, _w, cx| {
                this.cfg_apply(
                    ConfigPatch {
                        thinking_level: Some(v.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            })),
            model: Rc::new(cx.listener(|this, v: &(String, String), _w, cx| {
                let (provider, model_id) = v;
                // 空 modelId = 只换了供应商。模型得跟着换成这家的第一个，
                // 不然会停在别家的 modelId 上，服务端直接 400。
                let model_id = if model_id.is_empty() {
                    this.cfg
                        .models
                        .as_ref()
                        .and_then(|m| m.items.iter().find(|it| &it.provider == provider))
                        .map(|it| it.model_id.clone())
                        .unwrap_or_default()
                } else {
                    model_id.clone()
                };
                if model_id.is_empty() {
                    return;
                }
                this.cfg_apply(
                    ConfigPatch {
                        model: Some(format!("{provider}/{model_id}")),
                        ..Default::default()
                    },
                    cx,
                );
            })),
            skill: Rc::new(cx.listener(|this, name: &String, _w, cx| {
                // 内核对技能是**相对改法**（只增删差集），所以这里传完整目标数组就行，
                // 不用自己算增删 —— 跟 `/` 面板走的是同一条路。
                let wanted = crate::slash::toggle(&this.session_skill_names(), name);
                this.cfg_apply(
                    ConfigPatch {
                        skills: Some(wanted),
                        ..Default::default()
                    },
                    cx,
                );
            })),
            template: Rc::new(cx.listener(|this, v: &Option<String>, _w, cx| {
                this.cfg_apply(
                    ConfigPatch {
                        template: Some(v.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            })),
            machine: Rc::new(cx.listener(|this, v: &Option<String>, _w, cx| {
                this.cfg_apply(
                    ConfigPatch {
                        machine: Some(v.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            })),
            dir: Rc::new(cx.listener(|this, path: &String, _w, cx| {
                // 点目录是**导航**不是写配置：只拉那一层，不动会话的 cwd。
                this.cfg_enter_dir(Some(path.clone()), cx);
            })),
            readonly: Rc::new(cx.listener(|this, on: &bool, _w, cx| {
                this.cfg_apply(
                    ConfigPatch {
                        read_only: Some(*on),
                        ..Default::default()
                    },
                    cx,
                );
            })),
        };

        Some(crate::config_panel::render(
            &self.cfg,
            self.skills.as_ref(),
            s,
            &hooks,
        ))
    }

    // ── 列表 / 气泡 ─────────────────────────────────────────

    /// 发一条。SSE 走的是 `requestId` 幂等锚：重发不会答两遍。
    ///
    /// 返回「这一句到底有没有发出去」：`retry_last` 靠它决定红条留不留。
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // 面板在场时，回车归面板（拦键在 interceptor 里做）；这里再兜一道，
        // 防止某个路径绕过去把 `/deep` 当消息发出去。
        if self.slash_visible(cx) {
            return false;
        }
        // 生成中**不发**：回车这时只想接着写，不该把半句话塞进队列。
        // 引擎的 `whenBusy` 值域还没确认，先不传 —— 确认了只需要在这一个入口加分支。
        if self.streaming {
            log::info!("正在生成，这一句不发（回车被拦下）");
            return false;
        }
        let Some(id) = self.selected else {
            return false;
        };
        let Some(engine) = self.engine.clone() else {
            return false;
        };
        let text = self.input.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return false;
        }

        // 幂等锚：同一句话重发拿到同一次提交。
        let request_id = format!(
            "desk-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            text.len()
        );

        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });

        // 自己先当听众：先在本地插一条用户气泡，再开流。
        self.entries.push(nous_engine::types::Entry {
            id: serde_json::Value::Null,
            kind: "pi.user".into(),
            text: Some(text.clone()),
            parent: None,
            extra: Default::default(),
        });
        self.live = Some(String::new());
        self.live_tools.clear();
        // **不换 Live**：常连的合帧循环靠 `Arc::ptr_eq` 认身份，换掉会让循环退场。
        self.live_state.reset_round();
        self.streaming = true;
        self.turn_error = None;
        self.user_stopped = false;
        cx.notify();

        let live = self.live_state.clone();
        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            if let Some(h) = handle {
                // 发消息：不传 waitMs，立即回 202，正文靠 SSE。
                let _ = h
                    .spawn(async move {
                        if let Err(e) = engine.send_message(id, &text, &request_id).await {
                            live.fail(e.to_string());
                        }
                    })
                    .await;
            }
            let _ = this.update(cx, |_this, cx| cx.notify());
        })
        .detach();

        self.start_stream(cx);
        true
    }

    /// 连 SSE。**选中就一直连着**，断了 1s 重连；切会话才换。
    ///
    /// 两半：tokio 任务只往 `live_state` 里塞，UI 侧 16ms 取一次。
    fn start_stream(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if self.stream_for == Some(id) {
            return;
        }
        self.stream_for = Some(id);
        self.live = None;
        self.live_state = Live::new();
        let live = self.live_state.clone();

        // 读流：断了就重连，直到这个会话不再被选中。
        cx.spawn(async move |this, cx| {
            loop {
                let still = this
                    .update(cx, |this, _| this.stream_for == Some(id))
                    .unwrap_or(false);
                if !still {
                    break;
                }
                let Some(h) = crate::TOKIO.get().cloned() else {
                    break;
                };
                let engine = engine.clone();
                let live = live.clone();
                let _ = h
                    .spawn(async move {
                        use futures_util::StreamExt as _;
                        match engine.events(id).await {
                            Ok(mut stream) => {
                                log::info!("SSE 连上：会话 {id}");
                                while let Some(item) = stream.next().await {
                                    match item {
                                        Ok(event) => live.ingest(&event),
                                        Err(e) => {
                                            log::warn!("SSE 中断：{id}：{e}");
                                            live.fail(e.to_string());
                                            break;
                                        }
                                    }
                                }
                                log::info!("SSE 结束：会话 {id}");
                            }
                            Err(e) => {
                                log::warn!("SSE 连不上：{id}：{e}");
                                live.fail(e.to_string())
                            }
                        }
                    })
                    .await;
                // 断线不刷屏：等一秒重连（已切走就在下一圈退出）。
                Timer::after(Duration::from_millis(1000)).await;
            }
        })
        .detach();

        // UI 侧 16ms 一帧：把攒下的字写进气泡。
        let live = self.live_state.clone();
        cx.spawn(async move |this, cx| {
            // 看门狗：这一轮最后一条「有新东西」的时刻。
            let mut last_change = std::time::Instant::now();
            // 收到过「此刻没在生成」的快照。上游报错时失败事件可能早于我们订阅，
            // 之后这条流只剩一个空 snapshot —— 不设上限就会永远停在「停止」。
            let mut idle_snapshot = false;
            loop {
                Timer::after(Duration::from_millis(16)).await;
                let alive = this
                    .update(cx, |this, _| this.stream_for == Some(id))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                let drained = live.drain();
                let mut reload = false;
                let cont = this
                    .update(cx, |this, cx| {
                        // 换过会话的旧循环：立刻退场，别去改新会话的状态。
                        if !this.live_state.same_round(&live) {
                            log::info!("旧的合帧循环退场");
                            return false;
                        }
                        let Some(drained) = drained else {
                            // 没有新事件：把「永远等下去」变成一句能重试的话。
                            if this.streaming {
                                let limit = if idle_snapshot {
                                    Duration::from_secs(8)
                                } else {
                                    Duration::from_secs(30)
                                };
                                if last_change.elapsed() > limit {
                                    this.live = None;
                                    this.streaming = false;
                                    if !this.user_stopped {
                                        let why = if idle_snapshot {
                                            "这一轮没有跑起来（引擎那边没有正在生成的消息）"
                                        } else {
                                            "30 秒没有任何输出"
                                        };
                                        log::warn!("{why}，判定本轮失败");
                                        this.turn_error = Some(format!("{why}，可以重试"));
                                    }
                                    cx.notify();
                                }
                            }
                            return true;
                        };
                        last_change = std::time::Instant::now();
                        if drained.idle_snapshot && drained.text.is_empty() && !drained.done {
                            idle_snapshot = true;
                        }
                        if let Some(err) = drained.error {
                            this.live = None;
                            this.streaming = false;
                            if this.user_stopped {
                                // 用户按了停止：流被掐断是**预期**，别弹红条。
                                log::info!("停止后流断开，按预期忽略：{err}");
                            } else {
                                log::warn!("生成失败：{err}");
                                this.turn_error = Some(err);
                            }
                            cx.notify();
                            return true;
                        }
                        this.live = Some(drained.text);
                        // 工具卡片：这一轮调过哪些工具，SSE 里是带名字的（`ToolRun.name`）。
                        // 以前 `Drained.tools` 没人看，所以 `live.rs` 里那两个字段一直是死代码。
                        this.live_tools = drained.tools;
                        // SSE 首帧带来的时间戳：按天分组目录靠它，不自己编日期。
                        if let Some(times) = drained.times {
                            this.nav.times = times;
                        }
                        this.scroll.scroll_to_bottom();
                        if drained.done {
                            // 一轮结束：交出去，历史以引擎为准重拉一遍。流不关，继续等下一轮。
                            this.streaming = false;
                            idle_snapshot = false;
                            reload = true;
                            // 转成历史之前先收起来：历史版会按引擎的说法重画一遍。
                            this.live_tools.clear();
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !cont {
                    break;
                }
                if reload {
                    let _ = this.update(cx, |this, cx| {
                        this.live = None;
                        this.select_session(id, true, cx);
                    });
                }
            }
        })
        .detach();
    }

    /// 「停止」：`POST /v1/sessions/:id/abort`，把当前这一轮掐掉。
    ///
    /// 先改 UI 再发请求（按钮立刻变回「发送」，不等网络）；掐完以**引擎历史为准**
    /// 重拉一次 —— 引擎已经落下的半截回答要显示出来。失败只记日志：停止是「少做事」，
    /// 网络抖一下没必要吓用户。
    fn stop_generation(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let Some(engine) = self.engine.clone() else {
            return;
        };
        if !self.streaming {
            return;
        }
        self.streaming = false;
        self.user_stopped = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let Some(handle) = crate::TOKIO.get().cloned() else {
                return;
            };
            match handle.spawn(async move { engine.abort(id).await }).await {
                Ok(Ok(resp)) => log::info!("停止成功：{resp}"),
                Ok(Err(e)) => log::warn!("停止失败（不打扰用户）：{e}"),
                Err(e) => log::warn!("停止任务没跑完：{e}"),
            }
            let _ = this.update(cx, |this, cx| {
                this.live = None;
                this.select_session(id, true, cx);
            });
        })
        .detach();
    }

    /// 「重试」：把最后一条用户消息原样再发一遍（新的 `requestId`）。
    fn retry_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .entries
            .iter()
            .rev()
            .find(|e| e.kind == "pi.user")
            .and_then(|e| e.text.clone());
        match text {
            Some(text) if !text.trim().is_empty() => {
                self.input
                    .update(cx, |input, cx| input.set_value(text, window, cx));
                // **发出去了才收红条**：send() 可能被「正在生成」拦下，先清掉会让用户
                // 以为重试成功了（真踩过）。
                if self.send(window, cx) {
                    self.turn_error = None;
                } else {
                    log::warn!("重试没发出去（可能还在生成中）");
                }
                cx.notify();
            }
            _ => {
                self.turn_error = None;
                cx.notify();
            }
        }
    }

    // ── 三个栏 ──────────────────────────────────────────────

    fn render_icon_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 微信桌面版最左的深色窄条：聊天 / 通讯录两个入口，底部「≡ 更多」。
        div()
            .flex()
            .flex_col()
            .w(px(ICON_BAR_W))
            .h_full()
            .bg(rgb(SIDEBAR_DARK))
            .items_center()
            .py(px(10.))
            .gap(px(6.))
            .child(self.tab_button(IconName::MessageCircle, Tab::Chats, cx))
            .child(self.tab_button(IconName::BookUser, Tab::Templates, cx))
            .child(div().flex_1())
            .child(self.more_button(cx))
    }

    /// 底部那三个横杠。点和菜单同一个状态 —— 开着的时候按钮自己也亮着。
    fn more_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.menu_open;
        div()
            .id(ElementId::Name("rail-more".into()))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_menu(cx)))
            .flex()
            .items_center()
            .justify_center()
            .size(px(40.))
            .rounded(px(8.))
            .when(open, |el| el.bg(rgb(0x3f3f3f)))
            .hover(|el| el.bg(rgb(0x3a3a3a)))
            .child(
                Icon::new(IconName::Menu)
                    .with_size(Size::Size(px(20.)))
                    .text_color(rgb(if open { 0xffffff } else { 0x9a9a9a })),
            )
    }

    fn tab_button(&self, name: IconName, tab: Tab, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.tab == tab;
        let id = match tab {
            Tab::Chats => "tab-chats",
            Tab::Templates => "tab-templates",
        };
        div()
            .id(ElementId::Name(id.into()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.switch_tab(tab, cx);
            }))
            .flex()
            .items_center()
            .justify_center()
            .size(px(40.))
            .rounded(px(8.))
            .hover(|el| el.bg(rgb(0x3a3a3a)))
            .child(
                Icon::new(name)
                    .with_size(Size::Size(px(22.)))
                    .text_color(rgb(if active { WECHAT_GREEN } else { 0x9a9a9a })),
            )
    }

    /// 悬停 → 「滚动条在不在场」。
    ///
    /// 移开不能马上撤元素：撤了就看不到那 250ms 的淡出，会「啪」一下消失。
    /// 所以先留着，等淡出跑完再收 —— 期间要是又进来了，就当成淡入继续用。
    pub(crate) fn bar_hover(&mut self, which: Bar, hovered: bool, cx: &mut Context<Self>) {
        let (hover, shown) = match which {
            Bar::Chat => (&mut self.chat_hover, &mut self.chat_bar),
            Bar::List => (&mut self.list_hover, &mut self.list_bar),
            Bar::Tree => (&mut self.tree_hover, &mut self.tree_bar),
            Bar::Presets => (&mut self.presets_hover, &mut self.presets_bar),
        };
        if *hover == hovered {
            return;
        }
        *hover = hovered;
        *shown = true;
        cx.notify();
        if hovered {
            return;
        }
        cx.spawn(async move |this, cx| {
            Timer::after(scrollbar::FADE).await;
            this.update(cx, |this, cx| {
                let still_hovered = match which {
                    Bar::Chat => this.chat_hover,
                    Bar::List => this.list_hover,
                    Bar::Tree => this.tree_hover,
                    Bar::Presets => this.presets_hover,
                };
                if still_hovered {
                    return;
                }
                match which {
                    Bar::Chat => this.chat_bar = false,
                    Bar::List => this.list_bar = false,
                    Bar::Tree => this.tree_bar = false,
                    Bar::Presets => this.presets_bar = false,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 输入区的悬停提示：鼠标放上去 → 淡入一行浅灰字（「Enter 发送 · Shift+Enter 换行」），
    /// 移开 → 250ms 淡出。跟 `bar_hover` 同一套状态机，只是不经过 `Bar` 枚举。
    fn composer_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.composer_hover == hovered {
            return;
        }
        self.composer_hover = hovered;
        self.composer_hint = true;
        cx.notify();
        if hovered {
            return;
        }
        cx.spawn(async move |this, cx| {
            Timer::after(scrollbar::FADE).await;
            this.update(cx, |this, cx| {
                if this.composer_hover {
                    return;
                }
                this.composer_hint = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 当前搜索词（已 trim）。空 = 没在搜。
    fn query(&self, cx: &Context<Self>) -> String {
        self.search.read(cx).value().trim().to_string()
    }

    /// 列表顶上那个搜索框。常驻，不藏起来 —— 会话上百条之后「找得到」比「好看」重要。
    ///
    /// 高度交给组件（单行的 `.h()` 只管多行，设了也不生效 —— 试过了）。
    fn render_search_box(&self) -> AnyElement {
        Input::new(&self.search)
            .small()
            .cleanable(true)
            .prefix(
                Icon::new(IconName::Search)
                    .text_color(rgb(0xaaaaaa))
                    .size(px(14.)),
            )
            .into_any_element()
    }

    /// 搜索中的列表：**先过滤，再按组摆**。
    ///
    /// 顺序是固定的「置顶 → 今天 → 7 天内 → 更早」，跟 `pins.json` 和 `updated_at` 走；
    /// 组内保持 `ordered` 里的先后（置顶组就是 `pins` 给的顺序）。
    ///
    /// 没搜到不画空列表：给一句「没有找到」比一片白好认，至少知道是搜不到而不是列表塌了。
    fn search_rows(
        &self,
        ordered: &[&Session],
        q: &str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use crate::search::{Group, group_of, hit};
        let today = chrono::Local::now().date_naive();
        let mut out: Vec<AnyElement> = Vec::new();
        let mut total = 0usize;
        for g in [Group::Pinned, Group::Today, Group::Week, Group::Older] {
            let items: Vec<&Session> = ordered
                .iter()
                .copied()
                .filter(|s| hit(s, q).is_some())
                .filter(|s| group_of(s.updated_at, self.pins.contains(&s.id), today) == g)
                .collect();
            if items.is_empty() {
                continue;
            }
            total += items.len();
            out.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .text_color(rgb(0xaaaaaa))
                    .when(g == Group::Pinned, |el| {
                        el.child(
                            Icon::new(IconName::Pin)
                                .size(px(11.))
                                .text_color(rgb(0x2e7d32)),
                        )
                    })
                    .child(format!("{} · {}", g.label(), items.len()))
                    .into_any_element(),
            );
            out.extend(items.iter().map(|s| self.session_row(s, q, cx)));
        }
        if total == 0 {
            out.push(
                div()
                    .w_full()
                    .py(px(28.))
                    .text_size(px(12.))
                    .text_color(rgb(0xaaaaaa))
                    .text_center()
                    .child("没有找到相关会话")
                    .into_any_element(),
            );
        }
        out
    }

    /// 一行「可能带高亮」的文本。
    ///
    /// `q` 空的时候就是一根普通的截断文本 —— 跟加搜索之前**一字不差**；
    /// 在搜的时候拆成几段，命中的那段垫一层浅绿底。
    ///
    /// 为什么不用 `StyledText::with_runs`：`TextRun` 要求显式给 `Font`，而这里拿不到
    /// 「当前继承的是哪个字体」。拆 div 横排就不用管字体，代价是超宽时直接裁掉、
    /// 不补省略号。
    fn hl_line(text: &str, q: &str, size: Option<f32>, color: Option<u32>) -> AnyElement {
        if q.trim().is_empty() {
            let mut el = div().truncate();
            if let Some(s) = size {
                el = el.text_size(px(s));
            }
            if let Some(c) = color {
                el = el.text_color(rgb(c));
            }
            return el.child(text.to_string()).into_any_element();
        }
        let mut row = div().flex().min_w(px(0.)).overflow_hidden();
        for (part, hot) in crate::search::runs(text, q) {
            let mut seg = div().flex_none().whitespace_nowrap();
            if let Some(s) = size {
                seg = seg.text_size(px(s));
            }
            if let Some(c) = color {
                seg = seg.text_color(rgb(c));
            }
            if hot {
                // 高亮的底色 / 字色跟 demo 一致（淡绿底 + 深绿字）。
                seg = seg.bg(rgb(0xc8f0d8)).text_color(rgb(0x0a7a3f));
            }
            row = row.child(seg.child(part));
        }
        row.into_any_element()
    }

    fn render_session_list(&self, cx: &mut Context<Self>) -> AnyElement {
        // 分支视图是左列的另一种内容：同一块地方，换一份名单。
        if self.branches {
            if let Some(root) = self.branch_of {
                return self.render_branch_list(root, cx);
            }
        }
        // 默认只画主线：`parent != null` 的是 fork 分支，只在分支视图里出现 ——
        // 不然同一条分支会在两个地方各出现一次。
        //
        // 置顶排序只在**主线**这一份名单里做（分支视图按树画，不掺置顶）。
        let mainline: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.parent.is_none())
            .collect();
        let in_order: Vec<i64> = mainline.iter().map(|s| s.id).collect();
        let ordered: Vec<&Session> = order_ids(&in_order, &self.pins)
            .into_iter()
            .filter_map(|id| mainline.iter().find(|s| s.id == id).copied())
            .collect();
        let q = self.query(cx);
        let rows: Vec<AnyElement> = if q.is_empty() {
            // 没在搜：老样子。**一行都不动** —— 搜索不能把常态列表的顺序改了。
            ordered
                .iter()
                .map(|s| self.session_row(s, &q, cx))
                .collect()
        } else {
            self.search_rows(&ordered, &q, cx)
        };
        div()
            .flex()
            .flex_col()
            .w(px(SESSION_LIST_W))
            .h_full()
            .bg(rgb(LIST_BG))
            .child(
                div()
                    .p(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("NousDesk")
                    // 建会话的唯一入口。以前只能从模板起，现在多一条「从零配」的路。
                    .child(
                        div()
                            .id("new-chat-btn")
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(rgb(WECHAT_GREEN))
                            .hover(|s| s.bg(rgb(0xe8f8ee)))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.board_open(window, cx);
                            }))
                            .child("+ New Manual"),
                    ),
            )
            .child(div().px(px(12.)).child(self.status_line(cx)))
            // 搜索框：常驻在列表顶上（方案 6）。空着就是一行灰字，不占额外的排。
            .child(div().px(px(12.)).pb(px(8.)).child(self.render_search_box()))
            .child(
                div()
                    .id("session-scroll-area")
                    .relative()
                    .flex_1()
                    // `min_h(0)`：flex 项默认 `min-height: auto`，不加这行这个「视口」
                    // 会被内容撑到真实几万像素高，永远不需要滚（滚动条也就不会出现）。
                    .min_h(px(0.))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.bar_hover(Bar::List, *hovered, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .size_full()
                            .id("session-scroll")
                            .overflow_y_scroll()
                            .track_scroll(&self.list_scroll)
                            .children(rows),
                    )
                    .child(scrollbar::overlay(
                        "session-scrollbar",
                        &self.list_scroll,
                        self.list_bar,
                        self.list_hover,
                    )),
            )
            .into_any_element()
    }

    /// 聊天区：9 号方案的短主视图 + 右侧历史抽屉。目录列由 `render()` 插在它左边。
    fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(id) = self.selected else {
            return div()
                .flex()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .child("选个会话，这里开始聊")
                .into_any_element();
        };

        let title = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("会话 {id}"));

        // 9 号方案：主视图只留最近 KEEP 条。`nav.main_from` 被抽屉的桥往前推时才会变长，
        // 顶栏给一颗「收起」把它还原。
        let from = self.nav.from_of(self.entries.len());
        let older = from;
        let short = self.nav.is_short(self.entries.len());

        let status: Vec<AnyElement> = if self.entries_loading {
            vec![
                div()
                    .p(px(12.))
                    .text_size(px(12.))
                    .child("拉历史中…")
                    .into_any_element(),
            ]
        } else if let Some(e) = &self.entries_error {
            vec![
                div()
                    .p(px(12.))
                    .text_size(px(12.))
                    .text_color(rgb(0xd32f2f))
                    .child(format!("拉历史失败：{e}"))
                    .into_any_element(),
            ]
        } else if self.entries.is_empty() {
            vec![
                div()
                    .p(px(12.))
                    .text_size(px(12.))
                    .child("还没消息，先从模板开一条。")
                    .into_any_element(),
            ]
        } else {
            vec![]
        };

        // 主视图的气泡。前面挂「更早 N 条」入口（9 号方案的那扇门）。
        let mut main_rows: Vec<AnyElement> = Vec::new();
        if older > 0 && status.is_empty() {
            main_rows.push(crate::nav_ui::more_bar(older, cx));
            main_rows.push(
                div()
                    .mb(px(12.))
                    .text_size(px(11.))
                    .text_color(rgb(0xa0a0a0))
                    .child("下面是最近几轮 · 旧的都在抽屉里")
                    .into_any_element(),
            );
        }
        main_rows.extend(status);
        if self.entries.len() > from {
            main_rows.extend(self.nav_rows(&self.entries[from..], cx));
        }

        // 抽屉里的全量正文。只有开着时才画 —— 不然每次重绘都要解析整段历史。
        let drawer_rows: Vec<AnyElement> = if self.nav.drawer_open {
            self.nav_rows(&self.entries, cx)
        } else {
            vec![]
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            // `min_w(0)` 不能省：flex 项默认 `min-width: auto`，历史里只要有一条
            // 折不开的长行（工具输出整段），这块就被撑得比窗口还宽（坑.md 16）。
            .min_w(px(0.))
            .h_full()
            .bg(rgb(CHAT_BG))
            .child(
                div()
                    .flex()
                    .h(px(48.))
                    .w_full()
                    .flex_none()
                    .items_center()
                    .gap(px(9.))
                    .px(px(12.))
                    .bg(rgb(LIST_BG))
                    .child(div().flex_none().truncate().child(title))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(if self.nav.drawer_open {
                                "目录指向抽屉里的全量历史".to_string()
                            } else if short {
                                format!("主视图最近 {} 条", self.entries.len() - from)
                            } else {
                                format!("已展开 {} 条", self.entries.len() - from)
                            }),
                    )
                    .child(div().flex_1())
                    // 分支：分叉树唯一的入口。激活时变绿。
                    .child(self.branch_button(id, cx))
                    .children((!short).then(|| crate::nav_ui::collapse_button(cx)))
                    .child(crate::nav_ui::history_button(cx)),
            )
            .child(
                div()
                    // `on_hover` 挂在 `Stateful` 上，所以这里先给个 id。
                    .id("chat-scroll-area")
                    .relative()
                    .flex_1()
                    // 鼠标在这块内容上 → 滚动条出现（demo 的 3 号：平时一条线都没有）。
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.bar_hover(Bar::Chat, *hovered, cx)
                    }))
                    .min_w(px(0.))
                    // 竖版的 `min_w(0)`：不加它这个「视口」会被内容撑到十几万像素高。
                    .min_h(px(0.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .size_full()
                            .id("chat-scroll")
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .on_scroll(
                                cx.listener(|this, _e: &ScrollEvent, _, cx| this.nav_spy(cx)),
                            )
                            .py(px(8.))
                            .children(main_rows),
                    )
                    .child(scrollbar::overlay(
                        "chat-scrollbar",
                        &self.scroll,
                        self.chat_bar,
                        self.chat_hover,
                    ))
                    .child(crate::nav_ui::drawer(
                        self.nav.drawer_open,
                        drawer_rows,
                        &self.nav.drawer_scroll,
                        cx,
                    )),
            )
            // 这一轮的失败就地显示：红条 + 重试 + 收起。不占左栏那条「连不上引擎」的通道。
            // 按钮左锚、文字放最后（GPUI 里长文字会把它右边的东西顶出屏幕）。
            .when_some(self.turn_error.clone(), |el, message| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .w_full()
                        .px(px(12.))
                        .py(px(8.))
                        .bg(rgb(0xfff1f0))
                        .border_t(px(1.))
                        .border_color(rgb(0xf3c9c6))
                        .child(
                            div()
                                .id("turn-error-retry")
                                .flex_shrink_0()
                                .px(px(10.))
                                .py(px(4.))
                                .rounded(px(4.))
                                .bg(rgb(0xc0392b))
                                .text_size(px(12.))
                                .text_color(rgb(0xffffff))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.retry_last(window, cx)
                                }))
                                .child("重 试"),
                        )
                        .child(
                            div()
                                .id("turn-error-dismiss")
                                .flex_shrink_0()
                                .px(px(6.))
                                .py(px(4.))
                                .text_size(px(12.))
                                .text_color(rgb(0x8a6d6a))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.turn_error = None;
                                    cx.notify();
                                }))
                                .child("收起"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(rgb(0xc0392b))
                                .child(short_text(&message, 90)),
                        ),
                )
            })
            .child(crate::composer::render(
                &self.input,
                crate::composer::State {
                    streaming: self.streaming,
                    // 面板在场时发送键是灰的：这时回车归面板，点它也不该发出 `/deep`。
                    can_send: !self.slash_visible(cx)
                        && !self.input.read(cx).value().trim().is_empty(),
                },
                self.composer_hint,
                self.composer_hover,
                cx.listener(|this, hovered: &bool, _, cx| this.composer_hover(*hovered, cx)),
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.send(window, cx);
                }),
                cx.listener(|this, _: &ClickEvent, _window, cx| this.stop_generation(cx)),
                self.slash_panel(cx),
                self.cfg_first_tool(cx),
            ))
            .into_any_element()
    }

    /// 把一段 entries 画成气泡列表（含正在生成的工具和半截回答）。
    /// 主视图和抽屉共用这一套，避免两处渲染长得不一样。
    fn nav_rows(&self, slice: &[Entry], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = slice
            .iter()
            // 空正文的气泡不画 —— 引擎对「有 entry 没内容」会给空串/null，
            // 画出来就是一个白方块（坑.md 第 7 条）。
            .filter(|e| entry_visible(e))
            .map(|e| self.bubble(e, cx).into_any_element())
            .collect();
        // 正在生成这一轮调的工具：SSE 里带名字，所以是「bash · hostname; pwd」。
        for t in &self.live_tools {
            out.push(self.tool_card(&format!("live-{}", t.id), Some(&t.name), &t.output, cx));
        }
        // 正在生成的那条：跟助手气泡同一个样子，只是文字还在长。
        if let Some(live) = &self.live {
            if !live.is_empty() {
                out.push(
                    div()
                        .flex()
                        .justify_start()
                        .px(px(12.))
                        .py(px(4.))
                        .child(
                            div()
                                .max_w(px(480.))
                                .px(px(12.))
                                .py(px(8.))
                                .rounded(px(6.))
                                .bg(rgb(BUBBLE_OTHER))
                                .child(live.clone()),
                        )
                        .into_any_element(),
                );
            }
        }
        out
    }

    // ── 会话内跳转：目录 + 历史抽屉（J + 9） ────────────────────────────

    /// 目录按天分组。水位取引擎给的 `readAt`，没给就不标红点。
    fn nav_groups(&self) -> Vec<crate::session_nav::NavDay> {
        let (read_at, has_unread) = self
            .selected
            .and_then(|id| self.unread.of(id))
            .map(|s| (s.read_at, s.unread > 0))
            .unwrap_or((None, false));
        self.nav.groups(&self.entries, read_at, has_unread)
    }

    /// 滚动容器里，第 `entry_index` 条历史是第几个孩子。
    ///
    /// **不能拿 entry 下标直接当孩子下标**：空正文的气泡被跳过（坑.md 7），
    /// 主视图前面还挂着「更早 N 条」和一行说明，抽屉前面挂着一条提示。
    /// 不对齐的话跳转会偏到别的轮次 —— 这是「目录点一下跳歪」的根因。
    fn nav_child_of(&self, entry_index: usize, drawer: bool) -> Option<usize> {
        if entry_index >= self.entries.len() || !entry_visible(&self.entries[entry_index]) {
            return None;
        }
        nav_child_index(
            &self.entries,
            entry_index,
            if drawer {
                0
            } else {
                self.nav.from_of(self.entries.len())
            },
            drawer,
            self.entries_loading || self.entries_error.is_some(),
        )
    }

    /// 当前高亮该跟着谁：抽屉开着就跟抽屉，否则跟主视图。
    /// **只允许一个「当前正文」** —— 两套滚动状态各跟各的是这里最容易出的错。
    pub(crate) fn nav_spy(&mut self, cx: &mut Context<Self>) {
        let groups = self.nav_groups();
        if groups.is_empty() {
            return;
        }
        let drawer = self.nav.drawer_open;
        let from = if drawer {
            0
        } else {
            self.nav.from_of(self.entries.len())
        };
        let (offset, bounds, max_offset) = if drawer {
            (
                self.nav.drawer_scroll.offset(),
                self.nav.drawer_scroll.bounds(),
                self.nav.drawer_scroll.max_offset(),
            )
        } else {
            (
                self.scroll.offset(),
                self.scroll.bounds(),
                self.scroll.max_offset(),
            )
        };
        // 顶部 80px 那条线：最后一个越过它的用户消息就是「正在读的那轮」。
        // 贴底例外 —— 那时在看最新一轮，而它未必过了那条线。
        let viewport_h = bounds.size.height.as_f32();
        let at_bottom = -offset.y.as_f32() + viewport_h >= -max_offset.y.as_f32() - 4.0;
        let line = -offset.y.as_f32() + 80.0;
        let mut current = groups[0].rounds[0].index;
        for (i, e) in self.entries.iter().enumerate().skip(from) {
            if e.kind != "pi.user" {
                continue;
            }
            let b = self.nav_child_of(i, drawer).and_then(|ix| {
                if drawer {
                    self.nav.drawer_scroll.bounds_for_item(ix)
                } else {
                    self.scroll.bounds_for_item(ix)
                }
            });
            let Some(b) = b else { continue };
            if b.top().as_f32() > line {
                break;
            }
            if let Some(r) = groups
                .iter()
                .flat_map(|g| g.rounds.iter())
                .find(|r| r.entry_index == i)
            {
                current = r.index;
            }
        }
        if at_bottom && let Some(r) = groups.iter().flat_map(|g| g.rounds.iter()).last() {
            current = r.index;
        }
        let group = groups
            .iter()
            .find(|g| g.rounds.iter().any(|r| r.index == current))
            .map(|g| g.label.clone());
        let changed = self.nav.current != current;
        self.nav.current = current;
        // 自动展开当前组：H 单做时「正文在动、目录里找不到我」的那个坑。
        if let Some(g) = group {
            self.nav.reveal(&g);
        }
        if changed {
            // 目录自己别滚离视线：当前那组跟着进视口。
            if let Some(gi) = groups
                .iter()
                .position(|g| g.rounds.iter().any(|r| r.index == current))
            {
                self.nav.nav_scroll.scroll_to_top_of_item(gi);
            }
            cx.notify();
        }
    }

    /// 目录点一行。三个落点：抽屉里定位 / 主视图直接跳 / 打开抽屉定位。
    /// 用户拍板的是第三条 —— **主视图永远 12 行**，不搞「先加载再跳」。
    pub(crate) fn nav_jump(&mut self, index: usize, _group: String, cx: &mut Context<Self>) {
        let groups = self.nav_groups();
        let Some(r) = groups
            .iter()
            .flat_map(|g| g.rounds.iter())
            .find(|r| r.index == index)
            .cloned()
        else {
            return;
        };
        self.nav.current = index;
        if self.nav.drawer_open {
            // 抽屉里是全量，但孩子下标要跳过多余行（见 nav_child_of）。
            if let Some(ix) = self.nav_child_of(r.entry_index, true) {
                self.nav.drawer_scroll.scroll_to_top_of_item(ix);
            }
            cx.notify();
            return;
        }
        let from = self.nav.from_of(self.entries.len());
        if r.entry_index >= from {
            if let Some(ix) = self.nav_child_of(r.entry_index, false) {
                self.scroll.scroll_to_top_of_item(ix);
            }
            cx.notify();
            return;
        }
        self.nav_open_drawer(Some(r.entry_index), cx);
    }

    /// 打开抽屉。`to` 是「滚到哪条 entry」，None = 停在最早那段
    /// （最近 12 行主视图里已经有了，打开抽屉是为了看旧的）。
    pub(crate) fn nav_open_drawer(&mut self, to: Option<usize>, cx: &mut Context<Self>) {
        self.nav.drawer_open = true;
        if let Some(entry) = to {
            let ix = self.nav_child_of(entry, true);
            self.nav.drawer_scroll = ScrollHandle::new();
            if let Some(ix) = ix {
                self.nav.drawer_scroll.scroll_to_top_of_item(ix);
            }
            self.nav.current = self
                .nav_groups()
                .iter()
                .flat_map(|g| g.rounds.iter())
                .filter(|r| r.entry_index <= entry)
                .last()
                .map(|r| r.index)
                .unwrap_or(0);
        } else {
            self.nav.drawer_scroll = ScrollHandle::new();
        }
        // 抽屉开着时目录指向全量正文，第一组先摊开。
        let groups = self.nav_groups();
        if let Some(g) = groups.first() {
            self.nav.reveal(&g.label);
        }
        cx.notify();
    }

    pub(crate) fn nav_close_drawer(&mut self, cx: &mut Context<Self>) {
        self.nav.drawer_open = false;
        cx.notify();
    }

    /// 抽屉头上的桥：把「目录当前高亮的那轮」加载进主视图，然后关抽屉。
    pub(crate) fn nav_to_main(&mut self, cx: &mut Context<Self>) {
        let groups = self.nav_groups();
        let entry_index = groups
            .iter()
            .flat_map(|g| g.rounds.iter())
            .find(|r| r.index == self.nav.current)
            .map(|r| r.entry_index);
        if let Some(ix) = entry_index {
            self.nav.main_from = Some(ix.saturating_sub(1));
        }
        self.nav.drawer_open = false;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(crate) fn nav_collapse(&mut self, cx: &mut Context<Self>) {
        self.nav.main_from = None;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(crate) fn nav_toggle_fold(&mut self, cx: &mut Context<Self>) {
        let groups = self.nav_groups();
        let keys: Vec<String> = groups
            .iter()
            .take(groups.len().saturating_sub(1))
            .map(|g| g.label.clone())
            .collect();
        let any_open = keys.iter().any(|k| !self.nav.closed.contains(k));
        for k in keys {
            if any_open {
                self.nav.closed.insert(k);
            } else {
                self.nav.closed.remove(&k);
            }
        }
        cx.notify();
    }

    pub(crate) fn nav_toggle_group(&mut self, label: String, cx: &mut Context<Self>) {
        if !self.nav.closed.remove(&label) {
            self.nav.closed.insert(label);
        }
        cx.notify();
    }

    // ── 列表 / 气泡 ─────────────────────────────────────────

    fn status_line(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.loading {
            return div().text_size(px(12.)).child("连引擎中…");
        }
        if let Some(e) = &self.error {
            // 连不上就告诉用户「改哪个文件」，别让他猜。
            // 没找到配置时把**找过的地方**全列出来 —— 只说「没找到」等于让人干瞪眼。
            let hint = match Config::path() {
                Some(p) => format!("配置文件：{}", p.display()),
                None => format!(
                    "没找到配置文件，找过：{}",
                    Config::candidates()
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" · ")
                ),
            };
            return div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(12.))
                .text_color(rgb(0xd32f2f))
                .child(format!("连不上：{e}"))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(0x777777))
                        .child(hint),
                );
        }
        div().text_size(px(12.)).child(format!(
            "{} 个会话",
            self.sessions.iter().filter(|s| s.parent.is_none()).count()
        ))
    }

    fn session_row(&self, s: &Session, q: &str, cx: &mut Context<Self>) -> AnyElement {
        // 预览是最后一条 entry 的原文，带换行/连续空白是常态（工具输出整段）。
        // 摊平成一行，配合 `.truncate()` 才是微信那种单行摘要。
        let preview = s
            .last
            .as_ref()
            .and_then(|l| l.text.as_deref())
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let selected = self.selected == Some(s.id);
        // `busy` 是 `"queued" | "running" | <task kind> | null`，不是布尔（`wire.md`）。
        let busy = s.busy.is_some();
        let id = s.id;
        let pinned = self.pins.contains(&id);
        let can_pin = self.pins_client.is_some();

        let row = div()
            .flex()
            .flex_col()
            .justify_center()
            // 固定下限：没有预览的行（新会话、只有工具输出的）也跟别人一样高，列表才成一列节奏。
            .min_h(px(56.))
            .px(px(10.))
            .py(px(6.))
            .gap(px(2.))
            .border_b(px(1.))
            .border_color(rgb(0xe0e0e0))
            .when(selected, |el| el.bg(rgb(0xd9d9d9)))
            // 一行里三样东西：📌 / 名字 / 分支角标 / 时间。左半边 `flex_1` 占满并 truncate，
            // 右半边 `flex_none` 顶到右边 —— 谁都不会被谁顶出去（比 absolute 好使，也不会重叠）。
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(px(6.))
                    // 📌 用 lucide 的 Pin（AllAssets 里有）
                    .when(pinned, |el| {
                        el.child(
                            Icon::new(IconName::Pin)
                                .text_color(rgb(0x2e7d32))
                                .size(px(13.)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Self::hl_line(&s.name, q, None, None)),
                    )
                    // 搜的时候多一个小标签：这一条是名字命中的还是内容命中的。
                    // 不搜的时候不占地方（`when` 整块不渲染）。
                    .when(!q.is_empty(), |el| {
                        el.children(crate::search::hit(s, q).map(|w| {
                            div()
                                .flex_none()
                                .px(px(3.))
                                .rounded(px(3.))
                                .border(px(1.))
                                .border_color(rgb(0xeeeeee))
                                .text_size(px(10.))
                                .text_color(rgb(0xaaaaaa))
                                .child(w.label())
                        }))
                    })
                    // 别用 `⑂`：雅黑里没这个字形，会退化成别的样子。
                    .when(self.descendant_count(id) > 0, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(rgb(WECHAT_GREEN))
                                .child(format!("{} 分支", self.descendant_count(id))),
                        )
                    })
                    .children(when_text(s.updated_at).map(|t| {
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(t)
                    })),
            )
            // 第二行只有一个出口：左边是「正在输入…」或预览（`flex_1` + truncate），
            // 右边是未读红点（`flex_none`）。
            //
            // 早先拆成两个分支、红点挂在 `flex_col` 上，结果红点被拉伸成整条红带
            // —— 列容器里加子元素就是铺满宽度，红点得装在一个 `justify_end` 的横排里。
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(px(6.))
                    .child(if busy {
                        // 忙闲：微信那句「正在输入…」+ 一个小圆点。比一个「忙」字可读。
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(6.))
                                    .rounded(px(3.))
                                    .bg(rgb(WECHAT_GREEN)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(WECHAT_GREEN))
                                    .child("正在输入…"),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Self::hl_line(&preview, q, Some(12.), Some(MUTED)))
                            .into_any_element()
                    })
                    .children(self.unread_badge(id)),
            )
            // `.id()` 把 Div 转成 Stateful<Div>，on_click 只存在于后者。
            .id(ElementId::Name(format!("sess-{id}").into()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.choose_session(id, cx);
            }));

        // 右键菜单：置顶 / 取消置顶。菜单项发的是同一个 action，每行自己的 listener
        // 捕获自己的 id，所以 action 不用带参数。
        //
        // 只能在链尾分支：`context_menu()` 返回的是 `ContextMenu<Stateful<Div>>`，
        // 换个类型就没法再串 `when`（类型对不上）。
        if can_pin {
            row.on_action(cx.listener(move |this, _: &TogglePinAction, _window, cx| {
                this.toggle_pin(id, cx);
            }))
            .context_menu(move |menu, _window, _cx| {
                let label = if pinned {
                    "取消置顶"
                } else {
                    "置顶该聊天"
                };
                menu.menu(label, Box::new(TogglePinAction))
            })
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// 未读数红点。0 条就返回 `None`（别画个「0」在那儿）。
    ///
    /// `99+` 封顶：微信就是这么干的，条数到三位数对谁都没信息量。
    fn unread_badge(&self, id: i64) -> Option<AnyElement> {
        let n = self.unread.count(id);
        if n == 0 {
            return None;
        }
        Some(
            div()
                .flex_none()
                .min_w(px(18.))
                .h(px(18.))
                .px(px(5.))
                .rounded(px(9.))
                .bg(rgb(0xfa5151))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(rgb(0xffffff))
                .child(if n > 99 {
                    "99+".to_string()
                } else {
                    n.to_string()
                })
                .into_any_element(),
        )
    }

    /// 聊天区顶部那颗「⑂ 分支」。
    ///
    /// 点一下：左列从「全部会话」切成「这条会话的分支」；再点一下切回去。
    /// 没有分支也照样画（灰的），点进去是空状态「还没有分支」——
    /// 按钮时有时无会让人以为功能坏了。
    fn branch_button(&self, id: i64, cx: &mut Context<Self>) -> AnyElement {
        // 数字说的是**当前这个视图**有几条分支：开了分支视图就报根的数字，
        // 不然点到一条末端分支时角标会变成 0，看着像「视图空了」。
        let n = match (self.branches, self.branch_of) {
            (true, Some(root)) => self.descendant_count(root),
            _ => self.descendant_count(id),
        };
        let on = self.branches;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .px(px(10.))
            .h(px(28.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(if on { WECHAT_GREEN } else { 0xd0d0d0 }))
            .bg(rgb(if on { WECHAT_GREEN } else { 0xffffff }))
            .text_size(px(13.))
            .text_color(rgb(if on { 0xffffff } else { 0x333333 }))
            .id(ElementId::Name("branch-toggle".into()))
            .cursor_pointer()
            .hover(|el| el.bg(rgb(if on { 0x06a854 } else { 0xf2f2f2 })))
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                if this.branches {
                    this.exit_branches(cx);
                } else {
                    this.enter_branches(this.selected.unwrap_or(id), cx);
                }
            }))
            .child(
                Icon::new(IconName::GitBranch)
                    .with_size(Size::Size(px(15.)))
                    .text_color(rgb(if on { 0xffffff } else { 0x555555 })),
            )
            .child("分支")
            .children(if n > 0 {
                Some(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded(px(8.))
                        .bg(rgb(if on { 0x0aa85a } else { 0xededed }))
                        .text_size(px(11.))
                        .child(format!("{n}")),
                )
            } else {
                None
            })
            .into_any_element()
    }

    /// 工具卡片（甲案：一行细灰条，点开在**块内**滚动）。
    ///
    /// 为什么非要卡片：工具输出是**机器说的话**，混在气泡里既读不出来，也会把聊天区撑坏
    /// （一整段 `ls -l` 能撑得比窗口还宽，见 `坑.md` 16）。
    ///
    /// 标题里那个名字只有实时流给得起；历史里的 `pi.tool-result` 只有输出，
    /// 所以老会话显示「工具输出 · N 行」—— **不从输出内容反推是 bash 还是 read**，那是算命。
    fn tool_card(
        &self,
        key: &str,
        name: Option<&str>,
        raw: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = strip_harness(raw);
        let open = self.open_tools.contains(key);
        let lines = text.lines().count().max(1);
        let bytes = text.len();
        // 失败判据只用引擎自己的标记：剥完壳以 `[error]` 开头。
        let failed = text.trim_start().starts_with("[error]");
        let title = name
            .map(|n| n.to_string())
            .unwrap_or_else(|| "工具输出".to_string());
        let k = key.to_string();
        let for_copy = text.clone();

        // 外层是列（卡片头一行、正文在下面），但卡片头自己包一层 `flex_row`：
        // 直接在列里放「一行东西」，它会被拉成整行宽，甲案那股「一小条」的味道就没了
        // （跟坑.md 19 同一个病）。
        div()
            .flex()
            .flex_col()
            .items_start()
            .px(px(12.))
            .py(px(4.))
            .child(
                div().flex().flex_row().child(
                    div()
                        .id(ElementId::Name(format!("tool-{key}").into()))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        // 不收宽度上限：长标题会被外层挤成一条缝。`max_w` 是给整条卡片的。
                        .max_w(px(520.))
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(rgb(0xe3e3e3))
                        .bg(rgb(0xffffff))
                        .cursor_pointer()
                        .hover(|el| el.bg(rgb(0xfafafa)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            let opened = this.open_tools.insert(k.clone());
                            if !opened {
                                this.open_tools.remove(&k);
                            }
                            log::info!("工具卡片 {k} -> {}", if opened { "展开" } else { "收起" });
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(if open { "▾" } else { "▸" }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .size(px(6.))
                                .rounded(px(3.))
                                .bg(rgb(if failed { 0xfa5151 } else { WECHAT_GREEN })),
                        )
                        .child(div().flex_none().text_size(px(12.5)).child(title))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(if failed && lines == 1 {
                                    "失败".to_string()
                                } else if failed {
                                    format!("失败 · {lines} 行")
                                } else {
                                    format!("{lines} 行 · {}", size_text(bytes))
                                }),
                        )
                        // 复制整段。代码块里划选后按 Ctrl+C 不一定会复制（键盘上下文分不到
                        // TextView 上），所以给一个明面出口 —— 长输出本来就是「整段拿走」。
                        .child(
                            div()
                                .id(ElementId::Name(format!("copy-{key}").into()))
                                .flex_none()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .text_size(px(11.))
                                .text_color(rgb(WECHAT_GREEN))
                                .cursor_pointer()
                                .hover(|el| el.bg(rgb(0xe8f7ee)))
                                .on_click(cx.listener(move |_this, _: &ClickEvent, _w, cx| {
                                    // 不拦的话这一下会连带把卡片展开/收起
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        for_copy.clone(),
                                    ));
                                    log::info!("复制了工具输出：{} 字", for_copy.len());
                                }))
                                .child("复制"),
                        ),
                ),
            )
            .when(open, |el| {
                el.child(
                    div()
                        .id(ElementId::Name(format!("tool-body-{key}").into()))
                        .mt(px(2.))
                        // 宽高都用**固定值**，不用 `max_*`：`max_*` 在「内容比它大」时扛不住，
                        // 一行 487 行的代码能直接铺满整个聊天区（gpui 里 `min-width:auto`
                        // 那一套，见坑.md 16）。
                        .w(px(560.))
                        .h(px(190.))
                        .overflow_y_scroll()
                        // 不折行：等宽代码一折行缩进就全乱了，宁可让它在块里横向滚
                        .whitespace_nowrap()
                        .px(px(10.))
                        .py(px(8.))
                        .border_l_2()
                        .border_color(rgb(0xcfcfcf))
                        .bg(rgb(0xfbfbfb))
                        .font_family("Consolas")
                        .text_size(px(11.5))
                        .text_color(rgb(0x3c3c3c))
                        .child(selectable_code(&format!("tb-{key}"), &text)),
                )
            })
            .into_any_element()
    }

    fn bubble(&self, e: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let text = e.text.clone().unwrap_or_default();
        match e.kind.as_str() {
            "pi.user" => div()
                .flex()
                .justify_end()
                .px(px(12.))
                .py(px(4.))
                .child(
                    div()
                        .max_w(px(480.))
                        .min_w(px(0.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(BUBBLE_ME))
                        .child(selectable(&format!("u-{}", e.id), &text, 0x1a1a1a)),
                )
                .into_any_element(),
            "pi.assistant" => div()
                .flex()
                .justify_start()
                .px(px(12.))
                .py(px(4.))
                .child(
                    div()
                        .max_w(px(480.))
                        .min_w(px(0.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(BUBBLE_OTHER))
                        .child(selectable(&format!("a-{}", e.id), &text, 0x1a1a1a)),
                )
                .into_any_element(),
            "pi.tool-result" => self.tool_card(&e.id.to_string(), None, &text, cx),
            _ => div()
                .flex()
                .justify_center()
                .px(px(12.))
                .py(px(4.))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(text))
                .into_any_element(),
        }
    }
}

/// 左栏顺序：**置顶的在前**（顺序照 `pins` 给的），其余保持原来那套顺序。
///
/// 纯函数（只吃 id），所以能直接拿数字做单测 —— 排序这种事写错一次能烦一星期。
fn order_ids(in_order: &[i64], pins: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::with_capacity(in_order.len());
    for id in pins {
        if in_order.contains(id) && !out.contains(id) {
            out.push(*id);
        }
    }
    for id in in_order {
        if !out.contains(id) {
            out.push(*id);
        }
    }
    out
}

/// 读配置拿「宿主地址 / 盘名」，供置顶那次 resolve 用。读不到就用默认值。
fn pins_host() -> Option<String> {
    Config::load().ok().map(|c| c.host)
}

fn pins_board() -> String {
    Config::load()
        .map(|c| c.board)
        .unwrap_or_else(|_| Config::default().board)
}

/// 错误原文可能很长（引擎把上游 400 的整段 JSON 递上来）。红条只给人看前 90 个字，
/// 全文在 `%LOCALAPPDATA%` 下的 `nousdesk/desktop.log` 里。
fn short_text(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if out.chars().count() < text.chars().count() {
        out.push('…');
    }
    out
}

/// 连引擎：拿配置 → resolve 地址 → 拉会话列表 + 设备信息。全在 tokio 上。
///
/// 配置**从调用方传进来**，不在这里再读一次盘：设置页刚保存完就重连，
/// 重新读盘会让「保存了什么」和「连了什么」有可能对不上。
async fn load_engine(
    cfg: Config,
) -> Result<(Arc<EngineClient>, Vec<Session>, Option<String>, String), String> {
    let url = resolve_engine(&cfg.host, &cfg.board, SELF_BLOCK, ENGINE_BLOCK)
        .await
        .map_err(|e| e.to_string())?;
    let engine =
        EngineClient::new(url.as_str(), Duration::from_secs(10)).map_err(|e| e.to_string())?;
    let sessions = engine.sessions().await.map_err(|e| e.to_string())?.items;
    let mut sessions = sessions;
    sort_sessions(&mut sessions);
    // 版本只是「关于」里的展示项，拿不到不妨碍连接。
    let node = engine.device().await.ok().and_then(|d| d.node);
    Ok((Arc::new(engine), sessions, node, url.as_str().to_string()))
}

/// 会话排序：按 `last.id` 倒序 = 最后说话的那条在最上面。
///
/// `EntryId` 是全局单调的（durable 的 `conversation-tails` 就靠这条），实测也成立：
/// session 465 → last.id 483、session 14 → 458、session 2 → 411。所以排序**不需要时间**。
///
/// 一条 entry 都没有的新会话没有 `last`，退化成按会话 id 排（两个序列都随时间增），
/// 保证刚建的那条落在最上面。
fn sort_sessions(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        activity_of(b)
            .cmp(&activity_of(a))
            .then_with(|| b.id.cmp(&a.id))
    });
}

/// 一行的「长相」。轮询时只比这个，不变就不 `cx.notify()`。
/// 活跃度：最后一条 entry 的 id。没有 entry 的新会话退化成会话 id。
pub(crate) fn activity_of(s: &Session) -> i64 {
    s.last.as_ref().and_then(|l| l.id.as_i64()).unwrap_or(s.id)
}

/// 一段**可选中**的文本。
///
/// GPUI 里 `div().child("…")` 画出来的是死的 StyledText，鼠标拖不动 —— 整个界面
/// 一个字都复制不出来就是这个原因。`TextView` 才有选区（拖选、双击选词、Ctrl+C）。
///
/// 真渲染 markdown：标题、粗体、列表、表格、代码块都按语法走。
/// 代码块带高亮 + 一个「复制」（见 `code_highlighter` / `code_copy_button`）。
fn selectable(id: &str, text: &str, color: u32) -> TextView {
    TextView::markdown(ElementId::Name(format!("t-{id}").into()), chat_md(text))
        .selectable(true)
        .text_size(px(13.5))
        .text_color(rgb(color))
        .style(md_style())
        .code_block_actions(code_copy_button)
        // 全局同一份 Arc：框架靠这个判断「这块代码没变，别重算高亮」
        .shared_code_block_highlighter(code_highlighter())
}

/// 一段**可选中**的等宽文本（工具输出、代码）。
///
/// 包成代码块：markdown 才不会把 `#`、`- `、`|` 这些当成标题/列表/表格去排版，
/// 空白也原样保留。围栏长度跟着内容走，输出里本身有 ``` 也不会把块切断。
fn selectable_code(id: &str, text: &str) -> TextView {
    TextView::markdown(
        ElementId::Name(format!("tc-{id}").into()),
        fenced_code(text),
    )
    .selectable(true)
    .text_size(px(11.5))
    .font_family("Consolas")
    .text_color(rgb(0x3c3c3c))
}

/// 包成代码块（围栏比内容里最长的那串反引号还长）。
fn fenced_code(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!(
        "{fence}
{text}
{fence}"
    )
}

/// 给人看的体积：`12 B` / `1.9 KB`。
fn size_text(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// 列表右上角那格时间。`updatedAt` 为 null 就返回 `None` —— **不编时间**
/// （引擎没记过就是没记过，拿文件 mtime 冒充是错的）。
///
/// 微信的规矩：今天给钟点、昨天给「昨天」、一周内给几天前、今年给月日、再往前给年月日。
pub(crate) fn when_text(updated_at: Option<i64>) -> Option<String> {
    use chrono::{Datelike as _, Local, TimeZone as _};
    let t = Local.timestamp_millis_opt(updated_at?).single()?;
    let now = Local::now();
    let days = (now.date_naive() - t.date_naive()).num_days();
    Some(match days {
        0 => t.format("%H:%M").to_string(),
        1 => "昨天".to_string(),
        d if (2..7).contains(&d) => format!("{} 天前", d),
        _ if t.year() == now.year() => format!("{}月{}日", t.month(), t.day()),
        _ => format!("{}/{}/{}", t.year(), t.month(), t.day()),
    })
}

fn sig_of(s: &Session) -> (i64, String, Option<i64>, Option<String>, bool) {
    (
        s.id,
        s.name.clone(),
        s.last.as_ref().and_then(|l| l.id.as_i64()),
        s.last.as_ref().and_then(|l| l.text.clone()),
        s.busy.is_some(),
    )
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 面板的过滤快照只在 render 里算一次：键盘 ↵ 和鼠标点都认这一份，
        // 不会「看到的」和「选中的」对不上。
        self.refresh_slash(cx);

        // New Manual **顶替**右侧聊天区，不再盖住整窗：左导航和会话列表永远在。
        // 尺寸基准也因此从整窗 vmin 换成右格 cqmin（见 `board_art::M`）。
        // 设置页也顶替右栏，但它优先级更高：正在改连接时别把盘面盖在上面。
        let right: AnyElement = if self.settings_open {
            self.render_settings(cx)
        } else if self.newchat.open {
            self.render_board(window, cx)
        } else {
            self.render_chat(cx).into_any_element()
        };

        let shell_root = div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(CHAT_BG))
            .child(self.render_icon_bar(cx))
            .child(match self.tab {
                Tab::Chats => self.render_session_list(cx).into_any_element(),
                // 通讯录是**两栏**：类别树 + 预设列表。跟聊天页那一栏不是一个东西。
                Tab::Templates => div()
                    .flex()
                    .flex_row()
                    .h_full()
                    .min_w(px(0.))
                    .child(self.render_preset_tree(cx))
                    .child(self.render_templates(cx))
                    .into_any_element(),
            })
            // 会话内跳转目录（J）：只在聊天页、选中了会话、且**盘面没开**时出现。
            // New Manual 顶替右栏时那一列会把它挤窄，所以一并让开。
            .children(
                (self.tab == Tab::Chats
                    && self.selected.is_some()
                    && !self.newchat.open
                    && !self.settings_open)
                    .then(|| {
                        let groups = self.nav_groups();
                        if groups.is_empty() {
                            return div().into_any_element();
                        }
                        let from = self.nav.from_of(self.entries.len());
                        crate::nav_ui::nav_column(
                            &self.entries,
                            &groups,
                            &self.nav,
                            from,
                            self.nav.drawer_open,
                            cx,
                        )
                    },
                ),
            )
            // 别人的 New Manual 盘面顶替右栏；没开时才是聊天区。
            .child(right);

        // 配置面板挂在**整窗**这一层，不塞进输入区：塞进去它是 absolute 浮层，
        // 超出输入区那 130px 的部分 hitbox 会被裁掉 —— 画得出来、hover 也变色，
        // 但 on_click 一次都进不来（踩过）。
        div()
            .relative()
            .size_full()
            .child(shell_root)
            .children(self.cfg_overlay(cx))
            // 「≡ 更多」菜单压在最上面（它自带一层点空白收起的罩子）。
            .children(self.menu_open.then(|| self.render_menu_overlay(cx)))
    }
}

/// 空正文的气泡不画 —— 引擎对「有 entry 没内容」会给空串/null，
/// 画出来就是一个白方块（坑.md 第 7 条）。
/// `nav_spy` 和 `render_chat` 共用这一条，保证「滚动容器里的第几个孩子」算得对。
fn entry_visible(e: &Entry) -> bool {
    e.text
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
}

/// 第 `entry_index` 条历史在滚动容器里是第几个孩子。
///
/// 空正文被跳过，主视图前面有「更早」+ 说明两行，抽屉前面有一行提示。
/// 纯函数（只吃 entries 和几个开关）—— 「目录点一下跳歪」这条最容易错，
/// 所以拿数字直接钉住单测，不靠肉眼对齐。
fn nav_child_index(
    entries: &[Entry],
    entry_index: usize,
    from: usize,
    drawer: bool,
    loading: bool,
) -> Option<usize> {
    if entry_index < from || entry_index >= entries.len() || !entry_visible(&entries[entry_index]) {
        return None;
    }
    let mut n = if drawer {
        1 // 抽屉里的提示行
    } else {
        let mut head = 0;
        if from > 0 && !loading && !entries.is_empty() {
            head += 2; // 「更早」条 + 说明
        }
        if loading || entries.is_empty() {
            head += 1; // 状态行
        }
        head
    };
    for e in &entries[from..entry_index] {
        if entry_visible(e) {
            n += 1;
        }
    }
    Some(n)
}

// 排序写错一次能烦一星期，所以拿数字直接钉住。
#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::{nav_child_index, order_ids, when_text};
    use nous_engine::types::Entry;
    use serde_json::json;
    use std::collections::HashMap;

    fn e(id: i64, kind: &str, text: Option<&str>) -> Entry {
        Entry {
            id: json!(id),
            kind: kind.into(),
            text: text.map(str::to_string),
            parent: None,
            extra: HashMap::new(),
        }
    }

    /// 目录点一行，跳的必须是那一行 —— 空正文气泡被跳过 + 前面几行装饰，
    /// 拿 entry 下标直接当孩子下标会跳歪。
    #[test]
    fn 目录跳转的孩子下标要跳过空气泡和装饰行() {
        let es = vec![
            e(1, "pi.user", Some("第一句")),
            e(2, "pi.assistant", None), // 空正文：不画
            e(3, "pi.user", Some("第二句")),
            e(4, "pi.assistant", Some("回答")),
            e(5, "pi.user", Some("第三句")),
        ];
        // 抽屉：前面一行提示，空气泡跳过。
        assert_eq!(nav_child_index(&es, 0, 0, true, false), Some(1));
        assert_eq!(
            nav_child_index(&es, 1, 0, true, false),
            None,
            "空气泡没画出来"
        );
        assert_eq!(nav_child_index(&es, 2, 0, true, false), Some(2));
        assert_eq!(nav_child_index(&es, 4, 0, true, false), Some(4));
        // 主视图 from=3：前面「更早」+ 说明两行，从 3 开始数。
        assert_eq!(nav_child_index(&es, 4, 3, false, false), Some(3));
        assert_eq!(
            nav_child_index(&es, 2, 3, false, false),
            None,
            "不在主视图里"
        );
        // 拉历史中：只有一行状态，没有「更早」。
        assert_eq!(nav_child_index(&es, 4, 0, false, true), Some(4));
        assert_eq!(
            nav_child_index(&es, 2, 3, false, false),
            None,
            "不在主视图里"
        );
    }

    /// 时间列的三档：今天给钟点、昨天给「昨天」、今年更早给月日。
    /// 都拿「现在」往前推，不写死日期 —— 写死的话过一天就红。
    #[test]
    fn when_text_三档() {
        let now = chrono::Local::now();
        let today = now.timestamp_millis();
        let hhmm = when_text(Some(today)).unwrap();
        assert_eq!(hhmm.len(), 5, "今天的格式是 HH:MM，拿到 {hhmm:?}");

        let y = (now - chrono::Duration::days(1)).timestamp_millis();
        assert_eq!(when_text(Some(y)).unwrap(), "昨天");

        let five = (now - chrono::Duration::days(5)).timestamp_millis();
        assert_eq!(when_text(Some(five)).unwrap(), "5 天前");
    }

    /// `updatedAt == null` 必须给 `None`：引擎没记过就是没记过，
    /// 这格空着才对，不许拿别的时间填。
    #[test]
    fn when_text_没时间就是空() {
        assert!(when_text(None).is_none());
    }

    #[test]
    fn 工具输出剥掉_harness_壳() {
        use crate::md::strip_harness;
        assert_eq!(
            strip_harness(
                "<harness>
[error] boom
</harness>"
            ),
            "[error] boom"
        );
        assert_eq!(
            strip_harness(
                "j1900
/root"
            ),
            "j1900
/root"
        );
        // 只有一边的壳也剥（引擎写半截的时候别把它当正文）
        assert_eq!(strip_harness("<harness>ok"), "ok");
    }

    #[test]
    fn 体积给人看() {
        use super::size_text;
        assert_eq!(size_text(12), "12 B");
        assert_eq!(size_text(2048), "2.0 KB");
    }

    #[test]
    fn 单换行补成硬换行() {
        use crate::md::hard_breaks;
        assert_eq!(
            hard_breaks(
                "一
二"
            ),
            "一  
二"
        );
        assert_eq!(
            hard_breaks(
                "一

二"
            ),
            "一  

二"
        );
        // 代码块里的换行一个都不能碰（会改成代码内容）
        assert_eq!(
            hard_breaks(
                "```
a
b
```"
            ),
            "```
a
b
```"
        );
        // 空行不补（补了就是三个空格挂在那）
        assert_eq!(hard_breaks(""), "");
    }

    #[test]
    fn 代码围栏跟着内容变长() {
        use super::fenced_code;
        assert_eq!(
            fenced_code("ls"),
            "```
ls
```"
        );
        // 输出里本身有三连反引号：围栏要更长，不然块会被提前切断
        assert_eq!(
            fenced_code(
                "```
x"
            ),
            "````
```
x
````"
        );
        assert_eq!(
            fenced_code(
                "```
x"
            ),
            r"````
```
x
````"
        );
    }

    #[test]
    fn 置顶的排前面_且按置顶顺序() {
        // 原顺序 3,2,1；置顶顺序 1,3（新置顶的在最前）
        assert_eq!(order_ids(&[3, 2, 1], &[1, 3]), vec![1, 3, 2]);
    }

    #[test]
    fn 置顶里已经不存在的会话被忽略() {
        assert_eq!(order_ids(&[5, 4], &[99, 4]), vec![4, 5]);
    }

    #[test]
    fn 没有置顶就保持原顺序() {
        assert_eq!(order_ids(&[3, 2, 1], &[]), vec![3, 2, 1]);
    }

    #[test]
    fn 重复置顶不会重复出现() {
        assert_eq!(order_ids(&[1, 2], &[2, 2]), vec![2, 1]);
    }
}
