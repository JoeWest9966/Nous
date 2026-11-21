//! #82 combos 动作面：试跑 `fire` 与手动 `launch`。
//!
//! `fire` 先问一次 `#73` 的纯判定 `nous_dispatcher::evaluate`，不 fire 就不建会话；
//! `launch` 跳过判定、直接建会话。两者共用同一条 launch 路径（建会话 → 写血缘 → 记幂等回执），
//! 只在 `forced` 一位上分叉，免得「起一条会话」出现两份实现。
//!
//! 时间来自注入的 `Clock`（生产注入系统时钟，测试用假时钟），不在这里直接读系统时间。
//! 幂等回执沿用 #61 的 `.combos-receipts/` 盘面目录：同一 `requestId` 重放回第一次回执，
//! 不新建会话、不追加血缘。

use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::Json;
use nous_durable::harness::clock::Clock;
use nous_durable::session::commands::{record_origin, OriginRecord};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::combos::{combo_store, ComboFile, ComboStore, view};
use crate::error::{Result, ServerError};

/// 试跑判定需要的外部事实由装配带进来，`evaluate` 自己不查库。
#[derive(Clone)]
pub struct ComboAction {
    clock: Arc<dyn Clock>,
    clock_allowed: bool,
}

impl ComboAction {
    pub fn new(clock: Arc<dyn Clock>, clock_allowed: bool) -> Self {
        Self { clock, clock_allowed }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionBody {
    /// `launch` 必填：终极模板名；`fire` 从路径里的组合名取，body 里可不带。
    name: Option<String>,
    /// 幂等锚（必填）。
    request_id: Option<String>,
}

impl ActionBody {
    fn request_id(&self) -> Result<&str> {
        self.request_id.as_deref().filter(|id| !id.is_empty())
            .ok_or_else(|| ServerError::BadRequest("requestId is required".to_string()))
    }
}

/// 从 trigger 定义快照取判定所需的那部分。组合钉的 `trigger` 名就是触发器 `code`。
fn spec_of(runtime: &crate::triggers::TriggerRuntime, code: &str) -> Result<nous_dispatcher::SpecView> {
    let definitions = runtime.definitions();
    let trigger = definitions.iter().find(|definition| definition.code() == code)
        .ok_or_else(|| ServerError::NotFound(format!("组合钉的触发器 `{code}` 不在册")))?;
    let spec = trigger.spec();
    Ok(nous_dispatcher::SpecView {
        enabled: trigger.enabled(),
        schedule: spec.schedule.clone(),
        on: spec.on.clone(),
    })
}

/// 一条终极模板的动作前检查：从盘重扫（不读进程内名单），确认引用都在。
fn ultimate(store: &ComboStore, name: &str) -> Result<ComboFile> {
    let file = store.one("Ultimate-Template", name)?;
    let view = view(
        &file,
        &store.trigger_names(),
        &store.session_names()?,
    );
    if !view["enabled"].as_bool().unwrap_or(false) {
        return Err(ServerError::Conflict(format!(
            "组合 `{name}` 现在不能用：{}",
            view["problem"].as_str().unwrap_or("引用不全")
        )));
    }
    let spec = file.body.get("spec").cloned().unwrap_or(Value::Null);
    if spec.get("trigger").and_then(Value::as_str).unwrap_or("").is_empty() {
        return Err(ServerError::BadRequest(format!("组合 `{name}` 没钉触发器")));
    }
    Ok(file)
}

/// 试跑判定与起会话共用的一条路径。`forced=false` = 经过了触发器，`true` = 手点 launch。
async fn launch(
    state: &Arc<AppState>,
    name: &str,
    file: &ComboFile,
    request_id: &str,
    forced: bool,
    fired: bool,
    reason: &str,
    now_ms: i64,
) -> Result<Value> {
    let store = combo_store(state)?;
    // 幂等优先于一切：重放回第一次回执，不建会话、不写血缘。
    if let Some(previous) = store.action_receipt(name, request_id)? {
        return Ok(previous);
    }
    let spec = file.body.get("spec").cloned().unwrap_or(Value::Null);
    let trigger = spec.get("trigger").and_then(Value::as_str).unwrap_or("").to_string();
    let ultimate_name = file.name.clone();
    let session_template = spec.get("session").and_then(Value::as_str).unwrap_or("").to_string();
    let durable = state.durable_session().ok_or_else(|| {
        ServerError::Conflict("combos actions need a durable session".to_string())
    })?;
    let created = durable
        .create_conversation(json!({
            "name": format!("▶ {ultimate_name}"),
            "template": session_template,
        }))
        .await
        .map_err(|error| ServerError::Internal(format!("create_conversation: {error}")))?;
    let session = created.get("id").and_then(Value::as_i64)
        .ok_or_else(|| ServerError::Internal("create_conversation 没回 id".to_string()))?;
    record_origin(
        &durable,
        OriginRecord {
            id: session,
            ultimate: ultimate_name.clone(),
            trigger: trigger.clone(),
            at: now_ms,
            forced,
        },
    )
    .await
    .map_err(|error| ServerError::Internal(format!("record_origin: {error}")))?;
    let receipt = json!({
        "fired": fired,
        "reason": reason,
        "sessionId": session,
        "ultimate": ultimate_name,
        "trigger": if trigger.is_empty() { Value::Null } else { Value::String(trigger) },
        "forced": forced,
        "requestId": request_id,
    });
    store.remember_action(name, request_id, &receipt)?;
    Ok(receipt)
}

/// `POST /v1/combos/:section/:name/fire`：问一次触发器，判定 fire 才起会话。
pub async fn fire(
    State(state): State<Arc<AppState>>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    if section != "Ultimate-Template" {
        return Err(ServerError::BadRequest(format!(
            "试跑只在 Ultimate-Template 段：{section}"
        )));
    }
    let body: ActionBody = serde_json::from_slice(&body)
        .map_err(|error| ServerError::BadRequest(format!("fire body: {error}")))?;
    let request_id = body.request_id()?.to_string();
    let store = combo_store(&state)?;
    if let Some(previous) = store.action_receipt(&name, &request_id)? {
        return Ok(Json(previous));
    }
    let file = ultimate(store, &name)?;
    let trigger = file.body["spec"]["trigger"].as_str().unwrap_or("").to_string();
    let runtime = state.triggers().ok_or_else(|| {
        ServerError::ServiceUnavailable("triggers not configured".to_string())
    })?;
    let action = state.combo_action().ok_or_else(|| {
        ServerError::ServiceUnavailable("combo action clock not configured".to_string())
    })?;
    let spec = spec_of(runtime, &trigger)?;
    let now_ms = action.clock.now_ms() as i64;
    let state_view = nous_dispatcher::StateView {
        busy: false,
        clock_allowed: action.clock_allowed,
    };
    let decision = nous_dispatcher::evaluate(&spec, now_ms, &state_view);
    let reason = nous_dispatcher::reason_text(&decision.reason);
    if !decision.fire {
        // 不 fire 就是不建会话。回执也记下来：同一 requestId 重放仍回同一句。
        let receipt = json!({
            "fired": false,
            "reason": reason,
            "sessionId": Value::Null,
            "requestId": request_id,
        });
        store.remember_action(&name, &request_id, &receipt)?;
        return Ok(Json(receipt));
    }
    let receipt = launch(&state, &name, &file, &request_id, false, true, &reason, now_ms).await?;
    Ok(Json(receipt))
}

/// `POST /v1/presets/Ultimate-Template/launch`：跳过触发器判定，直接起一条。
pub async fn launch_handler(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let body: ActionBody = serde_json::from_slice(&body)
        .map_err(|error| ServerError::BadRequest(format!("launch body: {error}")))?;
    let name = body.name.as_deref().filter(|name| !name.is_empty())
        .ok_or_else(|| ServerError::BadRequest("name is required".to_string()))?;
    let request_id = body.request_id()?.to_string();
    let store = combo_store(&state)?;
    if let Some(previous) = store.action_receipt(name, &request_id)? {
        return Ok(Json(previous));
    }
    let file = ultimate(store, name)?;
    let now_ms = match state.combo_action() {
        Some(action) => action.clock.now_ms() as i64,
        None => {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        },
    };
    let receipt = launch(&state, name, &file, &request_id, true, true, "手动 launch", now_ms).await?;
    Ok(Json(receipt))
}
