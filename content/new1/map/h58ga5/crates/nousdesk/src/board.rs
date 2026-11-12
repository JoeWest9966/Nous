//! 「New Manual」盘面。
//!
//! 外观照抄 qrd3wh 的面板页 `/p/<board>/hb7k3j/`（Pele / Flexoki dark）：
//! 四块象限 + 中间一颗虚线椭圆，模型 / 上下文 / 扩展 / 权限各占一角，
//! 建会话的参数全在椭圆里，点哪块开哪块的弹层。
//!
//! **只占右侧聊天区那一格**，不再盖住整窗：左导航和会话列表永远在，
//! 点某个会话 / Esc 就回到聊天。尺寸基准因此从整窗 vmin 换成右格 cqmin。
//!
//! 三条纪律（和页面对齐，不是我自己编的）：
//! - **一屏不滚**：尺寸全走右格 cqmin，右格变了跟着缩，盘面永远填满右格。
//! - **形状说语义**：模型是「供应商 › 模型 › 思考」三段语法链，画成铁路图；
//!   技能 / 模板 / 目录是平铺清单，因为它们不是链（一个是集合、一个是单值、
//!   一个是导航）。把后者硬套成语法图就是把语义搞错。
//! - **单选不关窗、多选也不关窗**：模型点一下就收（它是单值、也没有别的要看）；
//!   技能要勾好几下，勾完按「完成」—— 在「点了就关」的窗口里做多选是做不了的事。

use std::collections::BTreeSet;

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::prelude::StatefulInteractiveElement as _;
use gpui_kit::*;

use nous_engine::EngineError;
use nous_engine::types::{
    CreateSession, CreatedSession, Dirs, Machines, Model, Models, Prompt, Prompts, Skill, Skills,
};

use crate::app::Desktop;
use crate::board_art as art;

/// 打开着哪一层的弹层。`None` = 就在盘面上。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sheet {
    Model,
    Skills,
    Prompt,
    Dir,
    Machine,
}

/// 盘面的状态。**选择是本地草稿**：这一页还没落库，一切都在内存里，
/// 按 Create 才变成一次 `POST /v1/sessions`。
pub(crate) struct NewChat {
    pub(crate) open: bool,
    focus: FocusHandle,
    name_input: Entity<TextareaState>,
    /// 五份清单，各自独立成功/失败 —— 一份挂了不该把别的拖着一起灰。
    models: Option<Models>,
    skills: Option<Skills>,
    prompts: Option<Prompts>,
    machines: Option<Machines>,
    dirs: Option<Dirs>,
    loading: bool,
    /// 这一页自己的错误带（在椭圆底下那行 `flash` 里显示，不弹窗）。
    load_err: Option<String>,
    flash: Option<String>,

    // ── 选择 ──
    /// `(provider, modelId)`。建会话时原样发回这个对象。
    picked: Option<(String, String)>,
    /// 思考档，`off` … `max`。换模型要按模型的 `thinkingLevels` 重新夹。
    level: String,
    picked_skills: BTreeSet<String>,
    /// `None` = 明确不要模板（会以 `template: null` 发出去）。
    template: Option<String>,
    /// `None` = 本机。
    machine: Option<String>,
    dir: Option<String>,
    /// 目录浏览到哪儿（和 `dir` 是两件事：看的地方和选的地方）。
    dir_at: Option<String>,

    pub(crate) sheet: Option<Sheet>,
    sheet_scroll: ScrollHandle,
    creating: bool,
}

impl NewChat {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Desktop>) -> Self {
        let name_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1)
                .placeholder("Name · 空着就用引擎给的名字")
        });
        Self {
            open: false,
            focus: cx.focus_handle(),
            name_input,
            models: None,
            skills: None,
            prompts: None,
            machines: None,
            dirs: None,
            loading: false,
            load_err: None,
            flash: None,
            picked: None,
            level: "off".into(),
            picked_skills: BTreeSet::new(),
            template: None,
            machine: None,
            dir: None,
            dir_at: None,
            sheet: None,
            sheet_scroll: ScrollHandle::new(),
            creating: false,
        }
    }

    /// 当前选中的模型（`provider/modelId`），给人看的那种写法。
    fn picked_label(&self) -> Option<String> {
        self.picked.as_ref().map(|(p, m)| format!("{p}/{m}"))
    }

    fn model(&self) -> Option<&Model> {
        let (p, m) = self.picked.as_ref()?;
        self.models
            .as_ref()?
            .items
            .iter()
            .find(|x| &x.provider == p && &x.model_id == m)
    }

    /// 这个模型支持的思考档（从少想到多想）。不支持思考档的给一个 `off`。
    fn levels(&self) -> Vec<String> {
        match self.model() {
            Some(m) => m.levels(),
            None => vec!["off".into()],
        }
    }

    /// 换模型后把档位夹回新模型的表里 —— 和 `pi-ai` 的 `clampThinkingLevel` 同序：
    /// 先往上找，找不到再往下。面板上看到的就是库里会落的值。
    fn clamp_level(&mut self) {
        let levels = self.levels();
        if levels.iter().any(|l| l == &self.level) {
            return;
        }
        let all = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
        let at = all.iter().position(|x| x == &self.level).unwrap_or(0);
        for cand in all.iter().skip(at) {
            if levels.iter().any(|l| l == cand) {
                self.level = (*cand).to_string();
                return;
            }
        }
        for cand in all.iter().take(at).rev() {
            if levels.iter().any(|l| l == cand) {
                self.level = (*cand).to_string();
                return;
            }
        }
        self.level = levels.first().cloned().unwrap_or_else(|| "off".into());
    }

    /// 能勾的技能：加载得起来、也不是 `disable-model-invocation` 那种。
    fn skill_list(&self) -> Vec<&Skill> {
        let mut out: Vec<&Skill> = self
            .skills
            .as_ref()
            .map(|s| {
                s.skills
                    .iter()
                    .filter(|x| x.loadable() && !x.hidden)
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn prompt_list(&self) -> Vec<&Prompt> {
        let mut out: Vec<&Prompt> = self
            .prompts
            .as_ref()
            .map(|p| p.prompts.iter().filter(|x| x.loadable()).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// 椭圆里那行状态。**只说现在差什么**，不说欢迎语。
    fn hub_sub(&self) -> String {
        if self.load_err.is_some() {
            return "Disorder · 拿不到清单".into();
        }
        if !self.open {
            return String::new();
        }
        if self.models.is_none() {
            return if self.loading {
                "Disorder · loading".into()
            } else {
                "Disorder · no models".into()
            };
        }
        match self.picked_label() {
            Some(m) => {
                let mut s = format!("Disorder · ready · {m}");
                if let Some(machine) = &self.machine {
                    s.push_str(&format!(" · {machine}"));
                }
                if let Some(dir) = &self.dir {
                    s.push_str(&format!(" · {}", tail_path(dir)));
                }
                s
            }
            None => "Disorder · pick a model".into(),
        }
    }
}

/// 路径尾巴：`/a/b/c` → `c`。椭圆里放不下整串。
fn tail_path(path: &str) -> String {
    let p = path.trim_end_matches('/');
    p.rsplit('/').next().unwrap_or(p).to_string()
}

/// 思考档给人看的名字。`off` 在面板上就是「不思考」，不填就是不思考 —— 不撒谎。
fn level_label(level: &str) -> &'static str {
    match level {
        "off" => "不思考",
        "minimal" => "极少",
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        "xhigh" => "很高",
        "max" => "最大",
        _ => "不思考",
    }
}

// ── 动作（全部挂在 `Desktop` 上：状态在这一层，盘面只是它的一块渲染）──────────

impl Desktop {
    pub(crate) fn board_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.newchat.open = true;
        self.newchat.sheet = None;
        self.newchat.flash = None;
        window.focus(&self.newchat.focus, cx);
        cx.notify();
        if self.newchat.models.is_none() && !self.newchat.loading {
            self.board_load(cx);
        }
    }

    pub(crate) fn board_close(&mut self, cx: &mut Context<Self>) {
        self.newchat.open = false;
        self.newchat.sheet = None;
        self.newchat.flash = None;
        cx.notify();
    }

    /// 五份清单一起拉，但**各自吃自己的错**：技能仓库挂了，模型照样能选。
    pub(crate) fn board_load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            self.newchat.load_err = Some("还没连上引擎".into());
            cx.notify();
            return;
        };
        self.newchat.loading = true;
        self.newchat.load_err = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let Some(h) = handle else { return };
            type Got<T> = Result<T, EngineError>;
            let (models, skills, prompts, machines, dirs) = h
                .spawn(async move {
                    let models: Got<Models> = engine.models().await;
                    let skills: Got<Skills> = engine.skills().await;
                    let prompts: Got<Prompts> = engine.prompts().await;
                    let machines: Got<Machines> = engine.machines().await;
                    let dirs: Got<Dirs> = engine.dirs(None).await;
                    (models, skills, prompts, machines, dirs)
                })
                .await
                .unwrap_or_else(|_| {
                    (
                        Err(EngineError::Engine {
                            status: 0,
                            message: "任务没跑完".into(),
                        }),
                        Err(EngineError::Engine {
                            status: 0,
                            message: "任务没跑完".into(),
                        }),
                        Err(EngineError::Engine {
                            status: 0,
                            message: "任务没跑完".into(),
                        }),
                        Err(EngineError::Engine {
                            status: 0,
                            message: "任务没跑完".into(),
                        }),
                        Err(EngineError::Engine {
                            status: 0,
                            message: "任务没跑完".into(),
                        }),
                    )
                });

            let _ = this.update(cx, |this, cx| {
                let b = &mut this.newchat;
                b.loading = false;
                if let Ok(m) = models {
                    // 默认模型：盘上那只眼指着的那个；没有就第一个。
                    b.picked = m
                        .default
                        .as_ref()
                        .map(|d| (d.provider.clone(), d.model_id.clone()));
                    if b.picked.is_none() {
                        b.picked = m
                            .items
                            .first()
                            .map(|x| (x.provider.clone(), x.model_id.clone()));
                    }
                    b.models = Some(m);
                    b.clamp_level();
                } else if let Err(e) = models {
                    b.load_err = Some(format!("模型清单：{e}"));
                }
                if let Ok(s) = skills {
                    b.skills = Some(s);
                }
                if let Ok(p) = prompts {
                    // 仓库默认值只用来预勾；选不选仍然是人的事。
                    b.template = p.default.clone();
                    b.prompts = Some(p);
                }
                if let Ok(mm) = machines {
                    b.machines = Some(mm);
                }
                if let Ok(d) = dirs {
                    b.dir_at = d.path.clone();
                    b.dirs = Some(d);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 点目录里的某一层：进去看一眼，**顺手把它当成选择**（省一次点击）。
    fn board_dir_enter(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.newchat.dir_at = Some(path.clone());
        self.newchat.dir = Some(path.clone());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let Some(h) = handle else { return };
            let got = h
                .spawn(async move { engine.dirs(Some(&path)).await })
                .await
                .ok();
            let _ = this.update(cx, |this, cx| {
                match got {
                    Some(Ok(d)) => {
                        this.newchat.dir_at = d.path.clone();
                        this.newchat.dirs = Some(d);
                    }
                    Some(Err(e)) => this.newchat.flash = Some(format!("列目录失败：{e}")),
                    None => this.newchat.flash = Some("列目录失败了".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Create：一次发完（模型 / 档位 / 技能 / 模板 / 目录 / 机器 / 名字），
    /// 回来再把盘上那只眼切过去，然后回列表、选中它。
    fn board_create(&mut self, cx: &mut Context<Self>) {
        if self.newchat.creating {
            return;
        }
        let Some((provider, model_id)) = self.newchat.picked.clone() else {
            return;
        };
        let Some(engine) = self.engine.clone() else {
            self.newchat.flash = Some("还没连上引擎".into());
            cx.notify();
            return;
        };

        let name = self.newchat.name_input.read(cx).value().to_string();
        let name = name.trim().to_string();
        let body = CreateSession {
            name: (!name.is_empty()).then_some(name),
            model: Some(serde_json::json!({
                "provider": provider,
                "modelId": model_id,
            })),
            thinking_level: Some(serde_json::json!(self.newchat.level)),
            skills: (!self.newchat.picked_skills.is_empty())
                .then(|| self.newchat.picked_skills.iter().cloned().collect()),
            // **总是发**：`Some(None)` 明确就是「不要模板」。
            template: Some(self.newchat.template.clone()),
            cwd: self.newchat.dir.clone(),
            machine: self.newchat.machine.clone(),
            ..Default::default()
        };

        self.newchat.creating = true;
        self.newchat.flash = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let handle = crate::TOKIO.get().cloned();
            let Some(h) = handle else { return };
            let outcome = h
                .spawn(async move {
                    let created: CreatedSession = engine.create(&body).await?;
                    // 创建成功之后才切那只眼；切不过去不该把这个会话判死。
                    let _ = engine.set_selection(Some(created.id)).await;
                    let sessions = engine.sessions().await?.items;
                    Ok::<_, EngineError>((created, sessions))
                })
                .await
                .ok();

            let _ = this.update(cx, |this, cx| {
                this.newchat.creating = false;
                match outcome {
                    Some(Ok((created, sessions))) => {
                        let mut sessions = sessions;
                        sessions.sort_by(|a, b| b.id.cmp(&a.id));
                        this.sessions = sessions;
                        this.newchat.open = false;
                        this.newchat.sheet = None;
                        this.tab = crate::app::Tab::Chats;
                        this.newchat.flash = None;
                        this.select_session(created.id, true, cx);
                    }
                    Some(Err(e)) => this.newchat.flash = Some(format!("创建失败：{e}")),
                    None => this.newchat.flash = Some("创建失败了".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── 渲染 ──────────────────────────────────────────────────────────

    /// 整块盘面。**顶替**右侧聊天区那一格（`app.rs` 的 render 里换掉 `render_chat`）。
    pub(crate) fn render_board(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 基准是**右格**不是整窗：盘面现在是三栏里的一格。
        let m = art::M::new(window.viewport_size(), self.tab);
        let b = &self.newchat;

        let body = div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .px(m.v(1.6))
            .pb(m.v(1.6))
            .child(self.board_canvas(&m))
            .child(self.board_regions(&m, cx))
            .child(self.board_hub(&m, cx));

        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            // flex 项不写 min_w(0)、h_full 会被内容撑到比右格还宽（坑 16）。
            .min_w(px(0.))
            .h_full()
            .bg(rgb(art::BG))
            .text_color(rgb(art::FG))
            .text_size(m.f(1.75, 12., 16.))
            .line_height(relative(1.5))
            .track_focus(&b.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    if this.newchat.sheet.is_some() {
                        this.newchat.sheet = None;
                    } else {
                        this.board_close(cx);
                        return;
                    }
                    cx.notify();
                }
            }))
            // 没有顶栏：右格自己没有标题行，和 HTML demo 一致。
            // 出口是 Esc、以及左栏点任意一个会话。
            .child(body)
            .children(self.board_sheet(&m, cx))
            .into_any_element()
    }

    /// 那张图：四块象限 + 三条虚线 + 一道崖。
    ///
    /// 用 `canvas` 直画路径，**不用 `svg()` 元素** —— 那个只走 alpha 蒙版、一个颜色，
    /// 四块色会糊成一块。图本来就是矢量，这里原样把 `d` 摊成 `PathBuilder`。
    fn board_canvas(&self, _m: &art::M) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(rgb(art::PANEL))
            .child(
                canvas(
                    move |bounds, _, _| art::shapes(bounds),
                    |_bounds, shapes, window, _| {
                        for shape in shapes {
                            window.paint_path(shape.path, shape.color);
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// 四块象限。位置和页面一一对应：r1 左上、r2 右上、r4 左下、r3 右下。
    fn board_regions(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        let muted = rgb(art::MUTED);

        let h2 = |text: &'static str| {
            div()
                .text_size(m.f(1.85, 12., 17.))
                .font_weight(FontWeight::BOLD)
                .child(text)
        };
        let sub = move |text: String| {
            div()
                .italic()
                .text_size(m.f(1.5, 10., 13.))
                .text_color(muted)
                .child(text)
        };

        // 模型那格：`volc-ark/deepseek-v4-flash` + `change ›`
        let model_id = b.picked_label().unwrap_or_else(|| "—".into());
        let model_node = self.board_node(
            "board-model",
            &model_id,
            "change ›",
            b.models.is_some(),
            m,
            cx,
            |this, _cx| this.newchat.sheet = Some(Sheet::Model),
        );

        // 上下文那格：两个小框，不是一个 —— 技能是集合（多选），模板是单值（单选）。
        let skills_count = b.picked_skills.len();
        let skills_total = b.skill_list().len();
        let prompt_label = b.template.clone().unwrap_or_else(|| "none".into());
        let skills_node = self.board_node(
            "board-skills",
            "Skills",
            &format!("{skills_count}/{skills_total} ›"),
            true,
            m,
            cx,
            |this, _cx| this.newchat.sheet = Some(Sheet::Skills),
        );
        let prompt_node = self.board_node(
            "board-prompt",
            "Prompt",
            &format!("{prompt_label} ›"),
            true,
            m,
            cx,
            |this, _cx| this.newchat.sheet = Some(Sheet::Prompt),
        );

        // 扩展那格：qrd3wh 没有这个端点（数据在 vbe4pt 块），先如实说。
        let ext_node = self.board_node(
            "board-ext",
            "none",
            "choose ›",
            true,
            m,
            cx,
            |this, cx| {
                this.newchat.flash = Some("扩展来自扩展块（vbe4pt），这一版没接".into());
                cx.notify();
            },
        );

        let region = |x: f32, y: f32, kids: Vec<AnyElement>, justify_top: bool| {
            div()
                .absolute()
                .left(relative(x))
                .top(relative(y))
                .w(relative(0.5))
                .h(relative(0.5))
                .flex()
                .flex_col()
                .items_center()
                .text_center()
                .pt(m.v(5.5))
                .px(m.v(3.))
                .children(kids)
                .when(!justify_top, |el| el.child(div().flex_1()))
                .into_any_element()
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(region(
                0.,
                0.,
                vec![
                    h2("Model").into_any_element(),
                    sub(b
                        .models
                        .as_ref()
                        .map(|x| format!("{} 个供应商 · {} 个模型", providers_of(x), x.items.len()))
                        .unwrap_or_else(|| "available".into()))
                    .into_any_element(),
                    div().mt(m.v(1.8)).child(model_node).into_any_element(),
                ],
                true,
            ))
            .child(region(
                0.5,
                0.,
                vec![
                    h2("Context").into_any_element(),
                    sub(format!(
                        "skills {skills_count}/{skills_total} · prompt {}",
                        prompt_label
                    ))
                    .into_any_element(),
                    div()
                        .mt(m.v(1.8))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(m.v(1.))
                        .child(skills_node)
                        .child(prompt_node)
                        .into_any_element(),
                ],
                true,
            ))
            .child(region(
                0.,
                0.5,
                vec![
                    h2("Extensions").into_any_element(),
                    sub("Nous extensions".into()).into_any_element(),
                    div().mt(m.v(1.8)).child(ext_node).into_any_element(),
                ],
                true,
            ))
            // 第四格照页面留空：那格是 Permissions，内核页也还没做。
            .child(region(
                0.5,
                0.5,
                vec![h2("Permissions").into_any_element()],
                true,
            ))
            .into_any_element()
    }

    /// 中央那颗虚线椭圆：名字、机器、目录、Create、状态行。
    fn board_hub(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        // 盘面尺寸 = 右格减去四周留白。椭圆是盘面的 36% × 30%（同 HTML demo），

        // 用 cqmin 算会变成正圆 —— 那个比例是宽/高的百分比。
        let board_w = m.w - 2. * f32::from(m.v(1.6));
        let board_h = m.h - f32::from(m.v(1.6));
        let w = (0.36 * board_w).clamp(220., 560.);
        let h = (0.30 * board_h).clamp(180., 320.);
        let muted = rgb(art::MUTED);

        let machine_label = b.machine.clone().unwrap_or_else(|| "本机".into());
        let dir_label = b
            .dir
            .as_ref()
            .map(|d| tail_path(d))
            .unwrap_or_else(|| "—".into());

        // 椭圆 + 虚线边只能自己画：`rounded(px)` 是绝对半径，宽 620 高 250 时会被夹成
        // 一枚胶囊（两头是半圆），不是椭圆；而 GPUI 的 border 又没有 dashed。
        // 画在 canvas 上两件事一起解决 —— 而且这就是页面里那个 `border-radius:50%` 的真形状。
        let ellipse = canvas(
            move |_bounds, _, _| (w, h),
            move |bounds, (w, h), window, _| {
                let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let rx = (w / 2. - 0.5).max(1.);
                let ry = (h / 2. - 0.5).max(1.);
                let mut pts = Vec::with_capacity(96);
                for i in 0..96 {
                    let t = std::f32::consts::TAU * i as f32 / 96.;
                    pts.push(point(
                        px(ox + w / 2. + rx * t.cos()),
                        px(oy + h / 2. + ry * t.sin()),
                    ));
                }
                // 先铺不透明的 panel 底（参考 `.hub { background: var(--panel) }`），
                // 再叠 18% 的青（参考 `.hub::before { background: var(--s5); opacity: .18 }`）。
                // 少了这层底，象限色和 cliff 会从椭圆里透出来 —— 中心就发脏了。
                let mut base = PathBuilder::fill();
                base.add_polygon(&pts, true);
                if let Ok(path) = base.build() {
                    window.paint_path(path, rgb(art::PANEL));
                }
                let mut tint = PathBuilder::fill();
                tint.add_polygon(&pts, true);
                if let Ok(path) = tint.build() {
                    window.paint_path(path, rgb(art::S5).opacity(art::HUB_ALPHA));
                }
                let mut edge = PathBuilder::stroke(px(1.)).dash_array(&[px(3.), px(3.)]);
                edge.add_polygon(&pts, true);
                let edge_color: Hsla = rgb(art::BORDER).into();
                if let Ok(path) = edge.build() {
                    window.paint_path(path, edge_color);
                }
            },
        );

        let hub = div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(m.v(0.7))
            .text_center()
            .w(px(w))
            .h(px(h))
            // 参考 `.hub { padding: 3% 9% }`：CSS 的 % 相对宽度，所以上下 3%、左右 9% 都乘 w。
            .px(px(w * 0.09))
            .py(px(w * 0.03))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(ellipse.size_full()),
            )
            .child(
                div()
                    .text_size(m.f(1.85, 12., 17.))
                    .font_weight(FontWeight::BOLD)
                    .child("New Manual"),
            )
            .child(
                div()
                    .italic()
                    .text_size(m.f(1.4, 9., 12.5))
                    .text_color(muted)
                    .child(b.hub_sub()),
            )
            .child(
                div()
                    .w(px(f32::from(m.v(34.)).min(w * 0.82)))
                    .mt(m.v(0.6))
                    .px(m.v(1.))
                    .py(m.v(0.3))
                    .bg(rgb(art::PANEL))
                    .border_1()
                    .border_color(rgb(art::BORDER))
                    .rounded(px(6.))
                    .text_size(m.f(1.5, 11., 13.5))
                    .child(
                        Textarea::new(&b.name_input)
                            .appearance(false)
                            .bordered(false),
                    ),
            )
            .child(self.board_node(
                "board-machine",
                &machine_label,
                "change ›",
                true,
                m,
                cx,
                |this, _cx| this.newchat.sheet = Some(Sheet::Machine),
            ))
            .child(self.board_node(
                "board-dir",
                &dir_label,
                "browse ›",
                true,
                m,
                cx,
                |this, _cx| this.newchat.sheet = Some(Sheet::Dir),
            ))
            .child(
                div()
                    .id(ElementId::Name("board-create".into()))
                    .px(m.v(1.6))
                    .py(m.v(0.5))
                    .bg(rgb(art::PANEL))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(art::BORDER))
                    .text_size(m.f(1.6, 11., 14.5))
                    .when(b.picked.is_some(), |el| {
                        el.hover(|s| s.bg(rgb(art::SURFACE)).border_color(rgb(art::LINE)))
                    })
                    .when(b.picked.is_none(), |el| el.opacity(0.45))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.board_create(cx);
                    }))
                    .child(if b.creating { "creating…" } else { "Create" }),
            )
            .child(
                div()
                    .h(m.f(1.35, 9., 12.))
                    .text_size(m.f(1.35, 9., 12.))
                    .font_family(art::MONO)
                    .text_color(rgb(art::S4))
                    .child(b.flash.clone().unwrap_or_default()),
            );

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(hub)
            .into_any_element()
    }

    /// 一个 `node`：panel 底、1px 边、6px 圆角、等宽字的 id + 灰色小 tag。
    #[allow(clippy::too_many_arguments)]
    fn board_node(
        &self,
        id: &'static str,
        value: &str,
        tag: &str,
        enabled: bool,
        m: &art::M,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(ElementId::Name(id.into()))
            .flex()
            .items_center()
            .gap(m.v(0.7))
            .px(m.v(0.95))
            .py(m.v(0.42))
            .bg(rgb(art::PANEL))
            .border_1()
            .border_color(rgb(art::BORDER))
            .rounded(px(6.))
            .when(enabled, |el| {
                el.hover(|s| s.border_color(rgb(art::FAINT)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
            })
            .when(!enabled, |el| el.opacity(0.45))
            .child(
                div()
                    .font_family(art::MONO)
                    .text_size(m.f(1.55, 10., 14.))
                    .truncate()
                    .child(value.to_string()),
            )
            .child(
                div()
                    .text_size(m.f(1.55 * 0.82, 9., 12.))
                    .text_color(rgb(art::MUTED))
                    .child(tag.to_string()),
            )
            .into_any_element()
    }

    // ── 弹层 ─────────────────────────────────────────────────────────

    fn board_sheet(&self, m: &art::M, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.newchat.sheet?;
        let (title, hint, body): (String, String, AnyElement) = match sheet {
            Sheet::Model => (
                "选择模型".into(),
                "供应商 › 模型 › 思考 · 点模型选中即收回".into(),
                self.sheet_model(m, cx),
            ),
            Sheet::Skills => (
                "上下文 · 技能".into(),
                "勾完按「完成」· 多选".into(),
                self.sheet_skills(m, cx),
            ),
            Sheet::Prompt => (
                "上下文 · 提示词模板".into(),
                "单值：这一条每轮都带 · 再点一次取消".into(),
                self.sheet_prompt(m, cx),
            ),
            Sheet::Dir => (
                "工作目录".into(),
                "列的是引擎那台机器上的目录 · 点一层进去".into(),
                self.sheet_dir(m, cx),
            ),
            Sheet::Machine => (
                "机器".into(),
                "会话跑在哪台机器上 · 默认本机".into(),
                self.sheet_machine(m, cx),
            ),
        };

        let scrim = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x100f0f8c))
            .id(ElementId::Name("board-scrim".into()))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.newchat.sheet = None;
                cx.notify();
            }));

        Some(
            scrim
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px((0.94 * m.w).min(720.)))
                        // 显式高度，不用 `max_h`：`flex_1` 的基准是 0，
                        // 父容器高度由内容决定时它就会塌成一条缝（正文一格都看不见）。
                        .h(px((0.78 * m.h).min(680.)))
                        .bg(rgb(art::PANEL))
                        .border_1()
                        .border_color(rgb(art::BORDER))
                        .rounded(px(14.))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(m.v(1.))
                                .px(m.v(1.8))
                                .pt(m.v(1.6))
                                .pb(m.v(1.2))
                                .border_b(px(1.))
                                .border_color(rgb(art::BORDER))
                                .child(
                                    div()
                                        .text_size(m.f(1.7, 12., 15.))
                                        .font_weight(FontWeight::BOLD)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .italic()
                                        .text_size(m.f(1.35, 9.5, 12.))
                                        .text_color(rgb(art::MUTED))
                                        .child(hint),
                                ),
                        )
                        .child(body)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px(m.v(1.8))
                                .py(m.v(1.))
                                .border_t(px(1.))
                                .border_color(rgb(art::BORDER))
                                .child(
                                    div()
                                        .font_family(art::MONO)
                                        .text_size(m.f(1.25, 9., 11.))
                                        .text_color(rgb(art::MUTED))
                                        .child("Esc 关闭 · 点空白处也关"),
                                )
                                .child(
                                    div()
                                        .id(ElementId::Name("board-sheet-done".into()))
                                        .px(m.v(1.1))
                                        .py(m.v(0.35))
                                        .rounded(px(6.))
                                        .border_1()
                                        .border_color(rgb(art::ACCENT))
                                        .bg(rgb(art::ACCENT).opacity(0.18))
                                        .text_size(m.f(1.5, 11., 13.))
                                        .hover(|s| s.bg(rgb(art::ACCENT).opacity(0.28)))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.newchat.sheet = None;
                                            cx.notify();
                                        }))
                                        .child("完成 ›"),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// 模型弹层：三列铁路图。**这是这一页的灵魂** —— 供应商 › 模型 › 思考 是
    /// 一条有依赖的语法链，画成三个滚动的列表就看不出「先选谁再选谁」。
    fn sheet_model(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        let (providers, models, levels) = {
            let providers: Vec<String> = b
                .models
                .as_ref()
                .map(|x| {
                    let mut v: Vec<String> = Vec::new();
                    for it in &x.items {
                        if !v.contains(&it.provider) {
                            v.push(it.provider.clone());
                        }
                    }
                    v
                })
                .unwrap_or_default();
            let cur_provider = b
                .picked
                .as_ref()
                .map(|(p, _)| p.clone())
                .unwrap_or_default();
            let models: Vec<String> = b
                .models
                .as_ref()
                .map(|x| {
                    x.items
                        .iter()
                        .filter(|it| it.provider == cur_provider)
                        .map(|it| it.model_id.clone())
                        .collect()
                })
                .unwrap_or_default();
            (providers, models, b.levels())
        };

        let pitch = f32::from(m.v(2.6)).clamp(26., 34.);
        let rows = providers.len().max(models.len()).max(levels.len()).max(1);
        let total_h = pitch * rows as f32 + 16.;

        // 三列的横向位置：列宽按比例切，和页面一样（列窄了不换版面，只是更挤）。
        let width = 720. - 2. * f32::from(m.v(1.));
        let cols = [width * 0.30, width * 0.42, width * 0.28];
        let xs = [0., cols[0], cols[0] + cols[1]];

        let trunk = rgb(art::BORDER);
        let hot = rgb(art::ACCENT);
        let mut layers: Vec<AnyElement> = Vec::new();

        // 每条链：一根竖轨 + 每行一个点 + 一根横支；选中的那条换成 accent。
        for (col, items) in [&providers, &models, &levels].into_iter().enumerate() {
            let x = xs[col] + 8.;
            let n = items.len();
            if n > 0 {
                layers.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(8. + pitch / 2.))
                        .w(px(1.))
                        .h(px(pitch * (n.saturating_sub(1)) as f32))
                        .bg(trunk)
                        .into_any_element(),
                );
            }
            for (row, item) in items.iter().enumerate() {
                let y = 8. + row as f32 * pitch;
                let selected = match col {
                    0 => b.picked.as_ref().map(|(p, _)| p) == Some(item),
                    1 => b.picked.as_ref().map(|(_, mo)| mo) == Some(item),
                    _ => &b.level == item,
                };
                // 点和横支
                layers.push(
                    div()
                        .absolute()
                        .left(px(x - 2.))
                        .top(px(y + pitch / 2. - 2.))
                        .size(px(5.))
                        .rounded_full()
                        .bg(if selected { hot } else { trunk })
                        .into_any_element(),
                );
                layers.push(
                    div()
                        .absolute()
                        .left(px(x + 8.))
                        .top(px(y + pitch / 2.))
                        .w(px(6.))
                        .h(px(1.))
                        .bg(if selected { hot } else { trunk })
                        .into_any_element(),
                );

                let label = if col == 2 {
                    format!("{item} · {}", level_label(item))
                } else {
                    item.clone()
                };
                let action = match col {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                };
                layers.push(
                    div()
                        .id(ElementId::Name(format!("pill-{col}-{row}").into()))
                        .absolute()
                        .left(px(x + 16.))
                        .top(px(y))
                        .w(px(cols[col] - 30.))
                        .h(px(pitch - 6.))
                        .flex()
                        .items_center()
                        .px(px(8.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(if selected { hot } else { rgb(art::BORDER) })
                        .bg(if selected {
                            rgb(art::ACCENT).opacity(0.16)
                        } else {
                            rgb(art::SURFACE)
                        })
                        .text_size(m.f(1.45, 10., 13.))
                        .font_family(art::MONO)
                        .truncate()
                        .hover(|s| s.bg(rgb(art::SURFACE_ALT)))
                        .on_click(cx.listener({
                            let label = label.clone();
                            move |this, _: &ClickEvent, _, cx| {
                                match action {
                                    0 => {
                                        // 换供应商：模型回到这家的第一个，档位重新夹。
                                        this.newchat.picked = Some((label.clone(), String::new()));
                                        this.board_pick_first_model();
                                    }
                                    1 => {
                                        if let Some((p, _)) = this.newchat.picked.clone() {
                                            this.newchat.picked = Some((p, label.clone()));
                                            this.newchat.clamp_level();
                                        }
                                    }
                                    _ => this.newchat.level = label.clone(),
                                }
                                cx.notify();
                            }
                        }))
                        .child(label)
                        .into_any_element(),
                );
            }
        }

        div()
            .id(ElementId::Name("board-model-body".into()))
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.newchat.sheet_scroll)
            .child(div().relative().h(px(total_h)).w_full().children(layers))
            .into_any_element()
    }

    /// 换供应商时，把模型换成这一家的第一个（不然会停在别家的 modelId 上）。
    fn board_pick_first_model(&mut self) {
        let Some((provider, _)) = self.newchat.picked.clone() else {
            return;
        };
        let first = self
            .newchat
            .models
            .as_ref()
            .and_then(|m| m.items.iter().find(|x| x.provider == provider))
            .map(|x| x.model_id.clone());
        if let Some(id) = first {
            self.newchat.picked = Some((provider, id));
        }
        self.newchat.clamp_level();
    }

    /// 技能弹层：平铺多选。**故意不用铁路图** —— 技能是集合不是链。
    fn sheet_skills(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        let rows: Vec<AnyElement> = b
            .skill_list()
            .into_iter()
            .map(|s| {
                let name = s.name.clone();
                let picked = b.picked_skills.contains(&name);
                let desc = s
                    .description
                    .clone()
                    .unwrap_or_default()
                    .replace(['\n', '\r'], " ");
                let meta = if s.category.is_empty() {
                    String::new()
                } else {
                    s.category.join(" / ")
                };
                div()
                    .id(ElementId::Name(format!("skill-{name}").into()))
                    .flex()
                    .items_start()
                    .gap(m.v(0.9))
                    .w_full()
                    .px(m.v(0.8))
                    .py(m.v(0.55))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if picked {
                        rgb(art::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .when(picked, |el| el.bg(rgb(art::ACCENT).opacity(0.14)))
                    .hover(|s| s.bg(rgb(art::ACCENT).opacity(0.07)))
                    .on_click({
                        let name = name.clone();
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if !this.newchat.picked_skills.remove(&name) {
                                this.newchat.picked_skills.insert(name.clone());
                            }
                            cx.notify();
                        })
                    })
                    .child(
                        div()
                            .mt(px(2.))
                            .size(m.f(1.1, 10., 14.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if picked {
                                rgb(art::ACCENT)
                            } else {
                                rgb(art::BORDER)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(m.f(0.85, 8., 11.))
                            .text_color(rgb(art::ACCENT))
                            .child(if picked { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .font_family(art::MONO)
                                    .text_size(m.f(1.5, 11., 13.5))
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(m.f(1.35, 10., 12.5))
                                    .text_color(rgb(art::MUTED))
                                    .truncate()
                                    .child(desc),
                            )
                            .when(!meta.is_empty(), |el| {
                                el.child(
                                    div()
                                        .font_family(art::MONO)
                                        .text_size(m.f(1.2, 9., 11.))
                                        .text_color(rgb(art::FAINT))
                                        .child(meta),
                                )
                            }),
                    )
                    .into_any_element()
            })
            .collect();

        self.sheet_list(
            m,
            rows,
            if b.skill_list().is_empty() {
                "这个盘上还没装技能（技能在 68kazk 那块）"
            } else {
                ""
            },
        )
    }

    /// 模板弹层：单选。再点一次取消 —— 「明确不要模板」也是一个有效选择。
    fn sheet_prompt(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        let rows: Vec<AnyElement> = b
            .prompt_list()
            .into_iter()
            .map(|p| {
                let name = p.name.clone();
                let picked = b.template.as_deref() == Some(name.as_str());
                let desc = p
                    .description
                    .clone()
                    .or_else(|| p.summary.clone())
                    .unwrap_or_default()
                    .replace(['\n', '\r'], " ");
                div()
                    .id(ElementId::Name(format!("prompt-{name}").into()))
                    .flex()
                    .items_start()
                    .gap(m.v(0.9))
                    .w_full()
                    .px(m.v(0.8))
                    .py(m.v(0.55))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if picked {
                        rgb(art::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .when(picked, |el| el.bg(rgb(art::ACCENT).opacity(0.14)))
                    .hover(|s| s.bg(rgb(art::ACCENT).opacity(0.07)))
                    .on_click({
                        let name = name.clone();
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.newchat.template =
                                if this.newchat.template.as_deref() == Some(name.as_str()) {
                                    None
                                } else {
                                    Some(name.clone())
                                };
                            cx.notify();
                        })
                    })
                    // 圆框 = 一次只能选一个。形状本身就在说这事。
                    .child(
                        div()
                            .mt(px(2.))
                            .size(m.f(1.1, 10., 14.))
                            .rounded_full()
                            .border_1()
                            .border_color(if picked {
                                rgb(art::ACCENT)
                            } else {
                                rgb(art::BORDER)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(m.f(0.5, 5., 7.)).rounded_full().bg(if picked {
                                rgb(art::ACCENT)
                            } else {
                                rgba(0x00000000)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .font_family(art::MONO)
                                    .text_size(m.f(1.5, 11., 13.5))
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(m.f(1.35, 10., 12.5))
                                    .text_color(rgb(art::MUTED))
                                    .truncate()
                                    .child(desc),
                            ),
                    )
                    .into_any_element()
            })
            .collect();

        self.sheet_list(
            m,
            rows,
            if b.prompt_list().is_empty() {
                "这个盘上还没有提示词模板（模板在 6ysg01 那块）"
            } else {
                ""
            },
        )
    }

    /// 目录弹层：点一层进一层。**这不是勾选，是导航** —— 所以没有方框，
    /// 当前选中的那格用边框表示。
    fn sheet_dir(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        let at = b.dir_at.clone().unwrap_or_default();
        let mut rows: Vec<AnyElement> = Vec::new();

        if !at.is_empty() {
            let up = at.rsplit_once('/').map(|(p, _)| p.to_string());
            let up = up.filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into());
            rows.push(
                div()
                    .id(ElementId::Name("dir-up".into()))
                    .flex()
                    .items_center()
                    .gap(m.v(0.9))
                    .w_full()
                    .px(m.v(0.8))
                    .py(m.v(0.5))
                    .rounded(px(6.))
                    .hover(|s| s.bg(rgb(art::ACCENT).opacity(0.07)))
                    .on_click(cx.listener({
                        let up = up.clone();
                        move |this, _: &ClickEvent, _, cx| {
                            this.board_dir_enter(up.clone(), cx);
                        }
                    }))
                    .child(
                        div()
                            .font_family(art::MONO)
                            .text_size(m.f(1.4, 10., 13.))
                            .text_color(rgb(art::MUTED))
                            .child(".."),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family(art::MONO)
                            .text_size(m.f(1.4, 10., 13.))
                            .text_color(rgb(art::MUTED))
                            .child(up),
                    )
                    .into_any_element(),
            );
        }

        for d in b.dirs.as_ref().map(|x| x.dirs.clone()).unwrap_or_default() {
            let path = d.path.clone();
            let picked_path = path.clone();
            let selected = b.dir.as_deref() == Some(path.as_str());
            rows.push(
                div()
                    .id(ElementId::Name(format!("dir-{}", d.name).into()))
                    .flex()
                    .items_center()
                    .gap(m.v(0.9))
                    .w_full()
                    .px(m.v(0.8))
                    .py(m.v(0.5))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if selected {
                        rgb(art::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .when(selected, |el| el.bg(rgb(art::ACCENT).opacity(0.14)))
                    .hover(|s| s.bg(rgb(art::ACCENT).opacity(0.07)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.board_dir_enter(picked_path.clone(), cx);
                    }))
                    .child(
                        div()
                            .font_family(art::MONO)
                            .text_size(m.f(1.4, 10., 13.))
                            .text_color(rgb(art::MUTED))
                            .child("›"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .font_family(art::MONO)
                            .text_size(m.f(1.4, 10., 13.))
                            .child(path.clone()),
                    )
                    .into_any_element(),
            );
        }

        self.sheet_list(
            m,
            rows,
            if b.dirs.as_ref().map(|d| d.dirs.is_empty()).unwrap_or(true) {
                "这层没有子目录"
            } else {
                ""
            },
        )
    }

    /// 机器弹层：本机在最上面，`None` 就是它。
    fn sheet_machine(&self, m: &art::M, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.newchat;
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut all: Vec<(Option<String>, String, String)> =
            vec![(None, "本机".into(), "qrd3wh 跑在哪儿就是哪儿".into())];
        for mc in b
            .machines
            .as_ref()
            .map(|x| x.machines.clone())
            .unwrap_or_default()
        {
            let note = mc
                .note
                .clone()
                .or_else(|| mc.addr.clone())
                .unwrap_or_default();
            all.push((Some(mc.name.clone()), mc.name.clone(), note));
        }
        for (value, label, note) in all {
            let picked = b.machine == value;
            rows.push(
                div()
                    .id(ElementId::Name(format!("machine-{}", label.clone()).into()))
                    .flex()
                    .items_center()
                    .gap(m.v(0.9))
                    .w_full()
                    .px(m.v(0.8))
                    .py(m.v(0.55))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if picked {
                        rgb(art::ACCENT)
                    } else {
                        rgba(0x00000000)
                    })
                    .when(picked, |el| el.bg(rgb(art::ACCENT).opacity(0.14)))
                    .hover(|s| s.bg(rgb(art::ACCENT).opacity(0.07)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.newchat.machine = value.clone();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .mt(px(1.))
                            .size(m.f(1.1, 10., 14.))
                            .rounded_full()
                            .border_1()
                            .border_color(if picked {
                                rgb(art::ACCENT)
                            } else {
                                rgb(art::BORDER)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(m.f(0.5, 5., 7.)).rounded_full().bg(if picked {
                                rgb(art::ACCENT)
                            } else {
                                rgba(0x00000000)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .font_family(art::MONO)
                                    .text_size(m.f(1.45, 10., 13.))
                                    .truncate()
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(m.f(1.3, 9.5, 12.))
                                    .text_color(rgb(art::MUTED))
                                    .truncate()
                                    .child(note),
                            ),
                    )
                    .into_any_element(),
            );
        }
        self.sheet_list(m, rows, "")
    }

    /// 弹层的正文：一个能滚的清单。空的时候把「为什么空」说出来，
    /// 别让人对着一个白框猜。
    fn sheet_list(&self, m: &art::M, rows: Vec<AnyElement>, empty: &str) -> AnyElement {
        if rows.is_empty() {
            return div()
                .flex_1()
                .min_h(px(120.))
                .flex()
                .items_center()
                .justify_center()
                .px(m.v(1.2))
                .py(m.v(2.4))
                .text_size(m.f(1.4, 10., 12.5))
                .text_color(rgb(art::MUTED))
                .child(empty.to_string())
                .into_any_element();
        }
        div()
            .id(ElementId::Name("board-sheet-list".into()))
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(m.v(1.2))
            .py(m.v(1.))
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.newchat.sheet_scroll)
            .children(rows)
            .into_any_element()
    }
}

/// 模型清单里有几个供应商。
fn providers_of(models: &Models) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for m in &models.items {
        if !seen.contains(&m.provider.as_str()) {
            seen.push(&m.provider);
        }
    }
    seen.len()
}
