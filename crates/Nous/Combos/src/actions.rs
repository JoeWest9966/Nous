//! 本插件的四条端点：名册、开关写穿、试跑、手动 launch。
//!
//! `fire` 先问一次 `nous_dispatcher::evaluate`（纯函数，不查库），不 fire 就不建会话；
//! `launch` 跳过判定、直接建会话。两者共用同一条 launch 路径（建会话 → 写血缘 → 记幂等回执），
//! 只在 `forced` 一位上分叉，免得「起一条会话」出现两份实现。
//!
//! 时间来自宿主的注入时钟（[`Host::combo_clock`]），不在这里直接读系统时间。
//! 幂等回执沿用 `.combos-receipts/` 盘面目录：同一 `requestId` 重放回第一次回执，
//! 不新建会话、不追加血缘。

use axum::extract::{Path as AxumPath, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::store::{view, ComboFile, ComboStore};
use crate::{store_of, Host, SpecView};
use nous_routes::HttpRefusal;

type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchBody {
    enabled: bool,
    request_id: Option<String>,
}

/// `GET /v1/combos/:section`：每次请求都重扫盘面。
pub async fn list<H>(State(host): State<H>, AxumPath((section,)): AxumPath<(String,)>) -> EndpointResult
where
    H: Host + Clone + Send + Sync + 'static,
{
    let store = store_of(&host)?;
    Ok(Json(store.roster(&section)?))
}

/// `PATCH /v1/combos/:section/:name`：写穿 `spec.enabled`。
///
/// `If-Match` 是版本号（`v` + sha256 前 12 位）；`requestId` 是幂等锚。
pub async fn patch<H>(
    State(host): State<H>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> EndpointResult
where
    H: Host + Clone + Send + Sync + 'static,
{
    let store = store_of(&host)?;
    let body: PatchBody = serde_json::from_slice(&body)
        .map_err(|error| HttpRefusal::bad_request(format!("patch body: {error}")))?;
    let if_match = headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let request_id = body
        .request_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| HttpRefusal::bad_request("requestId is required"))?
        .to_string();
    Ok(Json(store.patch(&section, &name, body.enabled, &if_match, &request_id)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionBody {
    /// `launch` 必填：终极模板名；`fire` 从路径里的组合名取，body 里可不带。
    name: Option<String>,
    /// 幂等锚（必填）。
    request_id: Option<String>,
}

impl ActionBody {
    fn request_id(&self) -> Result<&str, HttpRefusal> {
        self.request_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| HttpRefusal::bad_request("requestId is required"))
    }
}

/// 一条终极模板的动作前检查：从盘重扫（不读进程内名单），确认引用都在。
fn ultimate(store: &ComboStore, name: &str) -> Result<ComboFile, HttpRefusal> {
    let file = store.one("Ultimate-Template", name)?;
    let view = view(&file, &store.trigger_names(), &store.session_names()?);
    if !view["enabled"].as_bool().unwrap_or(false) {
        return Err(HttpRefusal::conflict(format!(
            "组合 `{name}` 现在不能用：{}",
            view["problem"].as_str().unwrap_or("引用不全")
        )));
    }
    let spec = file.body.get("spec").cloned().unwrap_or(Value::Null);
    if spec.get("trigger").and_then(Value::as_str).unwrap_or("").is_empty() {
        return Err(HttpRefusal::bad_request(format!("组合 `{name}` 没钉触发器")));
    }
    Ok(file)
}

/// 试跑判定与起会话共用的一条路径。`forced=false` = 经过了触发器，`true` = 手点 launch。
async fn launch<H>(
    host: &H,
    store: &ComboStore,
    name: &str,
    file: &ComboFile,
    request_id: &str,
    forced: bool,
    fired: bool,
    reason: &str,
    now_ms: i64,
) -> Result<Value, HttpRefusal>
where
    H: Host + Send + Sync,
{
    // 幂等优先于一切：重放回第一次回执，不建会话、不写血缘。
    if let Some(previous) = store.action_receipt(name, request_id)? {
        return Ok(previous);
    }
    let spec = file.body.get("spec").cloned().unwrap_or(Value::Null);
    let trigger = spec.get("trigger").and_then(Value::as_str).unwrap_or("").to_string();
    let ultimate_name = file.name.clone();
    let session_template = spec.get("session").and_then(Value::as_str).unwrap_or("").to_string();
    let session = host
        .launch_session(&ultimate_name, &trigger, &session_template, forced, now_ms)
        .await?;
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
pub async fn fire<H>(
    State(host): State<H>,
    AxumPath((section, name)): AxumPath<(String, String)>,
    body: axum::body::Bytes,
) -> EndpointResult
where
    H: Host + Clone + Send + Sync + 'static,
{
    if section != "Ultimate-Template" {
        return Err(HttpRefusal::bad_request(format!(
            "试跑只在 Ultimate-Template 段：{section}"
        )));
    }
    let body: ActionBody = serde_json::from_slice(&body)
        .map_err(|error| HttpRefusal::bad_request(format!("fire body: {error}")))?;
    let request_id = body.request_id()?.to_string();
    let store = store_of(&host)?;
    if let Some(previous) = store.action_receipt(&name, &request_id)? {
        return Ok(Json(previous));
    }
    let file = ultimate(&store, &name)?;
    let trigger = file.body["spec"]["trigger"].as_str().unwrap_or("").to_string();
    let spec = host.trigger_spec(&trigger)?;
    let (now_ms, clock_allowed) = host.combo_clock()?;
    let state_view = nous_dispatcher::StateView {
        busy: false,
        clock_allowed,
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
    let receipt = launch(
        &host, &store, &name, &file, &request_id, false, true, &reason, now_ms,
    )
    .await?;
    Ok(Json(receipt))
}

/// `POST /v1/presets/Ultimate-Template/launch`：跳过触发器判定，直接起一条。
pub async fn launch_handler<H>(
    State(host): State<H>,
    body: axum::body::Bytes,
) -> EndpointResult
where
    H: Host + Clone + Send + Sync + 'static,
{
    let body: ActionBody = serde_json::from_slice(&body)
        .map_err(|error| HttpRefusal::bad_request(format!("launch body: {error}")))?;
    let name = body
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| HttpRefusal::bad_request("name is required"))?
        .to_string();
    let request_id = body.request_id()?.to_string();
    let store = store_of(&host)?;
    if let Some(previous) = store.action_receipt(&name, &request_id)? {
        return Ok(Json(previous));
    }
    let file = ultimate(&store, &name)?;
    let now_ms = host.combo_clock()?.0;
    let receipt = launch(
        &host, &store, &name, &file, &request_id, true, true, "手动 launch", now_ms,
    )
    .await?;
    Ok(Json(receipt))
}

/// `SpecView` 在这里被用到（[`Host::trigger_spec`] 的返回类型），导出给宿主侧参考。
pub use nous_dispatcher::SpecView as _SpecView;
