//! 上游 `packages/durable/src/types.ts` 里真被 storage/session 用到的**记录**形状。
//!
//! 判据有两条，写在这里省得每次重讲：
//!
//! 1. **序列化出来的 JSON 名字与上游一字不差**。Rust 侧字段是 `snake_case`，靠
//!    `#[serde(rename_all = "camelCase")]` 落地；缺省的 `Option` 字段**整个键都不出现**
//!    （上游是 `readonly x?: T`，`undefined` 在 JSON 里不存在）。
//! 2. **取值域是枚举，不是字符串**。上游那些 `readonly status: "done"` 在 Rust 里落成 `enum`，
//!    拼错值和非法组合在**反序列化时**就报错，而不是等到某个 `if` 分支悄悄走错。
//!
//! 两处故意留成不透明 JSON（`docs/storage.md` §12.5）：`TaskRecord.state` / `TaskRecord.memos`
//! —— 本库没有 task runner，只存记录。`EntryRecord.model` 也是 `Value`：fixture 里 `pi.system`
//! 的 message 带 `toolsAdded`，用 `nous_ai::Message` 解析会把未知键丢掉，往返就不再逐字段相等。
//!
//! 验收见 `tests/records_fixture.rs`：这五种记录都从**真 fixture** 里读出来、再序列化回去，
//! 与源 JSON 逐字段相等（189 会话 / 209 entries / 128 tasks / 56 submissions / 961 文档）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Id, Seq};

/// 一条会话记录。上游 `ConversationRecord`。
///
/// 会话本身只记两件事：它从谁 fork 来（`parent`）、它归谁（`owner`）。名字、模板、模型这些
/// 展示字段在文档里，不在记录里 —— 别顺手加。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    pub id: Id,
    /// fork 源，以及历史从源会话的哪条 entry 起（含）。上游 `parent?: {conversationId, at}`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ConversationParent>,
    /// 创建者边：归属、子树 abort、子树 idle 等待都靠它。上游 `owner?: {conversationId, taskId}`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ConversationOwner>,
}

/// 会话的 fork 源。上游 `parent` 的形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationParent {
    pub conversation_id: Id,
    /// 历史从这条 entry 起（含）。
    pub at: Id,
}

/// 会话的创建者边。上游 `owner` 的形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationOwner {
    pub conversation_id: Id,
    pub task_id: Id,
}

/// 谁拥有这条会话。上游 `ConversationOwnership`。
///
/// 只在**创建**会话时出现，落库后存的是 `ConversationRecord::owner`（形状不同：那边是
/// `{conversationId, taskId}`，这边是 `{kind: "task", taskId}`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ConversationOwnership {
    /// 没有创建者任务（用户直接建的会话）。
    #[serde(rename = "ownerless")]
    Ownerless,
    /// 由某个任务建的会话。
    #[serde(rename = "task", rename_all = "camelCase")]
    Task { task_id: Id },
}

/// 一条 entry：不可变的转录事件。上游 `EntryRecord`。
///
/// `model` 是喂给模型的 message，`data` 是给视图/扩展看的 JSON —— 两者分开，互不影响。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRecord {
    pub id: Id,
    pub conversation_id: Id,
    /// 应用定义的 entry 判别键（`pi.user` / `pi.assistant` / `pi.tool-result` / `pi.system` …）。
    pub kind: String,
    /// 喂给模型上下文的 message；展示型或记账型 entry 没有这个字段。
    ///
    /// 保持 `Value`：`nous_ai::Message` 会丢掉未知键（fixture 里 `pi.system` 带 `toolsAdded`），
    /// 那样「读出来再写回去逐字段相等」这条判据就没了。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<Value>>,
    /// 给视图 / 扩展 / 记账用的 JSON。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// 这条 entry 选中的活动上下文的第一条 entry。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Id>,
    /// 对**更早**可见 entry 的上下文覆盖。上游 `edits?: readonly ContextEdit[]`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
    /// 追加这条 entry 的任务（durable 工作产出时才有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_task_id: Option<Id>,
    /// 这一轮为什么结束（issue #38，`/opt/Nous-spec/docs/spec/kernel-rust.md` ch5）。
    ///
    /// **一个字段、五值**，归因不靠第二种 entry kind：客户端只看「有没有终态」，
    /// 熔断 / 告警 / 启动扫孤儿各读一个字面量。缺席 = 还在跑（或这一条不是一轮的结局）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<TerminalReason>,
}

/// 一轮的终态。五个值，全在一处（ch5：一个字段五个值，不是两种 entry）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReason {
    /// 正常答完。
    Done,
    /// 客户端中止。
    UserCancel,
    /// 可重试错误、超时用尽后的归因。
    Timeout,
    /// 扩展进程死亡 / 熔断。
    ExtensionCrash,
    /// 整机崩 / 重启留下的孤儿，由启动扫描补写。
    Abandoned,
}

impl TerminalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::UserCancel => "user_cancel",
            Self::Timeout => "timeout",
            Self::ExtensionCrash => "extension_crash",
            Self::Abandoned => "abandoned",
        }
    }
}

/// 对一条更早的可见 entry 的**不可变**上下文覆盖。上游 `ContextEdit`。
///
/// 用内部标签 `action` 落成枚举，所以 `omit` 带 `messages`、`replace` 不带 `messages`
/// 这两种写法在反序列化时直接失败 —— 上游是 `messages?: never` / `messages: Message[]`，
/// 意思一样，只是 TS 靠类型、这里靠反序列化。
///
/// 手写 `Deserialize`：`serde` 的内部标签枚举**不认** `deny_unknown_fields`，靠 derive 的话
/// `{"action":"omit","target":9,"messages":[]}` 会被静默接受，`messages` 直接丢掉。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action")]
pub enum ContextEdit {
    /// 这条 entry 的 model message **整条省略**。
    #[serde(rename = "omit")]
    Omit { target: Id },
    /// 用 `messages` 替换这条 entry 的 model message。
    #[serde(rename = "replace")]
    Replace { target: Id, messages: Vec<Value> },
}

impl<'de> Deserialize<'de> for ContextEdit {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            target: Id,
            action: String,
            #[serde(default)]
            messages: Option<Vec<Value>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        match raw.action.as_str() {
            "omit" => match raw.messages {
                Some(_) => Err(serde::de::Error::custom(
                    "context edit omit must not carry messages",
                )),
                None => Ok(ContextEdit::Omit { target: raw.target }),
            },
            "replace" => {
                let messages = raw.messages.ok_or_else(|| {
                    serde::de::Error::custom("context edit replace requires messages")
                })?;
                Ok(ContextEdit::Replace {
                    target: raw.target,
                    messages,
                })
            }
            other => Err(serde::de::Error::unknown_variant(other, &["omit", "replace"])),
        }
    }
}

impl ContextEdit {
    /// 被覆盖的那条 entry。
    pub fn target(&self) -> Id {
        match self {
            ContextEdit::Omit { target } | ContextEdit::Replace { target, .. } => *target,
        }
    }
}

/// 一条 durable 任务的记录。上游 `TaskRecord<JsonValue, JsonValue, JsonValue>`。
///
/// **只存不跑**：本库没有 task runner（`docs/storage.md` §12.5）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: Id,
    pub conversation_id: Id,
    /// 注册过的任务定义名（fixture 里是 `pi.generation` / `pi.agent` 之类）。
    pub kind: String,
    /// 定义版本，用来迁移活着的 input 和 checkpoint。
    pub version: i64,
    /// 任务输入，活着或终态都留着。
    pub input: Value,
    /// 子任务的父任务；由会话自己拥有的任务没有这个字段。不可变。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Id>,
    /// 这个会话自带的任务是否被排进普通 idle 等待、会话 abort、级联之外。
    pub background: bool,
    /// durable abort 标记，跑模式推进前检查。
    pub abort_requested: bool,
    /// 不是 abort 请求而是别的原因设的标记：目前只有 `restart`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_reason: Option<AbortReason>,
    /// 来自 `TaskOptions.abandonOnRestart`，只在为 `true` 时出现。不可变。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_true",
        serialize_with = "serialize_true"
    )]
    pub abandon_on_restart: Option<bool>,
    /// 第一次进 `running` 的墙钟毫秒，由 session 盖章。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// 进 `terminal` 的墙钟毫秒；活着的任务没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// 连续被崩溃/驱逐/reset 掐断的次数，任务自己没换过状态才累加。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarts: Option<i64>,
    /// 任务状态机。**不透明 JSON**：本库不解释它（§12.5）。
    pub state: Value,
    /// 活着时保留的 first-writer-wins 小值。**不透明 JSON**（§12.5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memos: Option<Value>,
}

/// `abortReason` 的取值域。上游是字面量联合 `"restart"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AbortReason {
    /// 重启之后被放弃的任务（`TaskOptions.abandonOnRestart`）。
    Restart,
}

/// `abandonOnRestart` 上游是字面量 `true`：只接受 `true`，只写出 `true`。
fn deserialize_true<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    let value = bool::deserialize(d)?;
    if value {
        Ok(Some(true))
    } else {
        Err(serde::de::Error::custom(
            "abandonOnRestart is the literal true when present",
        ))
    }
}

fn serialize_true<S: serde::Serializer>(value: &Option<bool>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(true) => serializer.serialize_bool(true),
        // 不可达：`deserialize_true` 只产出 `Some(true)`，构造侧也只有 `Some(true)`。
        _ => Err(serde::ser::Error::custom(
            "abandonOnRestart is only ever true",
        )),
    }
}

/// 一条提交的生命周期记录。上游 `SubmissionRecord`（判别的 `type` × `status` 组合）。
///
/// 公共字段在这里，`type`/`status` 与其载荷在 [`SubmissionFields`] 里；这样 `SubmissionCreate`
/// （上游 `Omit<SubmissionRecord, "id">`）能原样复用同一份校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRecord {
    pub id: Id,
    #[serde(flatten)]
    pub fields: SubmissionFields,
}

/// 提交的判别字段：`type` + `status` + 该状态下才有的载荷。
///
/// 反序列化走 `try_from`，由 [`SubmissionFields::validate`] 按上游那张矩阵拒掉非法组合
/// （`continue` 不能 `queued`、`write` 不能 `placed`、`reason` 只能在 `unanswered` 出现…）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "SubmissionFieldsRepr")]
pub struct SubmissionFields {
    pub conversation_id: Id,
    /// 宿主给的去重键，作用域是会话。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// adapter 自己的追踪上下文；本库**原样存原样取**（§12.5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
    /// `input` / `write` / `continue`。
    #[serde(rename = "type")]
    pub kind: SubmissionType,
    pub status: SubmissionStatus,
    /// 这次提交对应的 entry。**只有 `input.unanswered` 允许缺席**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Id>,
    /// 回答这条输入的 assistant entry。只有 `done` 有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Id>,
    /// `unanswered` 的原因（`model_error` 之类）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 给 `unanswered` 的诊断载荷。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// 提交的 `type` 取值域。上游 `"input" | "write" | "continue"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmissionType {
    /// 用户输入（会被模型回答）。
    Input,
    /// 被动写入（已经是一条 entry，不等回答）。
    Write,
    /// 「继续」（没有新输入，只是催模型接着说话）。
    Continue,
}

/// 提交的 `status` 取值域。上游 `"queued" | "placed" | "done" | "unanswered"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmissionStatus {
    /// 已受理，还没进转录。
    Queued,
    /// 已放进转录，等回答。
    Placed,
    /// 有回答了。
    Done,
    /// 没有回答，带原因。
    Unanswered,
}

/// 反序列化的中转体：先按 TS 的宽形状收下来，再交给 [`SubmissionFields::validate`] 判矩阵。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmissionFieldsRepr {
    conversation_id: Id,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    trace: Option<Value>,
    #[serde(rename = "type")]
    kind: SubmissionType,
    status: SubmissionStatus,
    #[serde(default)]
    entry: Option<Id>,
    #[serde(default)]
    answer: Option<Id>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    detail: Option<Value>,
}

impl SubmissionFields {
    /// 上游 `types.ts` 那张 type × status 矩阵，一条一条写出来。
    ///
    /// | type | queued | placed | done | unanswered |
    /// |---|---|---|---|---|
    /// | input | 无 entry | 必带 entry | entry + answer | entry 可缺 + reason |
    /// | write | 无 entry | **不合法** | 必带 entry | **无** entry + reason |
    /// | continue | **不合法** | **无** entry | **无** entry + answer | **无** entry + reason |
    pub fn validate(&self) -> Result<(), String> {
        use SubmissionStatus::{Done, Placed, Queued, Unanswered};
        use SubmissionType::{Continue, Input, Write};

        let entry = self.entry;
        let answer = self.answer;
        let reason = self.reason.as_deref();

        let ok = match (self.kind, self.status) {
            (Input, Queued) => entry.is_none() && answer.is_none(),
            (Input, Placed) => entry.is_some() && answer.is_none(),
            (Input, Done) => entry.is_some() && answer.is_some(),
            (Input, Unanswered) => answer.is_none(),
            (Write, Queued) => entry.is_none() && answer.is_none(),
            (Write, Placed) => false,
            (Write, Done) => entry.is_some() && answer.is_none(),
            (Write, Unanswered) => entry.is_none() && answer.is_none(),
            (Continue, Queued) => false,
            (Continue, Placed) => entry.is_none() && answer.is_none(),
            (Continue, Done) => entry.is_none() && answer.is_some(),
            (Continue, Unanswered) => entry.is_none() && answer.is_none(),
        };
        if !ok {
            return Err(format!(
                "submission {:?}/{:?} is not a legal combination (entry {:?}, answer {:?})",
                self.kind, self.status, entry, answer
            ));
        }
        // `reason` 只属于 `unanswered`；`detail` 只跟着 `reason`。
        if reason.is_some() != matches!(self.status, Unanswered) {
            return Err(format!(
                "submission {:?}/{:?} carries reason {:?}; reason belongs to unanswered only",
                self.kind, self.status, reason
            ));
        }
        if self.detail.is_some() && reason.is_none() {
            return Err("submission detail without reason".to_string());
        }
        Ok(())
    }
}

impl TryFrom<SubmissionFieldsRepr> for SubmissionFields {
    type Error = String;

    fn try_from(repr: SubmissionFieldsRepr) -> Result<Self, Self::Error> {
        let fields = SubmissionFields {
            conversation_id: repr.conversation_id,
            request_id: repr.request_id,
            trace: repr.trace,
            kind: repr.kind,
            status: repr.status,
            entry: repr.entry,
            answer: repr.answer,
            reason: repr.reason,
            detail: repr.detail,
        };
        fields.validate()?;
        Ok(fields)
    }
}

/// 一份文档的化身记录。上游 `DocumentRecord`。
///
/// **`history` / `fork` / `scope.kind` 是枚举，不许 `Value`**：`nous-durable` 的
/// `HistoryNotRetained` 判定就建立在 `history == rewindable` 上（那是唯一允许 as-of 读的值），
/// 用 `Value` 包一层，判定就退化成运行时字符串比较。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "DocumentRecordRepr")]
pub struct DocumentRecord {
    /// 化身 id：同一份逻辑文档重建时**永不复用**。
    pub id: Id,
    /// 文档定义 kind。
    pub kind: String,
    /// 家族成员键；单例文档没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// 造出这份化身的提交序号，由 storage 盖章。
    pub created_at: Seq,
    /// 退役这份化身的提交序号；还是当前版本时没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<Seq>,
    /// 谁拥有这份文档。
    pub scope: DocumentScope,
    /// 保不保留历史。`session` / `task` 作用域下**必须缺席**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<DocumentHistory>,
    /// fork 时从哪来。取值域受 `history` 约束。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<DocumentFork>,
}

/// 文档作用域。上游 `{kind: "session"} | {kind: "conversation", conversationId} | {kind: "task", taskId}`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DocumentScope {
    /// 全会话共享的文档（fixture 里有 1 份）。
    #[serde(rename = "session")]
    Session,
    /// 属于一条会话。
    #[serde(rename = "conversation", rename_all = "camelCase")]
    Conversation { conversation_id: Id },
    /// 属于一个任务。
    #[serde(rename = "task", rename_all = "camelCase")]
    Task { task_id: Id },
}

/// 文档保不保留历史。上游 `"latest" | "rewindable"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentHistory {
    /// 只保留当前状态。
    Latest,
    /// 保留 as-of 读需要的历史 —— **唯一允许 `asOf` fork 的值**。
    Rewindable,
}

/// 文档 fork 的起点。上游 `"current" | "initial" | "asOf"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentFork {
    /// 从源文档当前状态开始。
    Current,
    /// 从定义的初始值开始。
    Initial,
    /// 从 cutoff 处开始；**只有 `history: rewindable` 允许**。
    AsOf,
}

/// 反序列化中转体：把三种非法组合挡在 `DocumentRecord` 外面。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentRecordRepr {
    id: Id,
    kind: String,
    #[serde(default)]
    key: Option<String>,
    created_at: Seq,
    #[serde(default)]
    retired_at: Option<Seq>,
    scope: DocumentScope,
    #[serde(default)]
    history: Option<DocumentHistory>,
    #[serde(default)]
    fork: Option<DocumentFork>,
}

impl DocumentRecord {
    /// 作用域与 `history` / `fork` 的配对规则（上游靠类型表达，这里靠这一句话）。
    pub fn validate(scope: DocumentScope, history: Option<DocumentHistory>, fork: Option<DocumentFork>) -> Result<(), String> {
        match scope {
            DocumentScope::Session | DocumentScope::Task { .. } => {
                if history.is_some() || fork.is_some() {
                    return Err(format!(
                        "document scope {scope:?} must not carry history/fork"
                    ));
                }
            }
            DocumentScope::Conversation { .. } => match (history, fork) {
                (Some(DocumentHistory::Latest), Some(DocumentFork::Current | DocumentFork::Initial)) => {}
                (Some(DocumentHistory::Rewindable), Some(DocumentFork::AsOf | DocumentFork::Current | DocumentFork::Initial)) => {}
                (Some(DocumentHistory::Latest), Some(DocumentFork::AsOf)) => {
                    return Err("fork AsOf requires history rewindable".to_string())
                }
                (None, _) => {
                    return Err("conversation-scoped document requires history".to_string())
                }
                (_, None) => return Err("conversation-scoped document requires fork".to_string()),
            },
        }
        Ok(())
    }
}

impl TryFrom<DocumentRecordRepr> for DocumentRecord {
    type Error = String;

    fn try_from(repr: DocumentRecordRepr) -> Result<Self, Self::Error> {
        DocumentRecord::validate(repr.scope, repr.history, repr.fork)?;
        Ok(DocumentRecord {
            id: repr.id,
            kind: repr.kind,
            key: repr.key,
            created_at: repr.created_at,
            retired_at: repr.retired_at,
            scope: repr.scope,
            history: repr.history,
            fork: repr.fork,
        })
    }
}
