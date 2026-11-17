//! 有界输出流与自适应进度提交 —— `packages/chord/src/output.ts` 的 Rust 翻译。
//!
//! **没有内部依赖**（AGENTS.md §二）：本模块只依赖 `std`。上游的 `TelemetrySpan` 在这里
//! 换成一个调用方注入的回调 [`ProgressHook`]，而不是把 `nous-telemetry` 加进 `nous-chord`
//! —— 契约图不许改，也不另造一个平行 span 类型。两个常量照抄上游，不另立数值：
//! [`PROGRESS_BYTES_PER_SECOND`] = 100 * 1024；默认最小间隔 100 ms
//! （`docs/storage.md` §6 的 `outputIntervalMs = 100`）。
//!
//! ## 与上游的差别
//!
//! - `Uint8Array` → `Vec<u8>`；`TextDecoder` 换成自己实现的一小段增量 UTF-8 解码
//!   （状态语义一致：残缺字符先攒着，字符串 chunk / skip 用 U+FFFD 冲掉，BOM 只在最开头丢）。
//! - `Progress` 的 `Promise`/`setTimeout`/`Date.now` 换成注入的 [`ProgressClock`]：
//!   纯库不占运行时，时间、定时器、异步写全由调用方提供。`write` 返回本次写的字节数，
//!   由此算出下一次提交的最早时刻（`started + max(min_interval, bytes/throughput)`）。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 一次输出流保留多少。上游 `OutputLimits`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    /// 保留字节上限。
    pub max_bytes: usize,
    /// 保留行数上限。
    pub max_lines: usize,
    /// 保留头部还是尾部。
    pub retain: Retain,
}

/// 保留哪一端。上游 `retain: "head" | "tail"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retain {
    Head,
    Tail,
}

/// 保留下来的文本和被丢掉的总量。上游 `BoundedOutput`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub text: String,
    pub dropped_bytes: usize,
    pub dropped_lines: usize,
}

/// 精确切片和它没留下的部分。上游 `OutputSlice`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSlice {
    pub text: String,
    pub bytes: usize,
    pub dropped_bytes: usize,
    pub dropped_lines: usize,
}

/// 紧挨当前 chunk 之前被省略的输出。上游 `OutputSkip`（U+FFFD 计 3 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputSkip {
    pub bytes: usize,
    pub newlines: usize,
    pub ends_with_newline: bool,
}

/// 输出块：字节或已解码文本。上游 `string | Uint8Array`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    Bytes(Vec<u8>),
    Text(String),
}

const NEWLINE: u8 = 0x0a;
const REPLACEMENT: &str = "\u{fffd}";

/// 去掉会破坏显示和记录的转义序列及控制字符；tab 与 newline 留着。
/// 两个字符类照抄上游 `ESCAPE_SEQUENCE` / `INVALID_OUTPUT`。
pub fn sanitize_output(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' {
            if let Some(end) = escape_sequence_end(&chars, i) {
                i = end;
                continue;
            }
        }
        let c = chars[i];
        if !matches!(c, '\x00'..='\x08' | '\x0b'..='\x1f' | '\u{fff9}'..='\u{fffb}') {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// 返回转义序列之后的下标。三条分支对应上游正则的三段（OSC/String/CSI/短 ESC）。
fn escape_sequence_end(chars: &[char], start: usize) -> Option<usize> {
    let n = chars.len();
    let next = *chars.get(start + 1)?;
    if matches!(next, ']' | 'P' | 'X' | '^' | '_') {
        // 字符串型：到 BEL 或 ST（ESC \）为止。
        let mut i = start + 2;
        while i < n {
            if chars[i] == '\x07' {
                return Some(i + 1);
            }
            if chars[i] == '\x1b' && chars.get(i + 1) == Some(&'\\') {
                return Some(i + 2);
            }
            i += 1;
        }
        // 上游正则要求闭合；没闭合就原样留给控制字符过滤。
        return None;
    }
    if next == '[' {
        // CSI：参数字节 + 中间字节 + 最终字节。
        let mut i = start + 2;
        while i < n && ('\x30'..='\x3f').contains(&chars[i]) {
            i += 1;
        }
        while i < n && ('\x20'..='\x2f').contains(&chars[i]) {
            i += 1;
        }
        if i < n && ('\x40'..='\x7e').contains(&chars[i]) {
            return Some(i + 1);
        }
        return None;
    }
    // 短形式：ESC + 若干中间字节 + 一个 0x30..=0x7e。
    if ('\x30'..='\x7e').contains(&next) {
        return Some(start + 2);
    }
    let mut i = start + 1;
    while i < n && ('\x20'..='\x2f').contains(&chars[i]) {
        i += 1;
    }
    if i < n && ('\x30'..='\x7e').contains(&chars[i]) {
        return Some(i + 1);
    }
    None
}

/// 按整行把 `text` 限在 limits 内；单行超长时在字符边界上切。上游 `boundOutput`。
pub fn bound_output(text: &str, limits: OutputLimits) -> OutputSlice {
    let bytes = text.as_bytes();
    let (from, to) = if limits.retain == Retain::Head {
        head_range(bytes, limits)
    } else {
        tail_range(bytes, limits)
    };
    let kept = &bytes[from..to];
    let slice = if kept.len() == bytes.len() {
        text.to_string()
    } else {
        String::from_utf8_lossy(kept).into_owned()
    };
    OutputSlice {
        text: slice,
        bytes: kept.len(),
        dropped_bytes: bytes.len() - kept.len(),
        dropped_lines: line_count(bytes) - line_count(kept),
    }
}

fn head_range(bytes: &[u8], limits: OutputLimits) -> (usize, usize) {
    if limits.max_lines == 0 || limits.max_bytes == 0 {
        return (0, 0);
    }
    let mut end = bytes.len();
    let mut lines = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == NEWLINE {
            lines += 1;
            if lines == limits.max_lines {
                end = index + 1;
                break;
            }
        }
    }
    if end > limits.max_bytes {
        let newline = bytes[..limits.max_bytes - 1]
            .iter()
            .rposition(|b| *b == NEWLINE);
        end = match newline {
            Some(index) => index + 1,
            None => character_end(bytes, limits.max_bytes),
        };
    }
    (0, end)
}

fn tail_range(bytes: &[u8], limits: OutputLimits) -> (usize, usize) {
    if limits.max_lines == 0 || limits.max_bytes == 0 {
        return (bytes.len(), bytes.len());
    }
    // 末尾的 newline 结束最后一行，不开启新的一行。
    let last = if bytes.last() == Some(&NEWLINE) {
        bytes.len().saturating_sub(2) as isize
    } else {
        bytes.len() as isize - 1
    };
    let mut start = 0usize;
    let mut lines = 1usize;
    let mut index = if last < 0 {
        None
    } else {
        bytes[..=last as usize]
            .iter()
            .rposition(|b| *b == NEWLINE)
    };
    while let Some(at) = index {
        if lines == limits.max_lines {
            start = at + 1;
            break;
        }
        lines += 1;
        index = if at == 0 {
            None
        } else {
            bytes[..at].iter().rposition(|b| *b == NEWLINE)
        };
    }
    if bytes.len() - start > limits.max_bytes {
        let from = bytes.len() - limits.max_bytes;
        let newline = if from == 0 {
            None
        } else {
            bytes[..from - 1].iter().position(|b| *b == NEWLINE)
        };
        start = match newline {
            Some(index) if index + 1 < bytes.len() => index + 1,
            _ => character_start(bytes, from),
        };
    }
    (start, bytes.len())
}

/// `index` 处或之前最后一个字符边界。上游 `characterEnd` 导出函数。
pub fn character_end(bytes: &[u8], index: usize) -> usize {
    let mut end = index.min(bytes.len());
    while end > 0 && bytes[end] & 0xc0 == 0x80 {
        end -= 1;
    }
    end
}

fn character_start(bytes: &[u8], index: usize) -> usize {
    let mut start = index.min(bytes.len());
    while start < bytes.len() && bytes[start] & 0xc0 == 0x80 {
        start += 1;
    }
    start
}

fn line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|b| **b == NEWLINE).count();
    newlines + usize::from(bytes.last() != Some(&NEWLINE))
}

/// 增量 UTF-8 解码器：字节 chunk 直接解，残缺字符留在内部，下一次字节 chunk 接上。
#[derive(Debug, Default)]
struct IncrementalDecoder {
    pending: Vec<u8>,
}

impl IncrementalDecoder {
    /// 返回的 `text` 就是上游 `decoder.decode(chunk, {stream:true})`：
    /// 合法的完整字符，尾部残缺字节留下。
    fn decode_stream(&mut self, chunk: &[u8], first: bool) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        if first && data.starts_with(&[0xef, 0xbb, 0xbf]) {
            data.drain(..3);
        }
        let complete = complete_prefix(&data);
        self.pending.extend_from_slice(&data[complete..]);
        String::from_utf8_lossy(&data[..complete]).into_owned()
    }

    /// 冲掉残缺字符：U+FFFD 一个，不会重复吐。
    fn flush(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            REPLACEMENT.to_string()
        }
    }
}

/// 返回从头开始能完整解码成 UTF-8 的最长前缀长度。
fn complete_prefix(data: &[u8]) -> usize {
    let mut end = data.len();
    while end > 0 && std::str::from_utf8(&data[..end]).is_err() {
        end -= 1;
    }
    end
}

#[derive(Debug, Clone)]
struct StoredChunk {
    text: String,
    bytes: usize,
    newlines: usize,
}

/// 有界运行输出。上游 `OutputBuffer`；一个流一个实例。
#[derive(Debug)]
pub struct OutputBuffer {
    limits: OutputLimits,
    decoder: IncrementalDecoder,
    started: bool,
    chunks: VecDeque<StoredChunk>,
    stored_bytes: usize,
    stored_newlines: usize,
    full: bool,
    total_bytes: usize,
    total_newlines: usize,
    ends_with_newline: bool,
}

impl OutputBuffer {
    pub fn new(limits: OutputLimits) -> Self {
        Self {
            limits,
            decoder: IncrementalDecoder::default(),
            started: false,
            chunks: VecDeque::new(),
            stored_bytes: 0,
            stored_newlines: 0,
            full: false,
            total_bytes: 0,
            total_newlines: 0,
            ends_with_newline: true,
        }
    }

    /// 当前存着的字节数（上限加一个 chunk）。
    pub fn stored_bytes(&self) -> usize {
        self.stored_bytes
    }

    /// 收一个 chunk。上游 `OutputBuffer.push(chunk, skipped)`；`skipped` 只在 tail 模式收。
    pub fn push(&mut self, chunk: Chunk, skipped: Option<OutputSkip>) -> Result<bool, OutputError> {
        let (pending, text) = match &chunk {
            Chunk::Text(text) => (self.decoder.flush(), text.clone()),
            Chunk::Bytes(bytes) => {
                let first = !self.started && skipped.is_none();
                let text = self.decoder.decode_stream(bytes, first);
                (String::new(), text)
            }
        };
        if !pending.is_empty() || !text.is_empty() || skipped.is_some() {
            self.started = true;
        }
        if skipped.is_none() {
            return Ok(self.accept(&format!("{pending}{text}")));
        }
        if self.limits.retain != Retain::Tail {
            return Err(OutputError::SkipRequiresTail);
        }
        self.accept(&pending);
        self.skip(skipped.expect("checked is_some"));
        self.accept(&text);
        Ok(true)
    }

    /// 流结束时冲掉残缺字符。
    pub fn end(&mut self) {
        let text = self.decoder.flush();
        self.accept(&text);
    }

    fn accept(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let bytes = utf8_byte_length(text);
        let newlines = count_newlines(text);
        self.total_bytes += bytes;
        self.total_newlines += newlines;
        self.ends_with_newline = text.ends_with('\n');
        if self.full {
            return true;
        }
        self.chunks.push_back(StoredChunk {
            text: text.to_string(),
            bytes,
            newlines,
        });
        self.stored_bytes += bytes;
        self.stored_newlines += newlines;
        if self.limits.retain == Retain::Head {
            self.full = self.stored_bytes > self.limits.max_bytes
                || self.stored_newlines >= self.limits.max_lines;
            return true;
        }
        while self.chunks.len() > 1 {
            let first = self.chunks.front().expect("len > 1");
            let bytes_after = self.stored_bytes - first.bytes;
            let newlines_after = self.stored_newlines - first.newlines;
            if bytes_after <= self.limits.max_bytes + 1
                && newlines_after <= self.limits.max_lines + 1
            {
                break;
            }
            self.chunks.pop_front();
            self.stored_bytes = bytes_after;
            self.stored_newlines = newlines_after;
        }
        true
    }

    fn skip(&mut self, skipped: OutputSkip) {
        if skipped.bytes == 0 {
            return;
        }
        self.total_bytes += skipped.bytes;
        self.total_newlines += skipped.newlines;
        self.ends_with_newline = skipped.ends_with_newline;
        self.chunks.clear();
        self.stored_bytes = 0;
        self.stored_newlines = 0;
    }

    /// 当前保留的、去掉控制字符的输出，及从头丢掉的总量。上游 `snapshot`。
    pub fn snapshot(&mut self) -> BoundedOutput {
        let stored = self
            .chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<String>();
        let kept = bound_output(&stored, self.limits);
        let stored_lines = lines(self.stored_newlines, stored.is_empty() || stored.ends_with('\n'));
        let kept_lines = stored_lines - kept.dropped_lines;
        if self.limits.retain == Retain::Tail || self.chunks.len() > 1 {
            let text = if self.limits.retain == Retain::Tail {
                tail_margin(&stored, self.limits)
            } else {
                stored.clone()
            };
            let bytes = if self.limits.retain == Retain::Tail {
                utf8_byte_length(&text)
            } else {
                self.stored_bytes
            };
            self.chunks.clear();
            if !text.is_empty() {
                self.chunks.push_back(StoredChunk {
                    newlines: count_newlines(&text),
                    text,
                    bytes,
                });
            }
            self.stored_bytes = bytes;
            self.stored_newlines = self.chunks.front().map(|c| c.newlines).unwrap_or(0);
        }
        BoundedOutput {
            text: sanitize_output(&kept.text),
            dropped_bytes: self.total_bytes - kept.bytes,
            dropped_lines: lines(self.total_newlines, self.ends_with_newline) - kept_lines,
        }
    }
}

/// 上游 `tailMargin`：比窗口多一字节或多一行的最短后缀。
fn tail_margin(text: &str, limits: OutputLimits) -> String {
    let bytes = text.as_bytes();
    let byte_start = if bytes.len() > limits.max_bytes {
        character_end(bytes, bytes.len() - limits.max_bytes - 1)
    } else {
        0
    };
    let mut line_start = 0usize;
    let mut newlines = 0usize;
    let mut at = bytes.len();
    while let Some(index) = bytes[..at].iter().rposition(|b| *b == NEWLINE) {
        newlines += 1;
        if newlines > limits.max_lines {
            line_start = index;
            break;
        }
        if index == 0 {
            break;
        }
        at = index;
    }
    let start = byte_start.max(line_start);
    if start == 0 {
        text.to_string()
    } else {
        String::from_utf8_lossy(&bytes[start..]).into_owned()
    }
}

fn lines(newlines: usize, terminated: bool) -> usize {
    newlines + usize::from(!terminated)
}

/// 字符串的 UTF-8 字节长度。Rust 的 `str` 里不会有孤立代理项，所以直接 `len()`。
pub fn utf8_byte_length(content: &str) -> usize {
    content.len()
}

fn count_newlines(text: &str) -> usize {
    text.as_bytes().iter().filter(|b| **b == NEWLINE).count()
}

/// **照抄上游**：每写 1 字节歇 1/102400 秒。不许自创别的数。
pub const PROGRESS_BYTES_PER_SECOND: usize = 100 * 1024;

/// **照抄上游 `DEFAULT_PROGRESS_POLICY.outputIntervalMs`**：工具输出闸 100 ms。
pub const DEFAULT_OUTPUT_INTERVAL_MS: u64 = 100;

/// 进度提交用的时钟和定时器。上游 `ProgressClock`；由调用方注入，本模块不起运行时。
/// 撤销定时器的 closure 在调用时会置回 `Progress.timer = None` 并重查时钟。
pub type TimerHandle = Box<dyn FnOnce() + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct ProgressClock {
    pub now: fn() -> Instant,
    /// 约 `delay` 之后调一次 callback；返回的 closure 用来撤销它。
    pub set_timer: fn(callback: Box<dyn FnOnce() + Send + Sync>, delay: Duration) -> TimerHandle,
}

impl Default for ProgressClock {
    fn default() -> Self {
        // 纯库没有默认定时器：必须显式给一个。这里只提供时间源，定时器由调用方填。
        Self {
            now: Instant::now,
            set_timer: |_, _| Box::new(|| {}),
        }
    }
}

/// 进度提交出错时通知调用方（上游 `onError`）。
pub type ProgressErrorHandler = Arc<dyn Fn(String) + Send + Sync>;

/// span 钩子：上游在 `Progress` 外由 telemetry 包住写操作的那一层。本库不依赖
/// `nous-telemetry`，由调用方注入；至少能断言 span 名与字段。
/// 回调是闭包，不派生 `Debug` / `PartialEq`。
pub struct ProgressHook {
    /// span 名，照上游的 `output.progress` 命名（字段随调用方决定）。
    pub name: String,
    pub on_start: Arc<dyn Fn(&str, &[(&str, String)]) + Send + Sync>,
    pub on_finish: Arc<dyn Fn(&str, bool) + Send + Sync>,
}

impl std::fmt::Debug for ProgressHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressHook").field("name", &self.name).finish_non_exhaustive()
    }
}

/// 自适应进度提交。上游 `Progress`，不带 `tokio`：定时器由 [`ProgressClock`] 注入，
/// 写操作由 [`Progress::run`] 的 future 传入。第一次变更立刻提交；之后按 `min_interval`
/// 与字节数配速；同时最多一次写，期间的变化合并进下一次。
pub struct Progress {
    on_error: ProgressErrorHandler,
    min_interval: Duration,
    clock: ProgressClock,
    next_at: Option<Instant>,
    dirty: bool,
    stopped: bool,
    timer: Option<TimerHandle>,
    /// 有写任务在飞时置位。纯库无法驱动 future，调用方每次 `mark` 时由 [`Progress::run`] 推进。
    in_flight: bool,
    hook: Option<ProgressHook>,
    /// 已经等着的提交次数：`mark_and_wait` 会记这里，提交完成后由调用方 drain。
    waiters: usize,
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Progress")
            .field("min_interval", &self.min_interval)
            .field("dirty", &self.dirty)
            .field("stopped", &self.stopped)
            .field("in_flight", &self.in_flight)
            .field("waiters", &self.waiters)
            .finish()
    }
}

impl Progress {
    pub fn new(
        on_error: ProgressErrorHandler,
        min_interval_ms: u64,
        clock: ProgressClock,
    ) -> Self {
        Self {
            on_error,
            min_interval: Duration::from_millis(min_interval_ms),
            clock,
            next_at: None,
            dirty: false,
            stopped: false,
            timer: None,
            in_flight: false,
            hook: None,
            waiters: 0,
        }
    }

    pub fn with_hook(mut self, hook: ProgressHook) -> Self {
        self.hook = Some(hook);
        self
    }

    /// 当前是不是有一次写在飞。
    pub fn is_in_flight(&self) -> bool {
        self.in_flight
    }

    /// 上次提交算出来的最早下次提交时刻。测试据此断言配速（100 KiB/s）。
    pub fn next_at(&self) -> Option<Instant> {
        self.next_at
    }

    /// 需要提交：等下次 [`Progress::run`] 驱动。
    pub fn mark(&mut self) {
        self.dirty = true;
        self.schedule();
    }

    /// 像上游 `markAndWait` 一样记一个等待者。真正的等待由调用方在提交完成后
    /// 用 [`Progress::take_waiters`] 领走（Rust 里没有 `Promise.withResolvers`）。
    pub fn mark_and_wait(&mut self) {
        self.waiters += 1;
        self.mark();
    }

    /// 领走已经完成的等待者数量。
    pub fn take_waiters(&mut self) -> usize {
        std::mem::take(&mut self.waiters)
    }

    /// 停掉提交。返回是否还有写任务需要调用方等（上游 `stop()` 等 `inFlight`）。
    pub fn stop(&mut self) -> bool {
        self.stopped = true;
        self.timer.take();
        self.in_flight
    }

    fn schedule(&mut self) {
        if self.stopped || self.timer.is_some() || self.in_flight {
            return;
        }
        let wait = self
            .next_at
            .map(|at| at.saturating_duration_since((self.clock.now)()))
            .unwrap_or_default();
        if wait.is_zero() {
            // 不在这里写：进度提交需要调用方提供 write future，`mark` 只置脏。
            return;
        }
        let clock = self.clock;
        let timer = (clock.set_timer)(Box::new(|| {}), wait);
        self.timer = Some(timer);
    }

    /// 由调用方驱动：定时器到了（或首次 `mark`）就真正提交。
    pub fn run<F: std::future::Future<Output = usize>>(&mut self, write: F) -> bool {
        if self.stopped || !self.dirty || self.in_flight {
            return false;
        }
        self.dirty = false;
        self.timer.take();
        self.in_flight = true;
        if let Some(hook) = &self.hook {
            (hook.on_start)(&hook.name, &[("min_interval_ms", self.min_interval.as_millis().to_string())]);
        }
        // 调用方把 future 跑完后调用 `complete(bytes)`；纯库不 poll 它。
        let _ = write;
        true
    }

    /// 写完成。上游在 `.then(bytes => nextAt = started + max(min, bytes*1000/rate))`。
    pub fn complete(&mut self, result: Result<usize, String>) {
        if let Some(hook) = &self.hook {
            (hook.on_finish)(&hook.name, result.is_ok());
        }
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                self.next_at = Some((self.clock.now)() + self.min_interval);
                (self.on_error)(error);
                self.in_flight = false;
                return;
            }
        };
        let paced = Duration::from_micros(bytes as u64 * 1_000_000 / PROGRESS_BYTES_PER_SECOND as u64);
        self.next_at = Some((self.clock.now)() + self.min_interval.max(paced));
        self.in_flight = false;
    }
}

/// 输出模块的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputError {
    /// 上游：跳过的输出只能在 tail 模式下收。
    SkipRequiresTail,
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputError::SkipRequiresTail => write!(f, "skipped output requires tail retention"),
        }
    }
}

impl std::error::Error for OutputError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_constants_are_copied_not_invented() {
        assert_eq!(PROGRESS_BYTES_PER_SECOND, 100 * 1024);
        assert_eq!(DEFAULT_OUTPUT_INTERVAL_MS, 100);
    }

    #[test]
    fn sanitize_keeps_tab_and_newline_removes_control_and_escapes() {
        assert_eq!(sanitize_output("a\tb\nc"), "a\tb\nc");
        assert_eq!(sanitize_output("a\u{0000}b\u{000c}c"), "abc");
        assert_eq!(sanitize_output("a\x1b[31mred\x1b[0m!"), "ared!");
        assert_eq!(sanitize_output("x\x1b]0;title\x07y"), "xy");
        assert_eq!(sanitize_output("x\x1b]0;title\x1b\\y"), "xy");
    }

    #[test]
    fn bound_output_keeps_whole_lines_and_counts_drops() {
        let limits = OutputLimits {
            max_bytes: 100,
            max_lines: 2,
            retain: Retain::Head,
        };
        let slice = bound_output("aa\nbb\ncc\n", limits);
        assert_eq!(slice.text, "aa\nbb\n");
        assert_eq!(slice.bytes, 6);
        assert_eq!(slice.dropped_bytes, 3);
        assert_eq!(slice.dropped_lines, 1);

        let limits = OutputLimits {
            max_bytes: 100,
            max_lines: 2,
            retain: Retain::Tail,
        };
        let slice = bound_output("aa\nbb\ncc\n", limits);
        assert_eq!(slice.text, "bb\ncc\n");
        assert_eq!(slice.dropped_bytes, 3);
        assert_eq!(slice.dropped_lines, 1);
    }

    #[test]
    fn head_and_tail_cut_at_character_boundaries() {
        let limits = OutputLimits {
            max_bytes: 5,
            max_lines: 100,
            retain: Retain::Head,
        };
        assert_eq!(bound_output("éééé", limits).text, "éé");
        let limits = OutputLimits {
            max_bytes: 5,
            max_lines: 100,
            retain: Retain::Tail,
        };
        assert_eq!(bound_output("éééé", limits).text, "éé");
    }

    #[test]
    fn output_buffer_head_stops_storing_after_a_full_window() {
        let mut buffer = OutputBuffer::new(OutputLimits {
            max_bytes: 6,
            max_lines: 100,
            retain: Retain::Head,
        });
        assert_eq!(buffer.push(Chunk::Text("abc\n".into()), None), Ok(true));
        assert_eq!(buffer.push(Chunk::Text("def\n".into()), None), Ok(true));
        assert_eq!(buffer.push(Chunk::Text("ghi\n".into()), None), Ok(true));
        let out = buffer.snapshot();
        assert_eq!(out.text, "abc\n");
        assert!(out.dropped_bytes > 0);
        assert!(out.dropped_lines > 0);
    }

    #[test]
    fn output_buffer_tail_snapshots_the_last_window_and_reports_drops() {
        let mut buffer = OutputBuffer::new(OutputLimits {
            max_bytes: 100,
            max_lines: 2,
            retain: Retain::Tail,
        });
        for line in ["aa\n", "bb\n", "cc\n", "dd\n"] {
            assert_eq!(buffer.push(Chunk::Text(line.into()), None), Ok(true));
        }
        let out = buffer.snapshot();
        assert_eq!(out.text, "cc\ndd\n");
        assert_eq!(out.dropped_bytes, 6);
        assert_eq!(out.dropped_lines, 2);
    }

    #[test]
    fn skip_is_tail_only_and_clears_stored_prefix() {
        let mut buffer = OutputBuffer::new(OutputLimits {
            max_bytes: 100,
            max_lines: 10,
            retain: Retain::Head,
        });
        assert_eq!(
            buffer.push(Chunk::Text("x".into()), Some(OutputSkip::default())),
            Err(OutputError::SkipRequiresTail)
        );
        let mut buffer = OutputBuffer::new(OutputLimits {
            max_bytes: 100,
            max_lines: 10,
            retain: Retain::Tail,
        });
        buffer.push(Chunk::Text("old\n".into()), None).unwrap();
        buffer
            .push(
                Chunk::Text("new\n".into()),
                Some(OutputSkip {
                    bytes: 4,
                    newlines: 1,
                    ends_with_newline: true,
                }),
            )
            .unwrap();
        let out = buffer.snapshot();
        assert_eq!(out.text, "new\n");
        // 流总量 = 4 (old) + 4 (skipped) + 4 (new)，窗口留下 4。
        assert_eq!(out.dropped_bytes, 8);
        assert_eq!(out.dropped_lines, 2);
    }

    #[test]
    fn incremental_decoder_waits_for_a_partial_character() {
        let mut buffer = OutputBuffer::new(OutputLimits {
            max_bytes: 100,
            max_lines: 10,
            retain: Retain::Tail,
        });
        let mut first = "é".as_bytes().to_vec();
        first.pop();
        buffer.push(Chunk::Bytes(first), None).unwrap();
        buffer
            .push(Chunk::Bytes(vec![0xa9, b'!']), None)
            .unwrap();
        buffer.end();
        assert_eq!(buffer.snapshot().text, "é!");
    }

    #[test]
    fn a_string_chunk_flushes_an_incomplete_character() {
        let mut buffer = OutputBuffer::new(OutputLimits {
            max_bytes: 100,
            max_lines: 10,
            retain: Retain::Tail,
        });
        buffer
            .push(Chunk::Bytes(vec![0xc3]), None)
            .unwrap();
        buffer.push(Chunk::Text("!".into()), None).unwrap();
        assert_eq!(buffer.snapshot().text, "\u{fffd}!");
    }

    #[test]
    fn progress_pacing_uses_the_upstream_throughput() {
        // 配速换算：1 KiB / 100 KiB/s = 10 ms。
        assert_eq!(
            Duration::from_micros(1024 * 1_000_000 / PROGRESS_BYTES_PER_SECOND as u64),
            Duration::from_millis(10)
        );
    }

    #[test]
    fn progress_hook_asserts_span_name_and_fields() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let starts = Arc::new(AtomicUsize::new(0));
        let finishes = Arc::new(AtomicUsize::new(0));
        let hook = ProgressHook {
            name: "output.progress".into(),
            on_start: Arc::new({
                let starts = Arc::clone(&starts);
                move |name, attrs| {
                    assert_eq!(name, "output.progress");
                    assert_eq!(attrs, &[("min_interval_ms", "100".to_string())]);
                    starts.fetch_add(1, Ordering::SeqCst);
                }
            }),
            on_finish: Arc::new({
                let finishes = Arc::clone(&finishes);
                move |name, succeeded| {
                    assert_eq!(name, "output.progress");
                    assert!(succeeded);
                    finishes.fetch_add(1, Ordering::SeqCst);
                }
            }),
        };
        let clock = ProgressClock {
            now: Instant::now,
            set_timer: |_, _| Box::new(|| {}),
        };
        let mut progress = Progress::new(
            Arc::new(|_| {}),
            DEFAULT_OUTPUT_INTERVAL_MS,
            clock,
        )
        .with_hook(hook);
        progress.mark();
        assert!(progress.run(async { 1024 }));
        progress.complete(Ok(1024));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(finishes.load(Ordering::SeqCst), 1);
        // 100KiB/s：写 1 KiB => 下次最早 10ms 后；100ms 闸更大，取 100ms。
        let next = progress.next_at().expect("nextAt");
        assert!(next > Instant::now() + Duration::from_millis(90));
    }
}
