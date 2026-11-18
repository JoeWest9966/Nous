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

use crate::harness::generation::LIVE_KIND;
use crate::storage::writes::StorageWrite;
use crate::types::{DocumentScope, EntryRecord, TerminalReason};
use crate::{DocumentContent, Id, Storage, StorageError};

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
    });
    // `input/queued` 的合法形状里**没有** `entry`（`types::SubmissionFields::validate` 的
    // 矩阵：entry 要等 `placed` 才出现）。这里写 `entry` 会被提交拒绝 —— 那不是类型啰嗦，
    // 是「还没进转录」这个事实。
    let mut submission = json!({
        "id": submission_id,
        "conversationId": request.conversation,
        "type": "input",
        "status": "queued",
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
// 提交：与 `Session::commit` 同一条路径（同一把锁、同一个事务、同一份 publication）
// ---------------------------------------------------------------------------

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
