//! markdown 渲染的「样品间」。
//!
//! 只做一件事：把一个固定的样例丢进 `TextView`，用来看渲染成什么样。
//!
//! 为什么需要它：模型订阅过期的时候没法生成带代码块/表格的回复，而样式这种东西
//! 靠看代码改不出来 —— 每次都要真消息才能验一遍的话，改一轮字号就等于赌一轮。
//!
//! 跑：`cargo run -p nousdesk --example mdpreview`

#[path = "../src/md.rs"]
mod md;

use gpui_kit::base::text::TextView;
use gpui_kit::*;

const SAMPLE: &str = r#"## 这是二级标题

普通一行，带 **粗体**、*斜体*、`行内代码`、[链接文字](https://example.com)。

一行
再一行
（这两行之间只有一个换行，靠硬换行才不会粘成一坨）

- 列表第一项
- 列表第二项
  - 嵌一层
1. 有序
2. 第二

> 引用一段话。

| 工具 | 次数 | 说明 |
| --- | --- | --- |
| bash | 12 | 跑命令 |
| read | 7 | 看文件 |

```rust
fn main() {
    // 一个很长的行，看看会不会把气泡撑爆：let x = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let n = 42;
    println!("{n}");
}
```

```bash
grep -rn "TODO" . | head
```

收尾的一行。
"#;

struct Preview;

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xededed))
            .flex()
            .justify_start()
            .items_start()
            .p(px(16.))
            .child(
                div()
                    .w(px(480.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .bg(rgb(0xffffff))
                    .child(
                        TextView::markdown(ElementId::Name("md-sample".into()), md::chat_md(SAMPLE))
                            .selectable(true)
                            .text_size(px(13.5))
                            .text_color(rgb(0x1a1a1a))
                            .style(md::md_style())
                            .code_block_actions(md::code_copy_button)
                            .shared_code_block_highlighter(md::code_highlighter()),
                    ),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(560.), px(780.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("markdown 样品间".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |_window, cx| cx.new(|_| Preview)).expect("开窗");
        });
}
