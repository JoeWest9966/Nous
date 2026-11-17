//! SSE 合帧：`text_delta` 只进缓冲，16 ms 定时器读一次再发。
//!
//! ## 为什么必须合帧
//!
//! `wire.md` 的合帧铁律：一个 token 一次 `notify()` 会把 CPU 烧穿。传输侧同理 ——
//! 每个 delta 一帧，1000 个 token 就是 1000 次 SSE 写 + 1000 次客户端重排。
//!
//! ## 合帧**只**合并 `message_update`
//!
//! 别的帧（`message_end` / `entry_appended` / `tool_execution_start` / `task_failed` …）
//! 是**状态转换**，不是增量：攒它们会让「工具启动」等到工具结束才出现（issue #22 的判据）。
//! 所以规则是：遇到非 `message_update` 先把缓冲冲刷掉，再原样发它。
//!
//! ## 两个模式，同一个实现
//!
//! [`Framing::Windowed`] 是生产行为；[`Framing::Immediate`] 是**负向对照**：同一个循环里
//! 只换一个分支，每条事件立即发。判据必须看到它把 1000 条 delta 变成 ≈1000 帧，否则
//! 「帧数 < 100」可能只是因为它把字吞了 —— 这是 `docs/decisions.md` 那条「判据测哪一面」。
//!
//! 注意不要用 `interval = 0` 做对照：窗口为 0 时 1000 条事件同样会在同一个 tick 内就绪、
//! 同样只发一帧，对照不会红。能关掉合帧的是**分支**，不是参数。

use std::time::Duration;

use nous_wire::{MessageChange, RemoteEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// 合帧窗口。`wire.md` 写死 16 ms（≈60 Hz），不接受配置漂移。
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// 生产行为：`message_update` 进缓冲，16 ms 读一次。
    Windowed,
    /// 负向对照：关闭合帧，每条事件立即发。
    Immediate,
}

/// 起一个合帧任务：读 `input`，写 `output`。任一端的关闭都会让任务结束。
pub fn spawn(
    framing: Framing,
    input: mpsc::Receiver<RemoteEvent>,
    output: mpsc::Sender<RemoteEvent>,
) -> JoinHandle<()> {
    tokio::spawn(run(framing, input, output))
}

/// 合帧循环本体。测试可以直接 `spawn(run(...))`，与生产同一条路径。
pub async fn run(
    framing: Framing,
    mut input: mpsc::Receiver<RemoteEvent>,
    output: mpsc::Sender<RemoteEvent>,
) {
    let mut buffer = UpdateBuffer::default();
    let mut deadline: Option<Instant> = None;
    loop {
        let tick = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            // 定时器优先：窗口到点时先冲刷，再处理刚到的下一批。
            biased;
            _ = tick => {
                if !flush(&mut buffer, &output).await {
                    return;
                }
                deadline = None;
            }
            received = input.recv() => {
                let Some(event) = received else {
                    // 上游关了：把缓冲里最后那批发出去（这不是「尾帧必交」，是连接关闭前
                    // 不让已经收到的字符凭空消失；`wire.md` 没有「尾帧丢弃」的规矩）。
                    flush(&mut buffer, &output).await;
                    return;
                };
                match event {
                    RemoteEvent::MessageUpdate { changes, message_id, message } => {
                        if framing == Framing::Immediate {
                            if output.send(RemoteEvent::MessageUpdate { changes, message_id, message }).await.is_err() {
                                return;
                            }
                            continue;
                        }
                        buffer.absorb(changes, message_id, message);
                        if deadline.is_none() {
                            deadline = Some(Instant::now() + FRAME_INTERVAL);
                        }
                    }
                    other => {
                        // 状态转换帧：先把缓冲发完，保证客户端看到的顺序不变，
                        // 再原样发这一帧（它不参与合帧）。
                        if !flush(&mut buffer, &output).await {
                            return;
                        }
                        deadline = None;
                        if output.send(other).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// 一个窗口里攒下的 `message_update` 载荷。
#[derive(Default)]
struct UpdateBuffer {
    changes: Vec<MessageChange>,
    message_id: Option<String>,
    message: Option<serde_json::Value>,
}

impl UpdateBuffer {
    fn absorb(
        &mut self,
        changes: Vec<MessageChange>,
        message_id: Option<String>,
        message: Option<serde_json::Value>,
    ) {
        self.changes.extend(changes);
        if message_id.is_some() {
            self.message_id = message_id;
        }
        if message.is_some() {
            self.message = message;
        }
    }

    fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.message_id.is_none() && self.message.is_none()
    }

    fn take(&mut self) -> RemoteEvent {
        RemoteEvent::MessageUpdate {
            changes: std::mem::take(&mut self.changes),
            message_id: self.message_id.take(),
            message: self.message.take(),
        }
    }
}

/// 冲掉缓冲。返回 `false` 表示下游已关，循环该停了。
async fn flush(buffer: &mut UpdateBuffer, output: &mpsc::Sender<RemoteEvent>) -> bool {
    if buffer.is_empty() {
        return true;
    }
    output.send(buffer.take()).await.is_ok()
}
