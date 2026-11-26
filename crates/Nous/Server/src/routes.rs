//! 路由：`wire.md` 的 `/v1/*`。
//!
//! 这一版落的六条（`#22` 票面）：
//!
//! - `GET  /v1/sessions` —— 会话树快照；
//! - `GET  /v1/sessions/:id/entries` —— 分页历史（不透明游标，上限 1000）；
//! - `POST /v1/sessions/:id/messages` —— 受理输入，202 `{submissionId, status}`；
//! - `GET  /v1/sessions/:id/events` —— SSE：先 `snapshot`，再合帧后的增量；
//! - `POST /v1/sessions/:id/abort` —— 200 `{status:"aborted"}`；
//! - `POST /v1/sessions/:id/fork` —— 201 `{id, at}`。
//!
//! 别的端点（`rename` / `archive` / `device` / `models` …）不在这里「顺手」加：每条都有
//! 自己的票面语义，先开票。加一个空壳 Handler 只会让客户端以为它通了。
//!
//! ## 错误形状只有一种
//!
//! 所有失败走 [`ServerError`]：`{ error: string }` + 语义状态码。路径里的 `:id` 用字符串
//! 接再自己 parse —— 让 axum 的 `Path<i64>` 拒掉会吐一段纯文本，破坏 `{error}`。

use std::convert::Infallible;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use nous_durable::storage::cursor::{Cursor, ScanOrder};
use nous_durable::storage::sessions::{ENTRIES_MAX_LIMIT};
use nous_sessions::turn::TurnRequest;
use nous_wire::{Generation, RemoteEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::app::AppState;
use crate::coalesce::{self, Framing};
use crate::error::{Result, ServerError};
// `messages` 留在 server，但会话 id 的解析口径归插件（一处判）。
use nous_sessions::routes::parse_session;
use nous_device::ModelsResponse;
use crate::records::CreateSessionRequest;
use crate::source::SessionSource;

/// 装配路由。`state` 里的 `source` 决定数据从哪来，`commands` 决定写往哪去，
/// `hub` 决定事件往哪扇。
pub fn router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        // 17 条会话端点里只有这一条留在 server：它要宿主组装根（driver / completions）
        // 与提示词注入（`nous-prompts` / skills 库），都是 app 层的装配物。
        .route("/v1/sessions/{id}/messages", post(messages));
    // 设备 / 模型 / 自省（`_routes` / `capabilities` / `openapi.json`）/ 插件 README
    // 全在 `Device` 插件里；`with_state` 之前逐条 `mount`，声明就是注册。

    let mut router = router;
    for route in crate::plugins::routes() {
        router = route.mount(router);
    }
    router.with_state(state.clone())
}

/// 把会话 template 正文 + 选中的技能正文写进一条 `pi.system` entry，并回一条 message 给本轮
/// provider。
///
/// 正文进 `sections.prompts`（不是 `content`）：#74 的 `?system=1` 回放按 sections 合并，
/// 后续系统消息能按名字覆盖这一段；名字本身就是诊断位（`template: missing` /
/// `skill: missing` / `skill: problem`）。
/// #83：技能是提示词不是工具，所以这里不查 `readOnly` —— 只读会话照样注入技能。
async fn inject_prompt_section(
    state: &Arc<AppState>,
    session_id: i64,
    submitted: &crate::commands::SubmitResult,
) -> Option<nous_ai::Message> {
    let template_name = state.commands().template(session_id).await.ok().flatten();
    let prompts_root = nous_env::PROMPTS.resolve();
    let template_text = template_name.map(|template_name| {
        if let Some(template) = nous_prompts::read_template(&prompts_root, &template_name) {
            nous_prompts::render(&template.body, &Default::default())
        } else {
            format!("template: missing ({template_name})")
        }
    });
    let selected = state.commands().skills(session_id).await.ok().unwrap_or_default();
    let skills_text = library_skill::assemble_prompts(&nous_env::SKILLS.resolve(), &selected);
    let text = match (template_text, skills_text.is_empty()) {
        (Some(head), false) => format!("{head}\n\n{skills_text}"),
        (Some(head), true) => head,
        (None, skills) if !skills => skills_text,
        _ => return None,
    };
    let actor = "prompts".to_string();
    let message_value = serde_json::to_value(nous_ai::transform::system::section_patch(
        &[("prompts", Some(text.as_str()))], 0,
    )).ok()?;
    let durable = state.durable_session()?;
    let entry = serde_json::json!({
        // +4 让给扩展（它先合并，preamble 排在技能正文前）；模板/技能用 +5。
        "id": submitted.entry_id + 5,
        "conversationId": session_id,
        "kind": "pi.system",
        "model": [message_value],
        "actor": { "kind": "system", "id": actor },
    });
    let _ = durable.commit(
        vec![nous_durable::StorageWrite::Entry(entry)],
        nous_durable::session::CommitOptions::default(),
    ).await;
    Some(nous_ai::Message::System(Box::new(
        nous_ai::transform::system::section_patch(&[("prompts", Some(text.as_str()))], 0),
    )))
}

/// 本会话的选中项 → 帧里的名字（`skill:<名>` / `prompt:<名>`，顺序即选中顺序）。
///
/// 名字是**内容的身份**，不是扩展的身份：`skills` 扩展靠 `skill:` 前缀挑自己要渲染的几条，
/// `prompts` 扩展靠 `prompt:` 前缀认模板。装载名（`skills` / `prompts` 这种进程名）
/// 不进这条清单 —— 扩展自己就是其中之一，列出来对它没有信息量。
fn session_selection(skills: Vec<String>, template: Option<String>) -> Vec<String> {
    let mut selected: Vec<String> = skills.into_iter().map(|name| format!("skill:{name}")).collect();
    if let Some(template) = template {
        selected.push(format!("prompt:{template}"));
    }
    selected
}

/// 扩展的段进系统提示词（EX-08 那一侧的落地）。
///
/// 三件事：拼出本会话的选中项 → 问运行时渲染每一段（`render_turn_with`，1s 总预算）→
/// 落一条 `pi.system`（`sections.extensions`）并塞进本轮 context。
///
/// 什么时候什么都不做：没装扩展运行时（测试 / 精简装配）、所有扩展都渲染出空串
/// （比如只装了不产出段的扩展）。**不因为「没选任何东西」就跳过** —— `nous` 那条
/// 扩展产出的正是基础 preamble，它跟选中项无关。
async fn inject_extension_section(
    state: &Arc<AppState>,
    session_id: i64,
    submitted: &crate::commands::SubmitResult,
) -> Option<nous_ai::Message> {
    let runtime = state.ext()?.clone();
    let skills = state.commands().skills(session_id).await.ok().unwrap_or_default();
    let template = state.commands().template(session_id).await.ok().flatten();
    let selected = session_selection(skills, template);
    // cwd 取进程 cwd（跟工具执行环境同一个）—— 扩展不许碰盘，但要能说「你在哪个目录」。
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    // `render_turn` 会起线程等子进程（最多 1s），不许占着异步执行器。
    let turn = tokio::task::spawn_blocking(move || {
        // generation 目前不进帧（`render_turn_with` 里丢掉），留给将来的历史裁剪。
        runtime.render_turn_with(session_id, 0, &cwd, &selected)
    })
    .await
    .ok()?;
    let text = turn.text.trim_end().to_string();
    if text.trim().is_empty() {
        return None;
    }
    let message_value = serde_json::to_value(nous_ai::transform::system::section_patch(
        &[("extensions", Some(text.as_str()))],
        0,
    ))
    .ok()?;
    let durable = state.durable_session()?;
    let entry = serde_json::json!({
        // 与 inject_prompt_section 同一段 id 区间：+2 assistant、+3 live、+4 扩展、+5 模板/技能。
        // 扩展排在前：段是按 entry 顺序合并的，preamble 必须在技能正文之前。
        "id": submitted.entry_id + 4,
        "conversationId": session_id,
        "kind": "pi.system",
        "model": [message_value],
        "actor": { "kind": "system", "id": "extensions" },
    });
    let _ = durable.commit(
        vec![nous_durable::StorageWrite::Entry(entry)],
        nous_durable::session::CommitOptions::default(),
    ).await;
    Some(nous_ai::Message::System(Box::new(
        nous_ai::transform::system::section_patch(&[("extensions", Some(text.as_str()))], 0),
    )))
}


#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitBody {
    content: Value,
    #[serde(default)]
    request_id: Option<String>,
    /// #70：请求体显式指定的模型（优先级最高）。
    #[serde(default)]
    model: Option<nous_device::ModelRef>,
    /// `0`/省略 = 立即 202；`>0` = 最多等这么久，拿到答案回 200，超时回 202。
    #[serde(default)]
    wait_ms: Option<Value>,
    /// `queue`（默认）/ `interrupt`。忙时排队或先停当前轮次。
    #[serde(default)]
    when_busy: Option<Value>,
}

/// `POST /v1/sessions/:id/messages`：不传 `waitMs` 时立即 202。
///
/// `requestId` 是幂等锚：同一个值打两次返回同一个 `submissionId`，库里也只多一条 submission
/// （判据在 `tests/commands.rs` 与 `tests/endpoints.rs`）。
async fn messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SubmitBody>,
) -> Result<impl IntoResponse> {
    let session = parse_session(&id)?;
    let wait_ms = match &body.wait_ms {
        None | Some(Value::Null) => 0u64,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| ServerError::BadRequest("waitMs must be a non-negative integer".to_string()))?,
    };
    let when_busy = match &body.when_busy {
        None | Some(Value::Null) => "queue",
        Some(Value::String(value)) if value == "queue" || value == "interrupt" => value.as_str(),
        _ => {
            return Err(ServerError::BadRequest(
                "whenBusy must be queue or interrupt".to_string(),
            ))
        }
    };
    if when_busy == "interrupt" && state.commands().busy(session).await? {
        // #70：先停当前轮次（abort 会 cancel 同一个 token、写 user_cancel 终态），
        // 再受理新的一条。停不掉就报错，不许「假装打断」。
        state.commands().abort(session).await?;
    }
    let faux = state.provider_kind() == "faux";
    // #70：模型解析在**受理之前**。没有来源就 503，不许先 202 再沉默。
    let catalog = current_catalog(&state).map_err(ServerError::ServiceUnavailable)?;
    let session_model = state
        .source()
        .sessions()?
        .into_iter()
        .find(|item| item.id == session)
        .and_then(|item| item.model);
    let reference = crate::runtime::resolve_model_ref(
        body.model.as_ref(),
        session_model.as_deref(),
        &catalog,
    )
    .ok_or_else(|| ServerError::ServiceUnavailable("没有可用模型".to_string()))?;
    let result = state
        .commands()
        .submit(session, body.content, body.request_id)
        .await?;

    // #70：受理后一律起一轮。faux 只决定模型来源，不决定「跑不跑」。
    // 事件只用 wire 已有的类型：user entry 的 message_start/entry_appended 由
    // durable 的 CommitObserver → project 投影发出，assistant 由轮次的投影发；
    // 这里不再手工补帧（之前补的假 entry 是重复且形状不对的）。
    let completion_rx = if wait_ms > 0 {
        state.runtime().map(|runtime| runtime.completions.subscribe())
    } else {
        None
    };
    if let Some(runtime) = state.runtime() {
        let submitted = result.clone();
        let driver = runtime.driver.clone();
        let completions = runtime.completions.clone();
        let commands = state.commands().clone();
        // #51：faux 验收模式没显式指定模型时仍用固定 echo 模型；否则用解析出的引用构造。
        let model = if faux && body.model.is_none() {
            nous_ai::Model::new(
                crate::runtime::FAUX_MODEL_ID,
                "Faux Echo",
                nous_ai::Api::Faux,
                "faux",
                "http://localhost:0",
            )
        } else {
            crate::runtime::model_from_ref(&reference)
        };
        // #72：会话 template 的单值投影，在起轮前落一条 pi.system（与 #74 回放同一条线）。
        // 缺模板只记诊断，不阻断轮次。注入失败也照跑（诊断可查，不让用户卡住）。
        let system_message = inject_prompt_section(&state, session, &submitted).await;
        // 扩展的段排在模板/技能之后：preamble 在最前，条目细节在后。
        let extension_message = inject_extension_section(&state, session, &submitted).await;
        let mut turn_context = nous_ai::Context::new(Vec::new());
        if let Some(message) = extension_message {
            turn_context.messages.push(message);
        }
        if let Some(message) = system_message {
            turn_context.messages.push(message);
        }
        tokio::spawn(async move {
            let outcome = driver
                .run(TurnRequest {
                    conversation: session,
                    submission: submitted.submission_id,
                    entry_id: submitted.assistant_entry_id,
                    live: submitted.live_id,
                    model,
                    context: turn_context,
                    attempt: 1,
                })
                .await;
            match outcome {
                Ok(outcome) => {
                    let _ = completions.send((submitted.submission_id, outcome.entry));
                }
                Err(error) => {
                    // #70：失败也必须收敛 submission，不能停在 queued 假装受理。
                    eprintln!("nous-server: 轮次失败：{error}");
                    if let Err(fail_error) = commands
                        .fail_submission(session, submitted.submission_id, "model_error")
                        .await
                    {
                        eprintln!("nous-server: submission 收敛失败：{fail_error}");
                    }
                }
            }
        });
    }

    if wait_ms > 0 {
        if let Some(mut rx) = completion_rx {
            let deadline = tokio::time::Duration::from_millis(wait_ms);
            loop {
                match tokio::time::timeout(deadline, rx.recv()).await {
                    Ok(Ok((id, answer))) if id == result.submission_id => {
                        return Ok((StatusCode::OK, Json(answer)));
                    }
                    Ok(Ok(_)) => continue,
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Closed))
                    | Err(_) => break,
                }
            }
        }
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "submissionId": result.submission_id,
                "status": "running",
            })),
        ));
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "submissionId": result.submission_id,
            "status": result.status,
        })),
    ))
}

