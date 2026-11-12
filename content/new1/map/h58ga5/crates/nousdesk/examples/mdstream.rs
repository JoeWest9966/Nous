//! 流式渲染压测台。
//!
//! 只回答一个问题：**一条消息一直在长的时候，会不会卡 / 会不会抖。**
//!
//! 做法：造一篇 8000 字的 markdown（散文 + 代码块 + 表格 + 列表），每 16ms 往尾巴上
//! 补几个字，就像引擎在吐 token。每一帧开头记一次时间戳，帧间隔就是「这一帧花了多久」——
//! 因为 gpui 只在被 notify 时重绘，间隔被拉长 = 渲染吃掉了时间。
//!
//! 跑：`cargo run -p nousdesk --example mdstream`（打 8000 字）或者
//! `MAX_CHARS=600 cargo run ...`（短消息对照）。
//!
//! 判读：间隔稳定在 16~17ms、峰值 < 30ms = 没问题；峰值上百毫秒 = 真流式会卡。

#[path = "../src/md.rs"]
mod md;

use gpui_kit::base::text::TextView;
use gpui_kit::*;
use smol::Timer;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 造一篇「像模型写的」长回答：散文夹代码块、表格、列表。
fn make_doc(sections: usize) -> String {
    let mut s = String::new();
    for n in 0..sections {
        s.push_str(&format!(
            "## 第 {n} 节\n\n这一段是普通说明文字，带 **粗体**、*斜体* 和 `行内代码`，再补一句稍微长一点的话，\
             让整行被填满，看看换行和行距在这种密度下是什么样。\n\n\
             - 要点之一：这里写点什么\n- 要点之二：这里再写点什么\n  - 嵌一层\n\n\
             ```rust\nfn f{n}(x: i32) -> i32 {{\n    // 第 {n} 段代码，故意写得长一点让它横着滚\n    let y = x * 2 + {n};\n    y\n}}\n```\n\n\
             | 列 | 值 |\n| --- | --- |\n| a{n} | {n} |\n\n"
        ));
    }
    s
}

/// 「真实的回答」：几千字散文，中途只有一两段代码 —— 模型写长文最常见的样子。
/// 上面那个 `make_doc` 是极端形态（每 200 字一个代码块 + 一张表），拿来做上限。
fn make_prose(chars: usize) -> String {
    let para = "这一段是模型写长文时常见的那种说明文字：先解释背景，再讲清楚为什么，最后                给一个不痛不痒的结论。中间会掺 **粗体** 和 `行内代码`，偶尔来个 [链接](https://example.com)。                长度凑够几千字以后，每一帧重新解析的代价才开始显现出来。";
    let mut s = String::from(
        "## 一个很长的回答

",
    );
    while s.chars().count() < chars / 2 {
        s.push_str(para);
        s.push_str(
            "

",
        );
    }
    s.push_str(
        "```rust
fn main() {
    println!(\"插在中间的一段代码\");
}
```

",
    );
    while s.chars().count() < chars {
        s.push_str(para);
        s.push_str(
            "

",
        );
    }
    s
}

/// 最常见的坏情况：模型在写一个长代码文件 —— 一个围栏代码块从头长到几千字。
/// 这条路上还有一个额外的坑：高亮结果是按「内容哈希」缓存的，流式时**每帧内容都变**，
/// 等于每帧把整个代码块重新跑一遍 syntect。
fn make_code(chars: usize) -> String {
    let mut s = String::from(
        "这是要给你写的一个模块，先看代码：

```rust
",
    );
    let mut n = 0;
    while s.chars().count() < chars {
        s.push_str(&format!(
            "fn handler_{n}(req: Request) -> Result<Response, Error> {{
    // 第 {n} 个处理器：校验参数，读数据库，拼响应
    let rows = db.query(\"select * from t where id = ?\", &[req.id])?;
    if rows.is_empty() {{ return Err(Error::NotFound); }}
    Ok(Response::json(rows))
}}

"
        ));
        n += 1;
    }
    s.push_str(
        "```

上面就是全部代码。
",
    );
    s
}

struct Stats {
    last: Instant,
    gaps: VecDeque<f32>,
    peak: f32,
    frames: u64,
    last_report: Instant,
    reported: u64,
}

struct Stream {
    chars: Vec<char>,
    pos: usize,
    chunk: usize,
    text: String,
    stats: Stats,
    /// 关掉语法高亮跑一遍，用来确认「卡」到底是不是高亮造成的。
    nohl: bool,
}

impl Stream {
    fn new(sections: usize, chunk: usize, prose: bool, code: bool) -> Self {
        let doc = if code {
            make_code(sections)
        } else if prose {
            make_prose(sections)
        } else {
            make_doc(sections)
        };
        Self {
            chars: doc.chars().collect(),
            pos: 0,
            chunk,
            text: String::new(),
            nohl: std::env::var("NOHL").is_ok(),
            stats: Stats {
                last: Instant::now(),
                gaps: VecDeque::new(),
                peak: 0.0,
                frames: 0,
                last_report: Instant::now(),
                reported: 0,
            },
        }
    }

    /// 吐一批字。返回 true 表示内容变了（要重绘）。
    fn tick(&mut self) -> bool {
        if self.pos >= self.chars.len() {
            // 吐完了停一会儿，然后从头再来一轮，方便手动看几遍。
            if self.stats.last_report.elapsed() > Duration::from_secs(3) {
                self.text.clear();
                self.pos = 0;
                self.stats.gaps.clear();
                self.stats.peak = 0.0;
                self.stats.last_report = Instant::now();
                return true;
            }
            return false;
        }
        let end = (self.pos + self.chunk).min(self.chars.len());
        self.text.extend(&self.chars[self.pos..end]);
        self.pos = end;
        true
    }

    fn frame_gap(&mut self) {
        let now = Instant::now();
        let gap = now.duration_since(self.stats.last).as_secs_f32() * 1000.0;
        self.stats.last = now;
        self.stats.frames += 1;
        self.stats.gaps.push_back(gap);
        while self.stats.gaps.len() > 240 {
            self.stats.gaps.pop_front();
        }
        self.stats.peak = self.stats.peak.max(gap);

        if self.stats.last_report.elapsed() >= Duration::from_secs(1) {
            let n = self.stats.gaps.len().max(1);
            let avg = self.stats.gaps.iter().sum::<f32>() / n as f32;
            let max = self.stats.gaps.iter().cloned().fold(0.0, f32::max);
            let text_len = self.text.chars().count();
            let fps = self.stats.frames - self.stats.reported;
            println!(
                "stream: {text_len:5} 字 · 帧/秒 {fps:5} · 间隔 平均 {avg:5.1}ms 最大 {max:5.1}ms · 峰值 {:6.1}ms",
                self.stats.peak
            );
            self.stats.reported = self.stats.frames;
            self.stats.last_report = Instant::now();
        }
    }

    fn overlay(&self) -> String {
        let n = self.stats.gaps.len().max(1);
        let avg = self.stats.gaps.iter().sum::<f32>() / n as f32;
        let max = self.stats.gaps.iter().cloned().fold(0.0, f32::max);
        format!(
            "{} 字 · 间隔 平均 {:.1}ms 最大 {:.1}ms · 峰值 {:.1}ms",
            self.text.chars().count(),
            avg,
            max,
            self.stats.peak
        )
    }
}

impl Render for Stream {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.frame_gap();
        let overlay = self.overlay();
        div()
            .size_full()
            .bg(rgb(0xededed))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(rgb(0x444444))
                    .child(overlay),
            )
            .child(
                div()
                    .w(px(500.))
                    .mx(px(16.))
                    .my(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .bg(rgb(0xffffff))
                    .child({
                        let tv = TextView::markdown(
                            ElementId::Name("md-stream".into()),
                            md::chat_md(&self.text),
                        )
                        .selectable(true)
                        .text_size(px(13.5))
                        .text_color(rgb(0x1a1a1a))
                        .style(md::md_style())
                        .code_block_actions(md::code_copy_button);
                        if self.nohl {
                            tv
                        } else {
                            tv.shared_code_block_highlighter(md::code_highlighter())
                        }
                    }),
            )
    }
}

fn main() {
    let sections: usize = std::env::var("SECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    // PROSE=1：散文版长回答（SECTIONS 当「总字数」用）
    let prose = std::env::var("PROSE").is_ok();
    // CODE=1：长代码块一路长大（顺手压 syntect 高亮的缓存失效问题）
    let code = std::env::var("CODE").is_ok();
    // 每批吐几个字。默认按「6 字 / 16ms ≈ 375 字每秒」折算，加快/放慢节奏时
    // 用 INTERVAL 调帧率、字速自动跟着走 —— 这样不同节流的对比才公平：
    // 同样是每秒 375 字，区别只在「一秒钟重画几次」。
    let interval: u64 = std::env::var("INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let chunk: usize = std::env::var("CHUNK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(((6 * interval / 16).max(1)) as usize);

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(580.), px(760.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("流式压测".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let (_, view) = gpui_kit::open_window(options, cx, move |_window, cx| {
                cx.new(|_| Stream::new(sections, chunk, prose, code))
            })
            .expect("开窗");

            cx.spawn(async move |cx| {
                loop {
                    Timer::after(Duration::from_millis(interval)).await;
                    view.update(cx, |this, cx| {
                        if this.tick() {
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        });
}
