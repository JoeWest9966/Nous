//! 应用主体。三栏：图标条 / 会话列表 / 聊天区。
//!
//! 微信桌面版的骨头。聊天区的流式（SSE）还没接，先做历史气泡。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use smol::Timer;
use nous_engine::resolve::resolve_engine;
use nous_engine::types::{Entry, Session};
use nous_engine::EngineClient;

use crate::config::{Config, ENGINE_BLOCK, SELF_BLOCK};
use crate::live::Live;

// 微信桌面版的几个固定色。
const SIDEBAR_DARK: u32 = 0x2e2e2e;
const LIST_BG: u32 = 0xf7f7f7;
const CHAT_BG: u32 = 0xededed;
const BUBBLE_ME: u32 = 0x95ec69;
const BUBBLE_OTHER: u32 = 0xffffff;
const MUTED: u32 = 0x999999;

pub struct Desktop {
    engine: Option<Arc<EngineClient>>,
    sessions: Vec<Session>,
    selected: Option<i64>,
    entries: Vec<Entry>,
    entries_loading: bool,
    entries_error: Option<String>,
    error: Option<String>,
    loading: bool,
    input: Entity<InputState>,
    /// 正在生成的那条助手消息（流式）。
    live: Option<String>,
    /// 共享给 tokio 任务的流式状态。
    live_state: Live,
    /// 有没有一条流正在跑，防止重复点火。
    streaming: bool,
    /// 聊天区的滚动位置：新消息进来要跟着到底。
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl Desktop {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("输入消息，回车发送"));
        let subs = vec![cx.subscribe_in(&input, window, {
            move |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    if !*shift {
                        this.send(window, cx);
                    }
                }
            }
        })];

        let mut this = Self {
            engine: None,
            sessions: vec![],
            selected: None,
            entries: vec![],
            entries_loading: false,
            entries_error: None,
            error: None,
            loading: true,
            input,
            live: None,
            live_state: Live::new(),
            streaming: false,
            scroll: ScrollHandle::new(),
            _subs: subs,
        };
        this.bootstrap(cx);
        this
    }

    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let outcome = match handle {
                Some(h) => match h.spawn(load_engine()).await {
                    Ok(Ok(v)) => Ok(v),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(format!("任务挂了：{e}")),
                },
                None => Err("tokio 没起".to_string()),
            };

            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok((engine, sessions)) => {
                        this.engine = Some(engine);
                        this.sessions = sessions;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_session(&mut self, id: i64, force: bool, cx: &mut Context<Self>) {
        if self.selected == Some(id) && !force {
            return;
        }
        self.selected = Some(id);
        self.entries.clear();
        self.entries_error = None;
        self.entries_loading = true;
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


    /// 发一条。SSE 走的是 `requestId` 幂等锚：重发不会答两遍。
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let Some(engine) = self.engine.clone() else { return };
        let text = self.input.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
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
        self.live_state = Live::new();
        self.streaming = true;
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
    }

    /// 连 SSE。tokio 任务只往 `live_state` 里塞，UI 侧 16ms 取一次。
    fn start_stream(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let Some(engine) = self.engine.clone() else { return };
        let live = self.live_state.clone();

        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let Some(h) = handle else { return };
            h.spawn(async move {
                match engine.events(id).await {
                    Ok(mut stream) => {
                        use futures_util::StreamExt as _;
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(event) => live.ingest(&event),
                                Err(e) => {
                                    live.fail(e.to_string());
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => live.fail(e.to_string()),
                }
            })
            .await
            .ok();
            let _ = this.update(cx, |_this, cx| cx.notify());
        })
        .detach();

        // UI 侧 16ms 一帧：把攒下的字写进气泡。一个 token 一次 notify 就是在这里被挡住的。
        let live = self.live_state.clone();
        cx.spawn(async move |this, cx| loop {
            Timer::after(std::time::Duration::from_millis(16)).await;
            let drained = live.drain();
            let mut reload = None;
            let stop = this
                .update(cx, |this, cx| {
                    let Some(drained) = drained else {
                        return this.streaming;
                    };
                    if let Some(err) = drained.error {
                        this.error = Some(format!("生成失败：{err}"));
                        this.live = None;
                        this.streaming = false;
                        cx.notify();
                        return false;
                    }
                    this.live = Some(drained.text);
                    // 新内容进来 / 收尾，都跟着滚到底。
                    this.scroll.scroll_to_bottom();
                    if drained.done {
                        // 收尾：把那口气泡交出去，历史以引擎为准重拉一遍。
                        this.streaming = false;
                        if let Some(id) = this.selected {
                            reload = Some(id);
                        }
                        cx.notify();
                        return false;
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !stop {
                if let Some(id) = reload {
                    // 流结束之后 live 就该消失，历史里已经有这条了。
                    let _ = this.update(cx, |this, cx| {
                        this.live = None;
                        this.select_session(id, true, cx);
                    });
                }
                break;
            }
        })
        .detach();
    }

    // ── 三个栏 ──────────────────────────────────────────────

    fn render_icon_bar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        // 微信桌面版最左的深色窄条。图标还没画，先放占位。
        div()
            .flex()
            .flex_col()
            .w(px(60.))
            .h_full()
            .bg(rgb(SIDEBAR_DARK))
            .items_center()
            .py(px(8.))
            .child(div().size(px(32.)).rounded(px(4.)).bg(rgb(0x444444)))
    }

    fn render_session_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = self
            .sessions
            .iter()
            .map(|s| self.session_row(s, cx).into_any_element())
            .collect();
        div()
            .flex()
            .flex_col()
            .w(px(280.))
            .h_full()
            .bg(rgb(LIST_BG))
            .child(div().p(px(12.)).child("Nous"))
            .child(div().px(px(12.)).child(self.status_line(cx)))
            .child(div().flex().flex_col().flex_1().overflow_hidden().children(rows))
    }

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

        let bubbles: Vec<AnyElement> = if self.entries_loading {
            vec![div().p(px(12.)).text_size(px(12.)).child("拉历史中…").into_any_element()]
        } else if let Some(e) = &self.entries_error {
            vec![div()
                .p(px(12.))
                .text_size(px(12.))
                .text_color(rgb(0xd32f2f))
                .child(format!("拉历史失败：{e}"))
                .into_any_element()]
        } else if self.entries.is_empty() {
            vec![div()
                .p(px(12.))
                .text_size(px(12.))
                .child("还没消息，先从模板开一条。")
                .into_any_element()]
        } else {
            let mut out: Vec<AnyElement> = self
                .entries
                .iter()
                // 空正文的气泡不画 —— 引擎对「有 entry 没内容」会给空串/null，
                // 画出来就是一个白方块（坑.md 第 7 条）。
                .filter(|e| {
                    e.text
                        .as_deref()
                        .map(|t| !t.trim().is_empty())
                        .unwrap_or(false)
                })
                .map(|e| self.bubble(e, cx).into_any_element())
                .collect();
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
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .bg(rgb(CHAT_BG))
            .child(
                div()
                    .flex()
                    .h(px(48.))
                    .w_full()
                    .items_center()
                    .px(px(12.))
                    .bg(rgb(LIST_BG))
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .id("chat-scroll")
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .py(px(8.))
                    .children(bubbles),
            )
            .child(
                div()
                    .flex()
                    .h(px(72.))
                    .w_full()
                    .items_center()
                    .px(px(12.))
                    .bg(rgb(LIST_BG))
                    .child(div().flex_1().child(Input::new(&self.input))),
            )
            .into_any_element()
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
        div()
            .text_size(px(12.))
            .child(format!("{} 个会话", self.sessions.len()))
    }

    fn session_row(&self, s: &Session, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = s
            .last
            .as_ref()
            .and_then(|l| l.text.clone())
            .unwrap_or_default();
        let selected = self.selected == Some(s.id);
        let id = s.id;

        div()
            .flex()
            .flex_col()
            .p(px(8.))
            .gap(px(2.))
            .border_b(px(1.))
            .border_color(rgb(0xe0e0e0))
            .when(selected, |el| el.bg(rgb(0xd9d9d9)))
            .child(div().child(s.name.clone()))
            .when(!preview.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(preview),
                )
            })
            // `.id()` 把 Div 转成 Stateful<Div>，on_click 只存在于后者。
            .id(ElementId::Name(format!("sess-{id}").into()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.select_session(id, false, cx);
            }))
    }

    fn bubble(&self, e: &Entry, _cx: &mut Context<Self>) -> impl IntoElement {
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
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(BUBBLE_ME))
                        .child(text),
                ),
            "pi.assistant" => div()
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
                        .child(text),
                ),
            _ => div()
                .flex()
                .justify_center()
                .px(px(12.))
                .py(px(4.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(text),
                ),
        }
    }
}

/// 连引擎：读配置 → resolve 地址 → 拉会话列表。全在 tokio 上。
async fn load_engine() -> Result<(Arc<EngineClient>, Vec<Session>), String> {
    let cfg = Config::load()?;
    let url = resolve_engine(&cfg.host, &cfg.board, SELF_BLOCK, ENGINE_BLOCK)
        .await
        .map_err(|e| e.to_string())?;
    let engine = EngineClient::new(url.as_str(), Duration::from_secs(10)).map_err(|e| e.to_string())?;
    let sessions = engine
        .sessions()
        .await
        .map_err(|e| e.to_string())?
        .items;
    // 引擎按 id 升序给，聊天应用要新的在上。
    let mut sessions = sessions;
    sessions.sort_by(|a, b| b.id.cmp(&a.id));
    Ok((Arc::new(engine), sessions))
}

impl Render for Desktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(CHAT_BG))
            .child(self.render_icon_bar(cx))
            .child(self.render_session_list(cx))
            .child(self.render_chat(cx))
    }
}
