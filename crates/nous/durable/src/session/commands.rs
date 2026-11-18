//! `#22` 端点背后的三条**命令**：发消息 / 中止 / fork。
//!
//! ## 为什么住在 `session` 的子模块
//!
//! 三条命令都要「铸 id + 一次提交 + 把 publication 交给观察者」，而 id 分配器与提交锁都在
//! [`Session`] 里（`Storage::mint_id` 不落盘，必须和下一次提交在同一个事务里推进）。
//! 把这三步拆到端点层，等于让 `nous-server` 自己重做一遍提交语义 —— 那正是 `AGENTS.md`
//! §二 要避免的事。子模块能碰父模块的私有字段（`storage` / `observers`），但**不动
//! `Session` 的五个方法公开面**（`tests/session.rs` 扫源码钉着那张表）。
//!
//! ## 幂等不是「返回同一个 202 就行」
//!
//! `requestId` 的语义是**库里只有一条** submission。第二次打进来必须查到第一次那条，
//! 而不是又写一条再回同一个 id（那会让提交计数、未读、重放全部漂）。查找按
//! `conversationId + requestId`，走 [`Storage::submissions`]。
//!
//! ## 这些函数**不**做传输
//!
//! 返回值是给 `nous-server` 的普通结构；HTTP 状态码映射在端点层（404 / 409 / 400）。
//! 中止与 fork 的前置条件（会话存在、没有活跃任务）在这里判，因为它们是会话语义，
//! 不是 HTTP 语义。

use std::time::{SystemTime, UNIX_EPOCH};

use nous_chord::context::Context;
use nous_chord::watch::WatchEnd;
use serde_json::{json, Value};

use crate::actor::Actor;
use crate::harness::generation::LIVE_KIND;
use crate::storage::writes::StorageWrite;
use crate::types::{DocumentScope, EntryRecord, TerminalReason};
use crate::{DocumentContent, DocumentPoint, Id, Storage, StorageError};

use super::{publication_of, CommitPublication, Session, SessionEnd, SessionError};

/// 命令失败。`Session` 是「会话已经不能用了」，另外三条是调用方要看到的语义错误。
#[derive(Debug)]
pub enum CommandError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Session(SessionError),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::BadRequest(message)
            | CommandError::NotFound(message)
            | CommandError::Conflict(message) => f.write_str(message),
            CommandError::Session(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<StorageError> for CommandError {
    fn from(error: StorageError) -> Self {
        CommandError::Session(SessionError::Storage(error))
    }
}

/// `POST /v1/sessions/:id/messages` 的入参。`content` 原样存（字符串或段落数组都行）。
#[derive(Debug, Clone)]
pub struct SubmitRequest {
    pub conversation: Id,
    pub content: Value,
    pub request_id: Option<String>,
}

/// 受理结果。`status` 现在是 `"queued"` —— 真正的生成由 harness 接手，端点只负责受理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    pub submission_id: Id,
    pub entry_id: Id,
    pub status: String,
}

/// 受理一条输入：写一条 `pi.user` entry + 一条 `queued` submission。
pub async fn submit_message(
    session: &Session,
    request: SubmitRequest,
    actor: &Actor,
) -> Result<SubmitOutcome, CommandError> {
    if !valid_content(&request.content) {
        return Err(CommandError::BadRequest(
            "content must be a string or an array of content parts".to_string(),
        ));
    }
    let mut guard = session.storage.lock().await;
    let (entry_id, submission_id) = {
        let storage = storage_mut(&mut guard)?;
        if storage.conversation(request.conversation)?.is_none() {
            return Err(CommandError::NotFound(format!(
                "session {} not found",
                request.conversation
            )));
        }

        // 幂等：同一条 requestId 已经受理过就原样返回它，**不再写第二条**。
        if let Some(request_id) = request.request_id.as_deref() {
            if let Some(existing) = storage
                .submissions(request.conversation)?
                .into_iter()
                .find(|record| record.get("requestId").and_then(Value::as_str) == Some(request_id))
            {
                return Ok(SubmitOutcome {
                    submission_id: id_of(&existing)?,
                    entry_id: existing
                        .get("entry")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                    status: existing
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("queued")
                        .to_string(),
                });
            }
        }

        (storage.mint_id()?, storage.mint_id()?)
    };

    let entry = json!({
        "id": entry_id,
        "conversationId": request.conversation,
        "kind": "pi.user",
        "model": [{
            "role": "user",
            "content": request.content,
            "timestamp": now_ms(),
        }],
        "actor": actor,
    });
    // `input/queued` 的合法形状里**没有** `entry`（`types::SubmissionFields::validate` 的
    // 矩阵：entry 要等 `placed` 才出现）。这里写 `entry` 会被提交拒绝 —— 那不是类型啰嗦，
    // 是「还没进转录」这个事实。
    let mut submission = json!({
        "id": submission_id,
        "conversationId": request.conversation,
        "type": "input",
        "status": "queued",
        "actor": actor,
    });
    if let Some(request_id) = request.request_id {
        submission["requestId"] = Value::String(request_id);
    }

    commit_and_publish(
        session,
        &mut guard,
        vec![
            StorageWrite::Entry(entry),
            StorageWrite::Submission(submission),
        ],
    )
    .await?;

    Ok(SubmitOutcome {
        submission_id,
        entry_id,
        status: "queued".to_string(),
    })
}

/// 取消结果。`tasks` 是这次被打上 `abortRequested` 的任务；`terminal_entry` 是终态那条 entry。
///
/// 两次取消返回**同一个结构**（同任务、同 entry id、同 reason），所以响应逐字节相等。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelOutcome {
    pub tasks: Vec<Id>,
    pub terminal_entry: Option<Id>,
}

/// 终止的形状对调用方（abort / 超时 / 扩展死）暴露的只有这三个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelRequest {
    pub conversation: Id,
    pub reason: TerminalReason,
}

/// 唯一的取消实现：客户端 abort、内核超时、扩展进程死都调这里。
///
/// ## 一条通道、一个实现（S7）
///
/// 触发源只造 `CancelRequest`。token 的 `cancel()` 由本函数发起，落库也只在这里。
/// 全库唯一的 `abortRequested` 写点是 [`mark_abort_requested`]。
///
/// ## 事务与终态
///
/// 取消不打断事务：它**在事务结束之后**作为一条新提交落下。终态与它的正文写在
/// 同一条 entry、同一次提交里 —— 否则 kill -9 会切出「有正文无终态」。
///
/// ## 幂等
///
/// 已经有终态 entry 就原样返回（第一个写下的归因赢），不报错、不改写。
pub async fn cancel(
    session: &Session,
    request: CancelRequest,
) -> Result<CancelOutcome, CommandError> {
    // 唯一的取消动作：三个触发源共用这一个 handle（clone 出去仍是同一个 token）。
    // 后面落库的终态 entry 是这次取消的可见事实，不是第二套「取消实现」。
    session.cancellation.cancel();

    let mut guard = session.storage.lock().await;
    let (outcome, writes) = {
        let storage = storage_mut(&mut guard)?;
        if storage.conversation(request.conversation)?.is_none() {
            return Err(CommandError::NotFound(format!(
                "session {} not found",
                request.conversation
            )));
        }
        if let Some(existing) = terminal_entry(storage, request.conversation)? {
            return Ok(existing);
        }

        let tasks = live_task_ids(storage, request.conversation)?;
        if tasks.is_empty() {
            return Err(CommandError::Conflict(
                "session has no running task to abort".to_string(),
            ));
        }
        let entry_id = storage.mint_id()?;
        // 终态与正文同一条 entry、同一次提交。若 pi.live 里已有落库的 partial，
        // 把它搬进来（并清 generation）—— 重连看到的是「有结局的一段文本」。
        let (body, clear) = terminal_body(storage, request.conversation)?;
        let mut writes = vec![StorageWrite::Entry(json!({
            "id": entry_id,
            "conversationId": request.conversation,
            "kind": "pi.assistant",
            "model": [body],
            "reason": request.reason.as_str(),
        }))];
        if let Some(write) = clear {
            writes.push(write);
        }
        for id in &tasks {
            let Some(mut task) = storage.task(*id)? else {
                continue;
            };
            mark_abort_requested(&mut task);
            writes.push(StorageWrite::Task(task));
        }
        (
            CancelOutcome {
                tasks,
                terminal_entry: Some(entry_id),
            },
            writes,
        )
    };
    commit_and_publish(session, &mut guard, writes).await?;
    Ok(outcome)
}

/// `POST /v1/sessions/:id/abort`：客户端触发源，只把语义翻成 `user_cancel` 后调 [`cancel`]。
pub async fn abort(session: &Session, conversation: Id) -> Result<CancelOutcome, CommandError> {
    cancel(
        session,
        CancelRequest {
            conversation,
            reason: TerminalReason::UserCancel,
        },
    )
    .await
}

/// 全库**唯一**的 `abortRequested` 写点。任务状态机是不透明 JSON（docs/storage.md §12.5），
/// 所以只改这一个标记，不碰 `state`。
fn mark_abort_requested(task: &mut Value) {
    task["abortRequested"] = Value::Bool(true);
}

/// 从 `pi.live.generation.message` 取已落库的正文，并生成一条清掉 `generation` 的写。
/// 没有 live / 没有 partial 时给一条空的 assistant 消息，不清任何文档。
fn terminal_body(
    storage: &Storage,
    conversation: Id,
) -> Result<(Value, Option<StorageWrite>), CommandError> {
    let documents = storage
        .scan_documents(
            &crate::DocumentQuery {
                scope: json!({ "kind": "conversation", "conversationId": conversation }),
                kind: Some(LIVE_KIND.to_string()),
                at: crate::DocumentPoint::Current,
            },
            10_000,
            None,
        )?
        .map_err(|refusal| CommandError::BadRequest(refusal.to_string()))?;
    for record in documents.items {
        let document_id = id_of(&record)?;
        let Some(doc) = storage.materialize(document_id, crate::DocumentPoint::Current)? else {
            continue;
        };
        let Some(message) = doc.value.get("generation").and_then(|g| g.get("message")) else {
            continue;
        };
        if message.is_null() {
            continue;
        }
        let mut body = message.clone();
        if let Some(object) = body.as_object_mut() {
            object.insert("stopReason".to_string(), json!("aborted"));
        }
        let current = nous_chord::delta::Revision::new(doc.version as u64);
        let mut draft = nous_chord::delta::draft::Draft::new(&doc.value, current);
        draft
            .remove(&[nous_chord::delta::Seg::key("generation")])
            .map_err(|error| CommandError::Session(SessionError::Shape(error.to_string())))?;
        let trusted = draft
            .prepare(current)
            .map_err(|error| CommandError::Session(SessionError::Shape(error.to_string())))?;
        let clear = StorageWrite::DocumentChange {
            id: document_id,
            content: DocumentContent::Delta {
                version: doc.version,
                ops: Value::Array(nous_chord::delta::codec::Encoder::new().encode(&trusted.into_ops())),
            },
        };
        return Ok((body, Some(clear)));
    }
    Ok((
        json!({ "role": "assistant", "content": [], "stopReason": "aborted" }),
        None,
    ))
}

/// 这个会话里最新的一条带终态的 entry；顺便还原当时的 tasks（幂等返回值）。
///
/// 热路径：SQL 直接走 partial index，`LIMIT 1`，不反序列化整条会话的 entry。
fn terminal_entry(
    storage: &Storage,
    conversation: Id,
) -> Result<Option<CancelOutcome>, CommandError> {
    let Some(record) = storage.terminal_entry_in(conversation)? else {
        return Ok(None);
    };
    let entry: EntryRecord = serde_json::from_value(record)
        .map_err(|error| CommandError::Session(SessionError::Shape(error.to_string())))?;
    // 幂等返回值不读 `abortRequested`：那个字段全库只有一处能碰（S7）。
    // 同一个会话第二次取消时，第一次打过标记的任务还在，按「还活着的」报回去
    // 与第一次逐字节相等（状态机由任务自己走，读它的标记就是第二处写语义）。
    let tasks = live_task_ids(storage, conversation)?;
    Ok(Some(CancelOutcome {
        tasks,
        terminal_entry: Some(entry.id),
    }))
}

fn live_task_ids(storage: &Storage, conversation: Id) -> Result<Vec<Id>, CommandError> {
    storage
        .tasks(conversation)?
        .into_iter()
        .filter(|task| !is_terminal(task))
        .map(|task| id_of(&task))
        .collect()
}

/// 启动扫描：整机崩 / 重启留下的孤儿补一条 `abandoned`（spec ch7）。
///
/// **幂等**：先写终态 entry（first-wins），再清 `pi.live.generation`；下次启动看不到
/// 有正文无终态的 live，就不会再补。连重启三次只多一条。
/// 进程熔断**不调这里** —— 那是 `extension_crash`，两件事（S6）。
pub async fn scan_orphans(session: &Session) -> Result<usize, CommandError> {
    // 启动一次的扫描上限。撞到上限必须打印 —— 否则「重启三次只多一条」在长会话上是假绿。
    const SCAN_LIMIT: usize = 10_000;
    let mut guard = session.storage.lock().await;
    let (writes, count) = {
        let storage = storage_mut(&mut guard)?;
        let conversations = storage
            .scan_conversations(
                &crate::ConversationQuery {
                    owner_conversation_id: None,
                    owner_task_id: None,
                    order: None,
                },
                SCAN_LIMIT,
                None,
            )?
            .map_err(|refusal| CommandError::BadRequest(refusal.to_string()))?;
        if conversations.next.is_some() {
            eprintln!(
                "nous-durable: 启动扫描被 {SCAN_LIMIT} 条会话截断，可能漏了更多会话的孤儿"
            );
        }
        let mut writes = Vec::new();
        let mut count = 0usize;
        for conversation in conversations.items {
            let conversation_id = id_of(&conversation)?;
            if terminal_entry(storage, conversation_id)?.is_some() {
                continue;
            }
            let documents = storage
                .scan_documents(
                    &crate::DocumentQuery {
                        scope: json!({ "kind": "conversation", "conversationId": conversation_id }),
                        kind: Some(LIVE_KIND.to_string()),
                        at: crate::DocumentPoint::Current,
                    },
                    SCAN_LIMIT,
                    None,
                )?
                .map_err(|refusal| CommandError::BadRequest(refusal.to_string()))?;
            if documents.next.is_some() {
                eprintln!(
                    "nous-durable: 启动扫描被 {SCAN_LIMIT} 份 pi.live 文档截断，会话 {conversation_id} 可能漏了孤儿"
                );
            }
            for record in documents.items {
                let document_id = id_of(&record)?;
                let Some(doc) = storage.materialize(document_id, crate::DocumentPoint::Current)?
                else {
                    continue;
                };
                let Some(message) = doc.value.get("generation").and_then(|g| g.get("message"))
                else {
                    continue;
                };
                if message.is_null() {
                    continue;
                }
                let entry_id = storage.mint_id()?;
                let (body, clear) = terminal_body(storage, conversation_id)?;
                writes.push(StorageWrite::Entry(json!({
                    "id": entry_id,
                    "conversationId": conversation_id,
                    "kind": "pi.assistant",
                    "model": [body],
                    "reason": TerminalReason::Abandoned.as_str(),
                })));
                if let Some(write) = clear {
                    writes.push(write);
                }
                count += 1;
            }
        }
        (writes, count)
    };
    if !writes.is_empty() {
        commit_and_publish(session, &mut guard, writes).await?;
    }
    Ok(count)
}

/// `POST /v1/sessions/:id/fork` 的入参。`at == None` 表示从最后一条 entry 分叉。
#[derive(Debug, Clone)]
pub struct ForkRequest {
    pub conversation: Id,
    pub at: Option<Id>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkOutcome {
    pub id: Id,
    pub at: Id,
}

/// `POST /v1/sessions/:id/fork`：开一条带 `parent: {conversationId, at}` 的新会话。
///
/// **名字不给就是空串**（`wire.md`：本库不生成默认名）。给了名字就写成一条
/// `qrd3wh.sessions` 的 base —— 会话树的名字就是那张 `names` 表，`render_sessions` 会合并
/// 全部同名文档。空闲才允许分叉：正在跑的会话分出去，两个会话会共享同一条还在写的生成态。
pub async fn fork(session: &Session, request: ForkRequest) -> Result<ForkOutcome, CommandError> {
    let mut guard = session.storage.lock().await;
    let (id, at, writes) = {
        let storage = storage_mut(&mut guard)?;
        if storage.conversation(request.conversation)?.is_none() {
            return Err(CommandError::NotFound(format!(
                "session {} not found",
                request.conversation
            )));
        }
        if storage
            .tasks(request.conversation)?
            .iter()
            .any(|task| !is_terminal(task))
        {
            return Err(CommandError::Conflict(
                "session is busy; fork after it settles".to_string(),
            ));
        }

        let at = match request.at {
            Some(at) => at,
            None => storage
                .latest_entry_id(request.conversation)?
                .unwrap_or_default(),
        };
        let id = storage.mint_id()?;
        let record = json!({
            "id": id,
            "parent": { "conversationId": request.conversation, "at": at },
        });

        let mut writes = vec![StorageWrite::Conversation(record)];
        let name = request.name.unwrap_or_default();
        if !name.is_empty() {
            let document_id = storage.mint_id()?;
            writes.push(StorageWrite::DocumentCreate {
                id: document_id,
                record: json!({
                    "id": document_id,
                    "kind": "qrd3wh.sessions",
                    "scope": DocumentScope::Session,
                }),
                content: DocumentContent::Base {
                    version: 1,
                    value: json!({ "names": { id.to_string(): name } }),
                },
            });
        }
        (id, at, writes)
    };

    commit_and_publish(session, &mut guard, writes).await?;
    Ok(ForkOutcome { id, at })
}

// ---------------------------------------------------------------------------
// #47：rename / read / unread
//
// 三种写都走 `commit_and_publish`（同一把锁、同一个事务、同一份广播）；读走同一 Session
// 的存储引用（只读扫描），不另开第二条 Storage。名字/水位住在本库自己的文档里，键是会话 id。
// ---------------------------------------------------------------------------

const NAMES_KIND: &str = "qrd3wh.sessions";
const UNREAD_KIND: &str = "qrd3wh.unread";
const NAME_MAX_LEN: usize = 120;

/// `POST /v1/sessions/:id/rename` 的结果（回包就是生效值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamedOutcome {
    pub id: Id,
    pub name: String,
}

/// `POST /v1/read/:id` 的回执。`read_at` 是 epoch 毫秒；水位只进不退。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkReadOutcome {
    pub id: Id,
    pub read_at: i64,
    pub unread: u64,
}

/// 会话改名：trim、空串拒绝、最长 120（旧引擎语义）。
///
/// 名字住在 `qrd3wh.sessions` 的 `names` 表里 —— `render_sessions` 合并所有这种文档的
/// names。session 作用域的同地址文档只有一份在世化身，内容用整份 base 覆盖。
pub async fn rename_session(
    session: &Session,
    id: Id,
    raw_name: &str,
) -> Result<RenamedOutcome, CommandError> {
    let name = raw_name.trim();
    if name.is_empty() {
        return Err(CommandError::BadRequest("name 是空的".to_string()));
    }
    let name = name.chars().take(NAME_MAX_LEN).collect::<String>();
    let mut guard = session.storage.lock().await;
    let storage = storage_mut(&mut guard)?;
    if storage.conversation(id)?.is_none() {
        return Err(CommandError::NotFound(format!("session {id} not found")));
    }
    let (existing_id, existing) = session_scope_document(storage, NAMES_KIND)?;
    let mut names = existing
        .as_ref()
        .and_then(|value| value.get("names").cloned())
        .unwrap_or_else(|| json!({}));
    names[id.to_string()] = json!(name);
    let writes = upsert_session_document(storage, NAMES_KIND, existing_id, json!({ "names": names }))?;
    drop(guard);
    commit_and_publish_with_new_guard(session, writes).await?;
    Ok(RenamedOutcome { id, name })
}

/// 全部会话的未读状态：`{"id": {"unread": N, "readAt": ms|null}}`。
///
/// `unread` = 「水位 entry id 之后还有几条 entry」；水位文档缺席给 `{0, null}`。
pub async fn unread_map(session: &Session) -> Result<Value, CommandError> {
    let guard = session.storage.lock().await;
    let storage = guard.as_ref().ok_or(CommandError::Session(SessionError::Closed))?;
    let conversations = storage
        .scan_conversations(
            &crate::ConversationQuery {
                owner_conversation_id: None,
                owner_task_id: None,
                order: None,
            },
            10_000,
            None,
        )?
        .map_err(|refusal| CommandError::BadRequest(refusal.to_string()))?;
    let marks = session_scope_document(storage, UNREAD_KIND)?
        .1
        .and_then(|value| value.get("marks").cloned())
        .unwrap_or_else(|| json!({}));
    let mut result = serde_json::Map::new();
    for conversation in conversations.items {
        let id = id_of(&conversation)?;
        let mark = marks.get(id.to_string());
        let watermark = mark
            .and_then(|entry| entry.get("entryId"))
            .and_then(Value::as_i64)
            .unwrap_or_default();
        // 没有水位 = 引擎种的 0（wire.md：没标过已读也给 0，不是把全部 history 算成未读）。
        let unread = if mark.is_none() {
            0
        } else {
            let entries = storage.scan_entries(
                &crate::storage::storage::EntryQuery {
                    conversation_id: id,
                    min_entry_id: Some(watermark.saturating_add(1)),
                    max_entry_id: None,
                    order: None,
                },
                10_000,
                None,
            )?
            .map_err(|refusal| CommandError::BadRequest(refusal.to_string()))?;
            entries.items.len() as u64
        };
        result.insert(
            id.to_string(),
            json!({
                "unread": unread,
                "readAt": mark.and_then(|entry| entry.get("readAt")).cloned().unwrap_or(Value::Null),
            }),
        );
    }
    Ok(Value::Object(result))
}

/// 标已读：水位推到该会话当前最新 entry id（只进不退），并广播。
pub async fn mark_read(
    session: &Session,
    id: Id,
) -> Result<MarkReadOutcome, CommandError> {
    let read_at = now_ms();
    let mut guard = session.storage.lock().await;
    let storage = storage_mut(&mut guard)?;
    if storage.conversation(id)?.is_none() {
        return Err(CommandError::NotFound(format!("session {id} not found")));
    }
    let latest = storage.latest_entry_id(id)?.unwrap_or_default();
    let (existing_id, existing) = session_scope_document(storage, UNREAD_KIND)?;
    let mut marks = existing
        .as_ref()
        .and_then(|value| value.get("marks").cloned())
        .unwrap_or_else(|| json!({}));
    let previous = marks
        .get(id.to_string())
        .and_then(|entry| entry.get("entryId"))
        .and_then(Value::as_i64)
        .unwrap_or(i64::MIN);
    let effective_watermark = previous.max(latest);
    let effective_read_at = marks
        .get(id.to_string())
        .and_then(|entry| entry.get("readAt"))
        .and_then(Value::as_i64)
        .unwrap_or(read_at)
        .max(read_at);
    marks[id.to_string()] =
        json!({ "entryId": effective_watermark, "readAt": effective_read_at });

    let writes = upsert_session_document(storage, UNREAD_KIND, existing_id, json!({ "marks": marks }))?;
    drop(guard);
    commit_and_publish_with_new_guard(session, writes).await?;
    Ok(MarkReadOutcome {
        id,
        read_at: effective_read_at,
        unread: 0,
    })
}

/// session 作用域的单例文档：`(化身 id, 当前值)`；没建过给 `(None, None)`。
fn session_scope_document(
    storage: &Storage,
    kind: &str,
) -> Result<(Option<Id>, Option<Value>), CommandError> {
    let record = storage
        .find_document(
            &json!({ "kind": kind, "scope": { "kind": "session" } }),
            crate::DocumentPoint::Current,
        )
        .map_err(CommandError::from)?;
    let Some(record) = record else {
        return Ok((None, None));
    };
    let id = id_of(&record)?;
    Ok((
        Some(id),
        storage
            .materialize(id, crate::DocumentPoint::Current)?
            .map(|document| document.value),
    ))
}

/// 同地址已有一份在世文档就整份 base 覆盖它；否则新建一份。
fn upsert_session_document(
    storage: &mut Storage,
    kind: &str,
    existing: Option<Id>,
    value: Value,
) -> Result<Vec<StorageWrite>, CommandError> {
    let content = DocumentContent::Base { version: 1, value };
    Ok(match existing {
        Some(id) => vec![StorageWrite::DocumentChange { id, content }],
        None => {
            let id = storage.mint_id()?;
            vec![StorageWrite::DocumentCreate {
                id,
                record: json!({
                    "id": id,
                    "kind": kind,
                    "scope": DocumentScope::Session,
                }),
                content,
            }]
        }
    })
}

// ---------------------------------------------------------------------------
// 提交：与 `Session::commit` 同一条路径（同一把锁、同一个事务、同一份 publication）
// ---------------------------------------------------------------------------

/// 与 [`commit_and_publish`] 同一条提交/广播路径，只是自己拿锁。
/// 用在「先只读扫描 + mint_id，再放开锁提交」的命令里（rename / read）。
async fn commit_and_publish_with_new_guard(
    session: &Session,
    batch: Vec<StorageWrite>,
) -> Result<CommitPublication, CommandError> {
    let mut guard = session.storage.lock().await;
    commit_and_publish(session, &mut guard, batch).await
}

async fn commit_and_publish(
    session: &Session,
    guard: &mut Option<Storage>,
    batch: Vec<StorageWrite>,
) -> Result<CommitPublication, CommandError> {
    let committed = {
        let storage = guard.as_mut().expect("checked by the caller");
        storage.commit(&[batch], |_, _| {})
    };
    let result = match committed {
        Ok(result) => result,
        Err(error) => {
            // 存储失败 → 会话自己关掉（与 `Session::commit` 同一条语义）。先把存储从槽里取出来，
            // 关掉的存储之后每次调用都失败（`StorageError::Closed`）；`settle` 用的是另一个 mutex，
            // 不会撞锁。
            let message = error.to_string();
            if let Some(mut storage) = guard.take() {
                let _ = storage.close();
            }
            session.end_all(&WatchEnd::Failed {
                error: message.clone(),
            });
            session.settle(SessionEnd::Failed { error: message }).await;
            return Err(CommandError::Session(SessionError::Storage(error)));
        }
    };
    if let Some(refusal) = result.refused {
        return Err(CommandError::BadRequest(refusal.to_string()));
    }
    let Some(batch) = result.batches.first() else {
        return Err(CommandError::Conflict(
            "nothing was stored for this command".to_string(),
        ));
    };
    let publication = publication_of(batch)
        .map_err(|error| CommandError::Session(SessionError::Shape(error.to_string())))?;
    session.observers.publish(&publication, &Context::new());
    Ok(publication)
}

/// 一次通用的文档写入。应用层（#48 的 `documents`）用它写任何 kind 的文档：
/// durable 只认 `StorageWrite`，产品词只留在应用层。
///
/// [`DocumentMutation::Create`] 的 id 由**同一把写锁下**的分配器铸出，和写一起落库；
/// 调用方不需要（也不许）自己猜一个 id —— 那是第二条写者才需要的做法。
///
/// 和 [`submit_message`](super::commands::submit_message) / [`fork`](super::commands::fork)
/// 走**同一条** `commit_and_publish`：同一把写锁、同一个事务、同一个 `ObserverRegistry`。
#[derive(Debug, Clone)]
pub enum DocumentMutation {
    /// 新建一份文档。`kind` 是 opaque 字符串（例如 `"presets.Session-Template"`）。
    Create { kind: String, key: String, value: Value },
    /// 改一份已有文档的内容。`base_version` 是当前 base 的版本（乐观锁），
    /// `value` 是新的整份值；durable 自己 diff 出 delta，历史因此逐次累积。
    Change { id: Id, base_version: i64, value: Value },
    /// 追加一批 delta 操作（中性 op 数组，durable 不解释内容）。
    Delta { id: Id, version: i64, ops: Value },
    /// 退役一份文档（名字改动 = 旧名退役 + 新名创建，同一批里）。
    Retire { id: Id },
}

impl DocumentMutation {
    /// 聚成一批 `StorageWrite`。只有 `Create` 需要 id 分配器。
    fn into_write(self, storage: &mut Storage) -> Result<StorageWrite, CommandError> {
        Ok(match self {
            DocumentMutation::Create { kind, key, value } => {
                let id = storage.mint_id()?;
                StorageWrite::DocumentCreate {
                    id,
                    record: json!({
                        "id": id, "kind": kind, "key": key,
                        "scope": { "kind": "session" },
                    }),
                    content: DocumentContent::Base { version: 1, value },
                }
            }
            DocumentMutation::Change { id, base_version, value } => {
                let before = storage.materialize(id, DocumentPoint::Current)?
                    .ok_or_else(|| CommandError::NotFound(format!("document {id} not found")))?;
                if before.version != base_version {
                    return Err(CommandError::Conflict(format!(
                        "document {id} version changed: expected {base_version}, found {}", before.version
                    )));
                }
                let trusted = nous_chord::delta::diff::diff_revisions(&before.value, &value);
                let ops = Value::Array(nous_chord::delta::codec::Encoder::new().encode(trusted.as_ops()));
                StorageWrite::DocumentChange {
                    id,
                    content: DocumentContent::Delta { version: before.version, ops },
                }
            }
            DocumentMutation::Delta { id, version, ops } => StorageWrite::DocumentChange {
                id,
                content: DocumentContent::Delta { version, ops },
            },
            DocumentMutation::Retire { id } => StorageWrite::DocumentRetire { id },
        })
    }
}

pub async fn write_document(
    session: &Session,
    mutations: Vec<DocumentMutation>,
) -> Result<CommitPublication, CommandError> {
    let mut guard = session.storage.lock().await;
    let writes = {
        let storage = storage_mut(&mut guard)?;
        mutations.into_iter()
            .map(|mutation| mutation.into_write(storage))
            .collect::<Result<Vec<_>, _>>()?
    };
    commit_and_publish(session, &mut guard, writes).await
}

/// 会话已经关了（或在排队期间关的）→ 409。
fn storage_mut(guard: &mut Option<Storage>) -> Result<&mut Storage, CommandError> {
    guard
        .as_mut()
        .ok_or_else(|| CommandError::Conflict("session is closed".to_string()))
}

fn is_terminal(task: &Value) -> bool {
    task.get("state")
        .and_then(|state| state.get("status"))
        .and_then(Value::as_str)
        == Some("terminal")
}

fn valid_content(content: &Value) -> bool {
    content.is_string()
        || content
            .as_array()
            .is_some_and(|parts| parts.iter().all(Value::is_object))
}

fn id_of(record: &Value) -> Result<Id, CommandError> {
    record
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| CommandError::Session(SessionError::Shape("record has no id".to_string())))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// 会话取消 token 的只读视图。不是 `Session` 的公开方法（A3 面不许动），
/// 应用层停 provider 流时用它；落库终态仍只走 [`cancel`]。
pub fn cancel_token(session: &Session) -> tokio_util::sync::CancellationToken {
    session.cancellation.token().clone()
}
