//! 扩展作者面：注册 section / tool / 名牌，然后 [`run`]。
//!
//! 进程的 fd 契约（spec ch7 / EX-71）：`0/1/2` 是标准三件，`3` 是 host→ext 管道的读端，
//! `4` 是 ext→host 管道的写端。除这五个之外一个 fd 都不许有。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::content::Handles;
use crate::frame::{read_frame, write_frame, FrameError, PROTOCOL};

/// 一次 section 调用的入参。**帧里只有三样**：`conversationId` / `cwd` / `extensions`
/// （EX-08）；`shown` / `read` / `model` 不进帧（EX-12）。
///
/// `extensions` 是**本会话选中的名字**，不是装载名：`skill:<名>` / `prompt:<名>`，顺序即
/// 选中顺序。内核从会话的 `pi.agent` 文档里拼（技能清单 + 模板名），扩展只按名字挑自己
/// 那几条 —— 装载名（`skills` / `prompts` 这种进程名）对扩展没有信息量，它自己就是其中之一。
#[derive(Debug, Clone)]
pub struct SectionInput {
    pub conversation_id: i64,
    pub cwd: String,
    pub extensions: Vec<String>,
    /// 装载时代读的内容。section 只许从这里读数据 —— 碰盘就破坏纯度。
    pub handles: Handles,
}

impl SectionInput {
    /// 把 `probe` 句柄（样例用的场景开关）当一行文本读出来。正式扩展不该用它。
    pub fn probe(&self) -> Option<&str> {
        self.handles.get("probe").and_then(|content| content.first_text())
    }
}

/// 一件能力：一段提示词。**纯函数**：同输入 → 同字节输出。
pub trait Section: Send {
    /// 清单里声明的键。
    fn key(&self) -> &'static str;
    /// 渲染这一段。不许碰盘、不许看时钟、不许发网络。
    fn render(&self, input: &SectionInput) -> String;
}

/// 一件能力：一个工具。唯一会**产生副作用**的一件。
pub trait Tool: Send {
    fn name(&self) -> &'static str;
    /// 参数的 JSON Schema（展示用）。
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    /// 跑一次。输出用 [`ToolCtx::output`] 分帧发；返回终态 `result`。
    ///
    /// 取消不是错误：看到 [`ToolCtx::cancelled`] 就返回
    /// `Err(Cancel::as_str().to_string())`，SDK 会补一帧 `final{cancelled:true}`。
    fn call(&mut self, ctx: &mut ToolCtx, args: &Value) -> Result<Value, String>;
}

/// 取消的约定返回值。工具在取消后返回它，SDK 就发 `final{cancelled:true}`。
pub struct Cancel;

impl Cancel {
    pub fn as_str() -> &'static str {
        "\u{1}cancelled"
    }
}

/// "我已经自己发过终帧了"：SDK 不再补 `final`。
///
/// 只在样例里模拟"取消之后又发一条 output"（EX-19 的晚期输出）时用。
pub struct Finished;

impl Finished {
    pub fn as_str() -> &'static str {
        "\u{1}finished"
    }
}

/// 名牌规则：扫哪个句柄的内容、加什么前缀。
///
/// 名字是**扫盘后才知道的**（`skill:<名>` / `prompt:<名>`），清单里写不出来 ——
/// 所以由扩展在应答里自报（spec 3.3）。
#[derive(Debug, Clone)]
pub struct Nameplate {
    pub handle: String,
    pub prefix: String,
}

/// 扩展进程能干的事的集合。
pub struct Extension {
    pub name: String,
    pub sections: Vec<Box<dyn Section>>,
    pub tools: Vec<Arc<Mutex<Box<dyn Tool>>>>,
    pub nameplates: Vec<Nameplate>,
}

impl Extension {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sections: Vec::new(),
            tools: Vec::new(),
            nameplates: Vec::new(),
        }
    }

    pub fn section(mut self, section: impl Section + 'static) -> Self {
        self.sections.push(Box::new(section));
        self
    }

    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools
            .push(Arc::new(Mutex::new(Box::new(tool) as Box<dyn Tool>)));
        self
    }

    pub fn nameplates(mut self, handle: &str, prefix: &str) -> Self {
        self.nameplates.push(Nameplate {
            handle: handle.to_string(),
            prefix: prefix.to_string(),
        });
        self
    }
}

/// 握手前就能定的行为。只给样例/恶意样例用；真扩展保持默认。
#[derive(Debug, Clone)]
pub struct Behavior {
    /// 自报的协议版本（默认 [`PROTOCOL`]）。不符即拒载（EX-06）。
    pub protocol: u32,
    /// 收到 hello 后立刻退出（开机装载失败 / 重载失败用）。
    pub exit_after_hello: bool,
    /// 握手完成后写一个 2MB 的帧（EX-58）。
    pub oversize_frame: bool,
    /// 握手完成后吃这么多 RSS（EX-36）。
    pub rss_bytes: u64,
    /// 收到 section 调用就退出（崩溃一次，EX-28）。
    pub crash_on_section: bool,
    /// section 调用前睡这么久（撞 1s 总预算，EX-29）。
    pub slow_section_ms: u64,
    /// 额外自报的名牌（冲突用，EX-59）。
    pub extra_nameplates: Vec<String>,
    /// 不报任何名牌（EX-23）。
    pub skip_nameplates: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            protocol: PROTOCOL,
            exit_after_hello: false,
            oversize_frame: false,
            rss_bytes: 0,
            crash_on_section: false,
            slow_section_ms: 0,
            extra_nameplates: Vec::new(),
            skip_nameplates: false,
        }
    }
}

/// 扩展的入口。读完 fd 3/4 上的协议，然后不返回。
pub fn run(extension: Extension) -> ! {
    run_with_behavior(extension, Behavior::default())
}

/// 带行为的入口（样例/恶意样例）。
pub fn run_with_behavior(extension: Extension, behavior: Behavior) -> ! {
    let input = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(3) };
    let output = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(4) };
    let code = run_on(input, output, extension, behavior);
    std::process::exit(code);
}

/// 在一对读写端上跑协议。给单测用（真进程用 [`run`]）。
pub fn run_on<R, W>(mut input: R, output: W, mut extension: Extension, behavior: Behavior) -> i32
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let writer: SharedWriter = Arc::new(Mutex::new(Box::new(output)));

    // 读端独立一条线程：工具执行期间也要能收到 cancel（取消走**同一条** host→ext 管道）。
    let (tx, rx) = mpsc::channel::<Result<Value, FrameError>>();
    std::thread::spawn(move || loop {
        match read_frame(&mut input) {
            Ok(frame) => {
                if tx.send(Ok(frame)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = tx.send(Err(error));
                return;
            }
        }
    });

    // ① hello
    let first = match rx.recv() {
        Ok(Ok(frame)) => frame,
        _ => return 2,
    };
    if first.get("kind").and_then(Value::as_str) != Some("hello") {
        return 2;
    }
    if behavior.exit_after_hello {
        return 1;
    }
    send(
        &writer,
        &json!({ "kind": "hello", "protocol": behavior.protocol }),
    );
    if behavior.oversize_frame {
        // 2MB 单帧：先写长度前缀，再写 payload。宿主必须在**读入之前**就拒（EX-58）。
        let payload = vec![b'x'; 2 * 1024 * 1024];
        let mut guard = writer.lock().expect("writer");
        let _ = guard.write_all(&(payload.len() as u32).to_be_bytes());
        let _ = guard.write_all(&payload);
        let _ = guard.flush();
        drop(guard);
        // 宿主会杀我们；自己先睡住，别让进程提前退出掩盖被杀这件事。
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    if behavior.rss_bytes > 0 {
        let mut ballast = vec![0u8; behavior.rss_bytes as usize];
        // 逐页触一遍，RSS 才真的涨上去。
        for page in ballast.chunks_mut(4096) {
            page[0] = 1;
        }
        // 拿着不放手，等宿主按 RSS 硬顶来杀。
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
        #[allow(unreachable_code)]
        drop(ballast);
    }

    // ② 内容（内核对句柄值的代读结果）
    let handles: Handles = match rx.recv() {
        Ok(Ok(frame)) if frame.get("kind").and_then(Value::as_str) == Some("content") => frame
            .get("handles")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        _ => return 2,
    };

    // ③ 名牌：由扩展自报（扫内容里的文件名）
    let mut names = Vec::new();
    if !behavior.skip_nameplates {
        for rule in &extension.nameplates {
            if let Some(content) = handles.get(&rule.handle) {
                for stem in content.stems() {
                    names.push(format!("{}{}", rule.prefix, stem));
                }
            }
        }
        names.extend(behavior.extra_nameplates.iter().cloned());
    }
    names.sort();
    names.dedup();
    send(&writer, &json!({ "kind": "nameplates", "names": names }));

    // ④ 调用循环
    for incoming in rx {
        let frame = match incoming {
            Ok(frame) => frame,
            Err(_) => return 0,
        };
        match frame.get("kind").and_then(Value::as_str) {
            Some("section") => {
                let call = frame.get("call").and_then(Value::as_u64).unwrap_or(0);
                if behavior.crash_on_section {
                    return 1;
                }
                if behavior.slow_section_ms > 0 {
                    std::thread::sleep(Duration::from_millis(behavior.slow_section_ms));
                }
                let input = section_input(&frame, &handles);
                let sections = render_sections(&mut extension, &input);
                send(
                    &writer,
                    &json!({ "call": call, "kind": "result", "sections": sections }),
                );
            }
            Some("tool") => {
                let call = frame.get("call").and_then(Value::as_u64).unwrap_or(0);
                let tool = frame
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = frame.get("args").cloned().unwrap_or_else(|| json!({}));
                let Some(index) = extension
                    .tools
                    .iter()
                    .position(|item| item.lock().expect("tool").name() == tool)
                else {
                    send(
                        &writer,
                        &json!({ "call": call, "kind": "result", "error": format!("unknown tool {tool}") }),
                    );
                    continue;
                };
                // 工具搬进自己的线程：主循环继续收 cancel（同一条管道）。
                let tool_box = extension.tools[index].clone();
                let writer = writer.clone();
                let cancelled = Arc::new(AtomicBool::new(false));
                // 把取消标志塞进一张表，主循环收到 cancel 时置位。
                CANCELS.lock().expect("cancels").insert(call, cancelled.clone());
                std::thread::spawn(move || {
                    let mut ctx = ToolCtx {
                        call,
                        writer,
                        seq: AtomicU64::new(0),
                        cancelled,
                    };
                    let mut guard = tool_box.lock().expect("tool");
                    match guard.call(&mut ctx, &args) {
                        Ok(result) => ctx.finish_ok(result),
                        Err(message) if message == Cancel::as_str() => ctx.finish_cancelled(),
                        Err(message) if message == Finished::as_str() => {}
                        Err(message) => ctx.finish_error(&message),
                    }
                });
            }
            Some("cancel") => {
                let call = frame.get("call").and_then(Value::as_u64).unwrap_or(0);
                if let Some(flag) = CANCELS.lock().expect("cancels").get(&call) {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            _ => {}
        }
    }
    0
}

/// 在飞调用的取消标志。**按 call id 路由，不靠 pid**（spec 2.2）。
static CANCELS: Mutex<BTreeMap<u64, Arc<AtomicBool>>> = Mutex::new(BTreeMap::new());

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(writer: &SharedWriter, frame: &Value) {
    let mut guard = writer.lock().expect("writer");
    let _ = write_frame(&mut **guard, frame);
}

fn section_input(frame: &Value, handles: &Handles) -> SectionInput {
    let input = frame.get("input").cloned().unwrap_or_else(|| json!({}));
    SectionInput {
        conversation_id: input
            .get("conversationId")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        cwd: input
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        extensions: input
            .get("extensions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        handles: handles.clone(),
    }
}

/// 按清单顺序产出；应答里的顺序与 `tag` 只认清单。
///
/// 样例开关（`probe` 句柄里的 `mode` 文件）：`shuffle` = 故意打乱应答顺序（EX-15），
/// `wrong-tag` = 故意写错 `tag`（EX-16）。
fn render_sections(extension: &mut Extension, input: &SectionInput) -> Vec<Value> {
    let mode = input.probe().unwrap_or("").to_string();
    let mut sections: Vec<Value> = extension
        .sections
        .iter()
        .map(|section| {
            json!({
                "key": section.key(),
                "text": section.render(input),
                "tag": section.key(),
            })
        })
        .collect();
    if mode.contains("shuffle") {
        sections.reverse();
    }
    if mode.contains("wrong-tag") {
        for section in &mut sections {
            if let Some(tag) = section.get_mut("tag") {
                *tag = json!("wrong");
            }
        }
    }
    sections
}

/// 工具执行时的上下文：分帧输出 + 取消。
pub struct ToolCtx {
    call: u64,
    writer: SharedWriter,
    seq: AtomicU64,
    cancelled: Arc<AtomicBool>,
}

impl ToolCtx {
    /// 发一段输出。`seq` 单调、无缺号、不重排（EX-17）。
    pub fn output(&mut self, text: &str) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        send(
            &self.writer,
            &json!({ "call": self.call, "seq": seq, "kind": "output", "text": text }),
        );
    }

    /// 查询是否被取消。工具应在每个可中断点检查它。
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 阻塞等待取消或被唤醒（样例的 `delayMs` 用它，而不是忙等）。
    pub fn sleep_unless_cancelled(&self, total: Duration) {
        let step = Duration::from_millis(10);
        let mut waited = Duration::ZERO;
        while waited < total {
            if self.cancelled() {
                return;
            }
            std::thread::sleep(step);
            waited += step;
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    /// 成功终态：`result` 与 `mutations` 在**同一帧**里（宿主会把它们放进同一次提交）。
    pub fn finish_ok(&self, result: Value) {
        let seq = self.next_seq();
        send(
            &self.writer,
            &json!({ "call": self.call, "seq": seq, "kind": "final", "result": result }),
        );
    }

    /// 取消终态：`final{cancelled:true}`。
    pub fn finish_cancelled(&self) {
        let seq = self.next_seq();
        send(
            &self.writer,
            &json!({ "call": self.call, "seq": seq, "kind": "final", "cancelled": true }),
        );
    }

    /// 明确错误：宿主把这条变成一条 error 工具结果回给模型（不是崩溃）。
    pub fn finish_error(&self, message: &str) {
        let seq = self.next_seq();
        send(
            &self.writer,
            &json!({ "call": self.call, "seq": seq, "kind": "final", "error": message }),
        );
    }
}
