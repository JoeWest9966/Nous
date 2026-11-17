//! 增量、容错的 JSON 解析：`append()` 一次读一遍文本，`snapshot()` 给出「读到现在」的值，
//! 同时报告这一片文本造成的 `JsonChange`（路径 + 值/文本）。
//!
//! 提货自 `pi@v1.5.2` `packages/ai/src/utils/streaming-json.ts`（559 行），**只取四个导出**：
//! `JsonPath` / `JsonChange` / `StreamingJsonParser` / `ToolCallArgumentsStream`。
//! 行为规则见上游模块注释，这里只写与上游的差别：
//!
//! - **对象键按字典序**：`serde_json::Value::Object` 是 `BTreeMap`，上游 JS 对象保插入序。
//!   值相等比较不受影响；影响的是序列化后的键顺序。工具参数是给人看的预览，按序无意义。
//! - **`snapshot()` 是深拷贝**：上游浅拷贝「开放路径」上的容器、共享已完成的容器；
//!   Rust 的 `Value` 没有共享结构，只能整棵拷贝。因此复制额度（credit）记账只是近似 ——
//!   它保证「不会因为一个巨大的开放容器把每次快照都变贵」，但省不下共享本身。
//! - **`\uXXXX` 代理对**：上游会把半个代理对当字符立刻填进文本（JS 字符串容忍孤立代理）；
//!   Rust 的 `String` 容不下，本库把一个 `\uD800-\uDBFF` 挂起等下一个转义，凑成真字符，
//!   落单的写 `U+FFFD`。完整文本的最终值一致。
//! - **`__proto__` 那条特例不需要**（Rust 没有原型链）：`set_member` 直接插。
//!
//! 与 `crate::json_parse::parse_streaming_json` 的分工：那个是**容错前缀解析**（每次整段重解，
//! 给「现在读到哪」）；这个是**增量解析器**（一次读一遍，给「这一片改了哪个路径」）。
//! 拿新旧快照做 diff 也能得到 change，但那是 O(n²)，而且会把「同值重复出现」误判成 change ——
//! `nous-durable` 的 `PartialChanges` 靠的正是「只报真的变了的地方」。

use serde_json::{Map, Number, Value};

/// 路径上的一段：对象成员键或数组下标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// 从根值出发的路径：成员键与数组下标。
pub type JsonPath = Vec<PathSegment>;

/// 一片文本对值做的改动。
#[derive(Clone, Debug, PartialEq)]
pub enum JsonChange {
    /// 这个位置现在是这个值（新键、新元素、或一个刚出现的标量）。
    Set { path: JsonPath, value: Value },
    /// 这个位置的字符串延长了 `text`。
    Append { path: JsonPath, text: String },
}

impl JsonChange {
    /// 改动落在哪。
    pub fn path(&self) -> &[PathSegment] {
        match self {
            Self::Set { path, .. } | Self::Append { path, .. } => path,
        }
    }

    /// 是不是改动根值本身（工具参数的根必须是个对象，`PartialChanges` 据此判断「这一段不算参数」）。
    pub fn is_root(&self) -> bool {
        self.path().is_empty()
    }

    /// 是不是「追加文本」。**只有它**能编码成 `toolcall_delta`（见 `docs/harness.md` §2.2）。
    pub fn is_append(&self) -> bool {
        matches!(self, Self::Append { .. })
    }
}

/// 路径上的一段，方便构造期望值。
pub fn key(name: &str) -> PathSegment {
    PathSegment::Key(name.to_string())
}

/// 数组下标那一段。
pub fn index(index: usize) -> PathSegment {
    PathSegment::Index(index)
}

/// 报告改动的最深路径：再深的容器关闭时整块报一次 `Set`，路径才不会长。
const MAX_REPORTED_DEPTH: usize = 32;
/// 每次 `append()` 挣的复制额度与每字符额度；下限保证小文档每次都新鲜。
const CREDIT_PER_APPEND: usize = 256;
const CREDIT_PER_CHARACTER: usize = 8;
/// 未结束的数字/字面量超过这个长度就先不显示，`snapshot()` 才不会变贵。
const MAX_PENDING_SCALAR_LENGTH: usize = 64;
const LITERALS: [(&str, fn() -> Value); 3] = [
    ("true", || Value::Bool(true)),
    ("false", || Value::Bool(false)),
    ("null", || Value::Null),
];

/// 一个值在父容器里的位置；`None` = 这个位置不接受值（读到了但不收）。
#[derive(Clone, Debug)]
enum Slot {
    Root,
    Member(String),
    Element(usize),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Key,
    Colon,
    Value,
    Comma,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    Key,
    Value,
}

/// 一个打开的对象或数组。`container` 在打开期间不交给调用方；快照拿的是副本。
struct Frame {
    container: Value,
    slot: Option<Slot>,
    /// 它坐在哪，当它的改动要报告时。
    path: Option<JsonPath>,
    /// 里面的改动报不报；不报时它关闭时整块报一次值。
    reports_inside: bool,
    /// 成员数或元素数：拷一份容器的成本。
    size: usize,
    /// 对象：最后读到的键，成员值往这里放。
    key: Option<String>,
    phase: Phase,
}

/// 一个没结束的字符串。
struct OpenString {
    target: Target,
    slot: Option<Slot>,
    text: String,
    /// 读到一半的转义：空 = 没有，否则是一个反斜杠加最多五个字符。
    escape: String,
    /// 一个值字符串坐在哪，当它的改动要报告时。
    path: Option<JsonPath>,
    /// 这个字符串延长的那条改动，当它还是本片最新的改动时。
    change: Option<usize>,
    /// 半个代理对（`\uD800-\uDBFF`），等下一个转义决定拼成真字符还是落单。
    pending_high: Option<u32>,
}

/// 增量、容错的 JSON 解析器。规则与成本见模块注释。
#[derive(Default)]
pub struct StreamingJsonParser {
    root: Option<Value>,
    done: bool,
    stack: Vec<Frame>,
    string: Option<OpenString>,
    scalar: String,
    scalar_slot: Option<Slot>,
    /// 开放路径上所有 `size + 1` 之和：一份新快照要拷的量。
    path_cost: usize,
    credit: usize,
    dirty: bool,
    snapshot: Option<Value>,
    /// 正在读的这一片的改动。
    changes: Vec<JsonChange>,
}

impl StreamingJsonParser {
    /// 新建一个空解析器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 读下一片文本，按顺序返回它造成的改动。成本与 `text` 长度成线性。
    pub fn append(&mut self, text: &str) -> Vec<JsonChange> {
        let chars: Vec<char> = text.chars().collect();
        self.credit += CREDIT_PER_APPEND + CREDIT_PER_CHARACTER * chars.len();
        self.changes.clear();
        if let Some(open) = self.string.as_mut() {
            open.change = None;
        }
        let mut at = 0;
        while at < chars.len() && !self.done {
            if self.string.is_some() {
                at = self.read_string(at, &chars);
            } else if !self.scalar.is_empty() {
                at = self.read_scalar(at, &chars);
            } else {
                self.structural(chars[at]);
                at += 1;
            }
        }
        std::mem::take(&mut self.changes)
    }

    /// 读到现在的值：值还没开始是 `None`，否则是一个之后不再变的 JSON 值。
    ///
    /// 当「自上次快照以来的文本没有改变值」，或者「一份新快照的复制额度还没挣够」时，
    /// 返回上一次的快照（见模块注释的成本段）。
    pub fn snapshot(&mut self) -> Option<Value> {
        if !self.dirty {
            return self.snapshot.clone();
        }
        let pending = self.pending_scalar();
        let cost = self.path_cost + usize::from(pending.is_some());
        if cost > self.credit {
            return self.snapshot.clone();
        }
        self.credit -= cost;
        self.dirty = false;
        let value = self.copy_open_path(pending);
        self.snapshot = value.clone();
        value
    }

    /// 从 `start` 开始读字符串字符，直到字符串结束或文本读完；返回下一个下标。
    fn read_string(&mut self, start: usize, chars: &[char]) -> usize {
        let mut open = match self.string.take() {
            Some(open) => open,
            None => return start,
        };
        let mut run_start = start;
        let mut at = start;
        while at < chars.len() {
            let ch = chars[at];
            if !open.escape.is_empty() {
                if self.continue_escape(&mut open, ch) {
                    run_start = at + 1;
                    at += 1;
                    continue;
                }
                // 不属于这个转义：下面当普通字符串字符读。
                run_start = at;
            }
            if ch == '"' {
                if at > run_start {
                    let run: String = chars[run_start..at].iter().collect();
                    self.append_string(&mut open, &run);
                }
                self.end_string(open);
                return at + 1;
            }
            if ch == '\\' {
                if at > run_start {
                    let run: String = chars[run_start..at].iter().collect();
                    self.append_string(&mut open, &run);
                }
                open.escape = "\\".to_string();
                run_start = at + 1;
            }
            at += 1;
        }
        if chars.len() > run_start {
            let run: String = chars[run_start..].iter().collect();
            self.append_string(&mut open, &run);
        }
        self.string = Some(open);
        chars.len()
    }

    /// 从 `start` 开始读数字/字面量字符到下一个分隔符；返回分隔符的下标。
    fn read_scalar(&mut self, start: usize, chars: &[char]) -> usize {
        let mut end = start;
        while end < chars.len() && !is_delimiter(chars[end]) {
            end += 1;
        }
        if end > start {
            self.scalar.extend(&chars[start..end]);
            if self.scalar_slot.is_some() {
                self.dirty = true;
            }
        }
        if end < chars.len() {
            self.end_scalar();
        }
        end
    }

    /// 处理一个字符串外的字符。
    fn structural(&mut self, ch: char) {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {}
            '"' => self.start_string(),
            '{' | '[' => {
                let container = if ch == '{' {
                    Value::Object(Map::new())
                } else {
                    Value::Array(Vec::new())
                };
                self.open_container(container);
            }
            '}' | ']' => self.close_container(),
            ':' => {
                if let Some(frame) = self.stack.last_mut() {
                    if matches!(frame.container, Value::Object(_)) && frame.phase == Phase::Colon {
                        frame.phase = Phase::Value;
                    }
                }
            }
            ',' => {
                if let Some(frame) = self.stack.last_mut() {
                    frame.phase = if matches!(frame.container, Value::Array(_)) {
                        Phase::Value
                    } else {
                        Phase::Key
                    };
                }
            }
            _ => {
                self.scalar = ch.to_string();
                self.scalar_slot = self.value_slot();
                if self.scalar_slot.is_some() {
                    self.dirty = true;
                }
            }
        }
    }

    /// 现在开始的值往哪放；这个位置不接受值是 `None`。
    fn value_slot(&self) -> Option<Slot> {
        let frame = match self.stack.last() {
            Some(frame) => frame,
            None => {
                return if self.root.is_none() {
                    Some(Slot::Root)
                } else {
                    None
                }
            }
        };
        if matches!(frame.container, Value::Array(_)) {
            let len = match &frame.container {
                Value::Array(items) => items.len(),
                _ => 0,
            };
            return Some(Slot::Element(len));
        }
        // 漏了冒号也容忍：`{"a" 1}` 读成 `{"a": 1}`。
        if matches!(frame.phase, Phase::Value | Phase::Colon) {
            if let Some(key) = frame.key.clone() {
                return Some(Slot::Member(key));
            }
        }
        None
    }

    fn start_string(&mut self) {
        let is_key_position = match self.stack.last() {
            Some(frame) => {
                matches!(frame.container, Value::Object(_))
                    && matches!(frame.phase, Phase::Key | Phase::Comma)
            }
            None => false,
        };
        if is_key_position {
            self.string = Some(OpenString {
                target: Target::Key,
                slot: None,
                text: String::new(),
                escape: String::new(),
                path: None,
                change: None,
                pending_high: None,
            });
            return;
        }
        let slot = self.value_slot();
        let path = self.reported_path(&slot);
        self.place(&slot, Value::String(String::new()));
        let mut open = OpenString {
            target: Target::Value,
            slot: slot.clone(),
            text: String::new(),
            escape: String::new(),
            path: path.clone(),
            change: None,
            pending_high: None,
        };
        if let Some(path) = path {
            let at = self.changes.len();
            self.changes.push(JsonChange::Set {
                path,
                value: Value::String(String::new()),
            });
            open.change = Some(at);
        }
        self.string = Some(open);
    }

    /// 最内层容器里 `slot` 那个位置的路径，当它的改动要报告时。
    fn reported_path(&self, slot: &Option<Slot>) -> Option<JsonPath> {
        let slot = slot.as_ref()?;
        if matches!(slot, Slot::Root) {
            return Some(Vec::new());
        }
        let frame = self.stack.last()?;
        if !frame.reports_inside {
            return None;
        }
        let mut path = frame.path.clone()?;
        match slot {
            Slot::Member(key) => path.push(PathSegment::Key(key.clone())),
            Slot::Element(index) => path.push(PathSegment::Index(*index)),
            Slot::Root => return Some(Vec::new()),
        }
        Some(path)
    }

    /// 继续一个转义；`ch` 不属于这个转义时返回 false（它要当普通字符读）。
    fn continue_escape(&mut self, open: &mut OpenString, ch: char) -> bool {
        if open.escape == "\\" {
            if let Some(decoded) = simple_escape(ch) {
                open.escape.clear();
                self.append_string(open, &decoded);
                return true;
            }
            if ch == 'u' {
                open.escape = "\\u".to_string();
                return true;
            }
            // 非法转义：反斜杠当文本留下，字符按普通字符读（与 `repair_json` 一致）。
            open.escape.clear();
            self.append_string(open, "\\");
            return false;
        }
        if ch.is_ascii_hexdigit() {
            open.escape.push(ch);
            if open.escape.len() < 6 {
                return true;
            }
            let unit = u32::from_str_radix(&open.escape[2..], 16).unwrap_or(0xFFFD);
            open.escape.clear();
            self.append_code_unit(open, unit);
            return true;
        }
        // `\u` 后面不足四个十六进制位：照样当文本留下，字符按普通字符读。
        let literal = std::mem::take(&mut open.escape);
        self.append_string(open, &literal);
        false
    }

    /// 追加一个 UTF-16 码元：代理对拼成真字符，落单的写 `U+FFFD`。
    fn append_code_unit(&mut self, open: &mut OpenString, unit: u32) {
        match unit {
            0xD800..=0xDBFF => {
                if let Some(high) = open.pending_high.replace(unit) {
                    let replacement = char::from_u32(high).unwrap_or('\u{FFFD}');
                    self.append_string(open, &replacement.to_string());
                    open.pending_high = Some(unit);
                }
            }
            0xDC00..=0xDFFF => match open.pending_high.take() {
                Some(high) => {
                    let combined = 0x10000 + ((high - 0xD800) << 10) + (unit - 0xDC00);
                    let decoded = char::from_u32(combined).unwrap_or('\u{FFFD}');
                    self.append_string(open, &decoded.to_string());
                }
                None => self.append_string(open, "\u{FFFD}"),
            },
            _ => {
                if open.pending_high.take().is_some() {
                    self.append_string(open, "\u{FFFD}");
                }
                let decoded = char::from_u32(unit).unwrap_or('\u{FFFD}');
                self.append_string(open, &decoded.to_string());
            }
        }
    }

    fn append_string(&mut self, open: &mut OpenString, text: &str) {
        if open.target != Target::Value {
            open.text.push_str(text);
            return;
        }
        open.text.push_str(text);
        let slot = open.slot.clone();
        self.place(&slot, Value::String(open.text.clone()));
        let path = match open.path.clone() {
            Some(path) => path,
            None => return,
        };
        // 这条改动还是最新一条时延长它，否则报一条 append。
        if let Some(at) = open.change {
            if at + 1 == self.changes.len() {
                match &mut self.changes[at] {
                    JsonChange::Set { value, .. } => match value {
                        Value::String(existing) => existing.push_str(text),
                        _ => *value = Value::String(open.text.clone()),
                    },
                    JsonChange::Append { text: existing, .. } => existing.push_str(text),
                }
                return;
            }
        }
        let at = self.changes.len();
        self.changes.push(JsonChange::Append {
            path,
            text: text.to_string(),
        });
        open.change = Some(at);
    }

    fn end_string(&mut self, mut open: OpenString) {
        if open.pending_high.take().is_some() {
            self.append_string(&mut open, "\u{FFFD}");
        }
        self.string = None;
        let frame_present = self.stack.last().is_some();
        if open.target == Target::Key {
            if !frame_present {
                return;
            }
            if let Some(frame) = self.stack.last_mut() {
                frame.key = Some(open.text);
                frame.phase = Phase::Colon;
            }
            return;
        }
        self.value_ended();
    }

    fn end_scalar(&mut self) {
        let token = std::mem::take(&mut self.scalar);
        let slot = self.scalar_slot.take();
        if let Some(value) = scalar_value(&token) {
            self.place(&slot, value.clone());
            let path = self.reported_path(&slot);
            if let Some(path) = path {
                self.changes.push(JsonChange::Set { path, value });
            }
        }
        self.value_ended();
    }

    /// 文本末尾那个没结束的标量显示出来的值，如果它显示的话。
    fn pending_scalar(&self) -> Option<(Slot, Value)> {
        let slot = self.scalar_slot.clone()?;
        if self.scalar.is_empty() || self.scalar.chars().count() > MAX_PENDING_SCALAR_LENGTH {
            return None;
        }
        let value = pending_scalar_value(&self.scalar)?;
        Some((slot, value))
    }

    fn open_container(&mut self, container: Value) {
        let slot = self.value_slot();
        let path = self.reported_path(&slot);
        self.place(&slot, container.clone());
        let reports_inside = match &path {
            Some(path) => path.len() < MAX_REPORTED_DEPTH,
            None => false,
        };
        let phase = if matches!(container, Value::Array(_)) {
            Phase::Value
        } else {
            Phase::Key
        };
        self.stack.push(Frame {
            container: container.clone(),
            slot,
            path: path.clone(),
            reports_inside,
            size: 0,
            key: None,
            phase,
        });
        self.path_cost += 1;
        self.dirty = true;
        // 一个刚打开的容器：干活的那个会一直变，改动里的那个不会。
        if let Some(path) = path {
            self.changes.push(JsonChange::Set {
                path,
                value: container,
            });
        }
    }

    fn close_container(&mut self) {
        let frame = match self.stack.pop() {
            Some(frame) => frame,
            None => return,
        };
        self.path_cost -= frame.size + 1;
        self.dirty = true;
        // 太深、里面不报：整块报一次值，此时它已经关闭，值不会再变。
        if let Some(path) = frame.path.clone() {
            if !frame.reports_inside {
                self.changes.push(JsonChange::Set {
                    path,
                    value: frame.container.clone(),
                });
            }
        }
        // 打开的这段时间里改动都落在 `frame.container` 上；关闭时把它放回父容器（或根）。
        // JS 那边靠对象引用（父容器和 frame 是同一个对象），Rust 的 `Value` 是值语义，必须显式回填。
        if let Some(slot) = frame.slot.clone() {
            self.place(&Some(slot), frame.container);
        }
        self.value_ended();
    }

    /// 一个值结束了：在没有容器时就是根值结束。
    fn value_ended(&mut self) {
        match self.stack.last_mut() {
            None => {
                // 根值之后的文本忽略，但只在根值已经存在时。
                if self.root.is_some() {
                    self.done = true;
                }
            }
            Some(frame) => frame.phase = Phase::Comma,
        }
    }

    /// 把 `slot` 那个位置设成 `value`；位置不接受值时什么都不做。
    fn place(&mut self, slot: &Option<Slot>, value: Value) {
        let slot = match slot {
            Some(slot) => slot,
            None => return,
        };
        self.dirty = true;
        match slot {
            Slot::Root => self.root = Some(value),
            Slot::Element(index) => {
                let frame = match self.stack.last_mut() {
                    Some(frame) => frame,
                    None => return,
                };
                if let Value::Array(items) = &mut frame.container {
                    if *index == items.len() {
                        frame.size += 1;
                        self.path_cost += 1;
                        items.push(value);
                    } else if *index < items.len() {
                        items[*index] = value;
                    }
                }
            }
            Slot::Member(key) => {
                let frame = match self.stack.last_mut() {
                    Some(frame) => frame,
                    None => return,
                };
                if let Value::Object(members) = &mut frame.container {
                    if !members.contains_key(key) {
                        frame.size += 1;
                        self.path_cost += 1;
                    }
                    members.insert(key.clone(), value);
                }
            }
        }
    }

    /// 一份新快照：把开放路径上的容器由内向外各拷一份，每份装上它那个开放子项的副本。
    fn copy_open_path(&self, pending: Option<(Slot, Value)>) -> Option<Value> {
        if self.stack.is_empty() {
            return match pending {
                Some((Slot::Root, value)) => Some(value),
                _ => self.root.clone(),
            };
        }
        let mut child: Option<Value> = None;
        let mut child_slot: Option<Slot> = None;
        for depth in (0..self.stack.len()).rev() {
            let frame = &self.stack[depth];
            let mut copied = match &frame.container {
                Value::Array(items) => Value::Array(items.clone()),
                Value::Object(members) => Value::Object(members.clone()),
                other => other.clone(),
            };
            if depth + 1 == self.stack.len() {
                if let Some((slot, value)) = &pending {
                    set_slot(&mut copied, slot, value.clone());
                }
            }
            if let (Some(value), Some(slot)) = (&child, &child_slot) {
                set_slot(&mut copied, slot, value.clone());
            }
            child = Some(copied);
            child_slot = frame.slot.clone();
        }
        child
    }
}

/// 把值放到容器的 `slot` 位置（快照拷贝时用）。
fn set_slot(container: &mut Value, slot: &Slot, value: Value) {
    match slot {
        Slot::Root => *container = value,
        Slot::Element(index) => {
            if let Value::Array(items) = container {
                if *index < items.len() {
                    items[*index] = value;
                } else {
                    items.push(value);
                }
            }
        }
        Slot::Member(key) => {
            if let Value::Object(members) = container {
                members.insert(key.clone(), value);
            }
        }
    }
}

/// 简单转义字符（不含 `\u`）。
fn simple_escape(ch: char) -> Option<String> {
    Some(
        match ch {
            '"' => "\"",
            '\\' => "\\",
            '/' => "/",
            'b' => "\u{8}",
            'f' => "\u{c}",
            'n' => "\n",
            'r' => "\r",
            't' => "\t",
            _ => return None,
        }
        .to_string(),
    )
}

/// 字符串外的分隔符。
fn is_delimiter(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t' | '\n' | '\r' | ',' | ':' | '{' | '}' | '[' | ']' | '"'
    )
}

/// 完整的数字文字，形状与上游那条正则一致。
fn is_number(token: &str) -> bool {
    let bytes = token.as_bytes();
    let mut at = 0;
    if at < bytes.len() && bytes[at] == b'-' {
        at += 1;
    }
    // 整数部分：`0` 或 `[1-9][0-9]*`
    if at >= bytes.len() {
        return false;
    }
    if bytes[at] == b'0' {
        at += 1;
    } else if bytes[at].is_ascii_digit() {
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
    } else {
        return false;
    }
    // 小数部分
    if at < bytes.len() && bytes[at] == b'.' {
        at += 1;
        if at >= bytes.len() || !bytes[at].is_ascii_digit() {
            return false;
        }
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
    }
    // 指数部分
    if at < bytes.len() && (bytes[at] == b'e' || bytes[at] == b'E') {
        at += 1;
        if at < bytes.len() && (bytes[at] == b'+' || bytes[at] == b'-') {
            at += 1;
        }
        if at >= bytes.len() || !bytes[at].is_ascii_digit() {
            return false;
        }
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
    }
    at == bytes.len()
}

/// 数字文字的 JSON 值：整数走 `i64`/`u64`（与 `serde_json::from_str` 一致），其余走 `f64`。
fn number_value(token: &str) -> Option<Value> {
    if !is_number(token) {
        return None;
    }
    let integer_only = !token.contains(['.', 'e', 'E']);
    if integer_only {
        if let Ok(n) = token.parse::<i64>() {
            return Some(Value::Number(Number::from(n)));
        }
        if let Ok(n) = token.parse::<u64>() {
            return Some(Value::Number(Number::from(n)));
        }
    }
    token
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map(Value::Number)
}

/// 完整的数字或字面量的值；其它一律丢弃。
fn scalar_value(token: &str) -> Option<Value> {
    for (literal, make) in LITERALS {
        if token == literal {
            return Some(make());
        }
    }
    number_value(token)
}

/// 没结束的数字或字面量显示出来的值。
fn pending_scalar_value(token: &str) -> Option<Value> {
    for (literal, make) in LITERALS {
        if literal.starts_with(token) {
            return Some(make());
        }
    }
    if let Some(value) = number_value(token) {
        return Some(value);
    }
    // `1e` / `1.5e+`：指数没写完，显示尾数。
    if let Some(mantissa) = strip_incomplete_exponent(token) {
        return number_value(mantissa);
    }
    None
}

/// 去掉「指数符号写了但没数字」的尾巴：`1.5e+` → `1.5`。
fn strip_incomplete_exponent(token: &str) -> Option<&str> {
    let trimmed = token.trim_end_matches(['+', '-']);
    let without_e = if trimmed.ends_with('e') || trimmed.ends_with('E') {
        &trimmed[..trimmed.len() - 1]
    } else {
        return None;
    };
    if without_e.is_empty() {
        return None;
    }
    Some(without_e)
}

/// 一条流式工具调用的参数：原始文本（结束时做唯一一次完整解析）加一个解析器（中间给部分参数）。
/// provider 在**自己的流状态**里每条调用存一个，绝不放在 content block 上 —— 原始文本不进口
/// partial message，也不进 transcript。
#[derive(Default)]
pub struct ToolCallArgumentsStream {
    text: String,
    parser: StreamingJsonParser,
    snapshot: Option<Value>,
    arguments: Value,
}

/// `replace()` 的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct Replaced {
    /// provider 补上的那一段；`None` 表示重来。
    pub delta: Option<String>,
    pub arguments: Value,
}

/// `finish()` 的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct FinishedArguments {
    pub arguments: Value,
    /// 参数不是合法 JSON 对象时，上游把错因与原文一起放这里，交工具校验回给模型。
    pub error: Option<String>,
}

impl ToolCallArgumentsStream {
    pub fn new() -> Self {
        Self {
            arguments: Value::Object(Map::new()),
            ..Self::default()
        }
    }

    /// 到目前为止收到的原始参数文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 追加一片参数文本；返回部分参数（变了才变）。
    pub fn append(&mut self, delta: &str) -> Value {
        self.text.push_str(delta);
        self.parser.append(delta);
        self.partial()
    }

    /// 取 provider 给的**完整**参数文本：接得上就返回缺的那一段，接不上就重来。
    pub fn replace(&mut self, text: &str) -> Replaced {
        if text.starts_with(&self.text) {
            let delta = text[self.text.len()..].to_string();
            if delta.is_empty() {
                return Replaced {
                    delta: Some(delta),
                    arguments: self.partial(),
                };
            }
            let arguments = self.append(&delta);
            return Replaced {
                delta: Some(delta),
                arguments,
            };
        }
        self.text.clear();
        self.parser = StreamingJsonParser::new();
        self.snapshot = None;
        let arguments = self.append(text);
        Replaced {
            delta: None,
            arguments,
        }
    }

    /// 最终参数：对完整文本做一次完整解析，参数不合法时把错因报出来。
    pub fn finish(&self) -> FinishedArguments {
        parse_tool_call_arguments(&self.text)
    }

    fn partial(&mut self) -> Value {
        let snapshot = self.parser.snapshot();
        if snapshot == self.snapshot {
            return self.arguments.clone();
        }
        self.snapshot = snapshot.clone();
        // 工具参数是个对象；任何别的东西都显示成「还没有参数」。
        self.arguments = match snapshot {
            Some(value @ Value::Object(_)) => value,
            _ => Value::Object(Map::new()),
        };
        self.arguments.clone()
    }
}

/// 完成后的参数文本解析：合不上就报 `{}` + 错因（错因里带原文）。
pub fn parse_tool_call_arguments(text: &str) -> FinishedArguments {
    if text.trim().is_empty() {
        return FinishedArguments {
            arguments: Value::Object(Map::new()),
            error: None,
        };
    }
    let parsed = serde_json::from_str::<Value>(text).or_else(|_| {
        serde_json::from_str::<Value>(&crate::json_parse::repair_json(text))
    });
    match parsed {
        Ok(value @ Value::Object(_)) => FinishedArguments {
            arguments: value,
            error: None,
        },
        Ok(_) => FinishedArguments {
            arguments: Value::Object(Map::new()),
            error: Some(format!(
                "Tool arguments must be a JSON object\n\nReceived arguments:\n{text}"
            )),
        },
        Err(error) => FinishedArguments {
            arguments: Value::Object(Map::new()),
            error: Some(format!("{error}\n\nReceived arguments:\n{text}")),
        },
    }
}
