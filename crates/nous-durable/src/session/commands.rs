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

use crate::storage::writes::StorageWrite;
use crate::types::DocumentScope;
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
        vec![StorageWrite::Entry(entry), StorageWrite::Submission(submission)],
    )
    .await?;

    Ok(SubmitOutcome {
        submission_id,
        entry_id,
        status: "queued".to_string(),
    })
}

/// 中止结果。`tasks` 是被打上 `abortRequested` 的任务 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortOutcome {
    pub tasks: Vec<Id>,
}

/// `POST /v1/sessions/:id/abort`：给这个会话的**每一个**活跃任务打 abort 标记。
///
/// 只改 `abortRequested`，不动 `state`：任务状态机是不透明 JSON（`docs/storage.md` §12.5），
/// 由任务自己在下一个 checkpoint 读这个标记。没有活跃任务时是 409 —— 「中止一个闲会话」
/// 不是成功，客户端拿它当成功会以为停下来了。
pub async fn abort(session: &Session, conversation: Id) -> Result<AbortOutcome, CommandError> {
    let mut guard = session.storage.lock().await;
    let (aborted, writes) = {
        let storage = storage_mut(&mut guard)?;
        if storage.conversation(conversation)?.is_none() {
            return Err(CommandError::NotFound(format!(
                "session {conversation} not found"
            )));
        }
        let mut aborted = Vec::new();
        let mut writes = Vec::new();
        for mut task in storage.tasks(conversation)? {
            if is_terminal(&task) {
                continue;
            }
            let id = id_of(&task)?;
            task["abortRequested"] = Value::Bool(true);
            aborted.push(id);
            writes.push(StorageWrite::Task(task));
        }
        (aborted, writes)
    };
    if writes.is_empty() {
        return Err(CommandError::Conflict(
            "session has no running task to abort".to_string(),
        ));
    }
    commit_and_publish(session, &mut guard, writes).await?;
    Ok(AbortOutcome { tasks: aborted })
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
            // 存储失败 → 会话自己关掉（与 `Session::commit` 同一条语义）。先把存储取出来，
            // `close` 消费自己；`settle` 用的是另一个 mutex，不会撞锁。
            let message = error.to_string();
            if let Some(storage) = guard.take() {
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
