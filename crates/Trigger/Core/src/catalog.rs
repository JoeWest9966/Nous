//! 触发器目录的管理面：`/v1/triggers*` 那 11 条端点里的 11 个 handler。
//!
//! 搬自 `server/src/triggers.rs` 的 HTTP 半边与 `server/src/routes.rs` 的三条
//! （清单 / 点火 / 账）。语义一字未改：发布闸（五段 cron + on slug）在写库之前、
//! 坏定义 400 且库里一行都不写；`version` 是 ETag，撞了 412。
//!
//! 领域层在 [`crate::runtime`]，宿主能力在 [`crate::routes::Host`]（只要一件事：
//! 点火运行时在不在）。


use axum::extract::{Path, Query, State};
use axum::Json;
use nous_durable::actor::Actor;
use nous_durable::storage::sessions::ENTRIES_MAX_LIMIT;
use nous_routes::HttpRefusal;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::refusal::{bad, conflict, internal, not_found, precondition_failed};
use crate::routes::Host;

/// 本模块的失败都是「拒绝 + 状态码」：语义在 `nous_routes::HttpRefusal`，不另造一套枚举。
type Result<T> = std::result::Result<T, HttpRefusal>;
use crate::runtime::{definition_json, fire_history, observation_report};

/// #73：`PATCH /v1/triggers/{code}` 的请求体。逐键覆盖；未在协议里的键按
/// `wire.md` 原样忽略（`code` 是写死的例外，由 PUT 的显式字段拒绝）。
#[derive(Debug, Deserialize)]
pub struct PatchBody {
    /// `code` 是不可变 slug：PATCH 里出现就 400（不是静默忽略），与 PUT 同一规矩。
    #[serde(default)]
    code: Option<Value>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    enabled: Option<bool>,
    /// #73 的 ETag：当前 `version`。整数是票面写法；字符串 / `W/"N"` 也认，
    /// 方便和既有 PATCH 的 If-Match 客户端共用。
    #[serde(default, deserialize_with = "deserialize_version")]
    version: Option<i64>,
}

fn deserialize_version<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<i64>, D::Error> {
    /// 请求里的 `version` 字段是可选的；缺省时 `Option` 自己会进来，这里只接有值的那种。
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawVersion {
        Number(i64),
        Text(String),
    }
    let value = match Option::<RawVersion>::deserialize(d)? {
        Some(RawVersion::Number(number)) => Value::Number(number.into()),
        Some(RawVersion::Text(text)) => Value::String(text),
        None => return Ok(None),
    };
    let text = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text,
        Value::Null => return Ok(None),
        other => return Err(serde::de::Error::custom(format!("version 不是数字：{other}"))),
    };
    let trimmed = text.trim().trim_start_matches("W/").trim_matches('"');
    trimmed
        .parse::<i64>()
        .map(Some)
        .map_err(|_| serde::de::Error::custom("version 不是数字"))
}

fn parse_version(raw: &str) -> Result<i64> {
    let trimmed = raw.trim().trim_start_matches("W/").trim_matches('"');
    trimmed
        .parse::<i64>()
        .map_err(|_| bad(format!("version 不是数字：{raw}")))
}

/// `If-Match` 头也必须和 body 的 `version` 一致：两个地方都不许各说各话。
fn header_version(headers: &axum::http::HeaderMap) -> Result<Option<String>> {
    Ok(headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string))
}

/// #73：`PATCH /v1/triggers/{code}`。改 `schedule` / `params` / `enabled`，`code` 不可变。
pub async fn patch<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| {
        conflict("triggers not configured".to_string())
    })?;
    let body: PatchBody = serde_json::from_slice(&body)
        .map_err(|error| bad(format!("patch body: {error}")))?;
    if body.code.is_some() {
        return Err(bad("code 不可变（PATCH 里不许带 code）".to_string()));
    }
    let expected = body.version.ok_or_else(|| {
        bad("version is required".to_string())
    })?;
    if let Some(header) = header_version(&headers)? {
        if parse_version(&header)? != expected {
            return Err(precondition_failed("If-Match 与 version 不一致".to_string()));
        }
    }
    if body.schedule.is_none() && body.params.is_none() && body.enabled.is_none() {
        return Err(bad("patch body 至少要改一个键".to_string()));
    }
    // requestId 可选：重放回第一次回执，版本不涨。
    if let Some(request_id) = body.request_id.as_deref().filter(|id| !id.is_empty()) {
        let store = runtime.store.lock().await;
        if let Some(receipt) = store.patch_receipt(&code, request_id).map_err(internal)? {
            return Ok(Json(serde_json::from_str(&receipt).map_err(internal)?));
        }
    }
    let response = {
        let store = runtime.store.lock().await;
        let row = store.trigger(&code).map_err(internal)?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(|| not_found(format!("trigger {code} not found")))?;
        let current = nous_store::Trigger::from_row(&row, &row.spec).map_err(internal)?;
        let schedule = match body.schedule {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(cron)) => Some(Some(cron)),
            Some(other) => return Err(bad(format!("schedule 必须是字符串或 null：{other}"))),
        };
        let patched = current
            .patch(schedule, body.params, body.enabled)
            .map_err(|error| bad(error.slug().to_string()))?;
        // 回执正文在事务之前算好：definition 与库里改后的行逐字段一致，
        // version 就是 expected + 1（乐观锁保证只有这一笔能写进去）。
        let mut after = definition_json(&row);
        after["enabled"] = json!(patched.enabled());
        let patch_json = patched.definition_json();
        if let Ok(spec) = serde_json::from_str::<Value>(&patch_json) {
            after["spec"] = spec["spec"].clone();
            after["nextFireAt"] = spec["nextFireAt"].clone();
        }
        after["version"] = json!(expected + 1);
        let response = json!({
            "code": code,
            "enabled": after["enabled"].clone(),
            "name": after["name"].clone(),
            "spec": after["spec"].clone(),
            "version": expected + 1,
            "definition": after,
        });
        // review 10：变更 + 回执同一次提交。回执写失败 → 定义不涨 version、库不变。
        let version = if let Some(request_id) = body.request_id.as_deref().filter(|id| !id.is_empty()) {
            store.patch_with_receipt(&code, &patch_json, patched.enabled(), expected,
                request_id, &response.to_string()).map_err(internal)?
        } else {
            store.patch_trigger(&code, &patch_json, patched.enabled(), expected).map_err(internal)?
        };
        version.ok_or_else(|| precondition_failed(format!("trigger {code} version 不匹配")))?;
        response
    };
    runtime.reload_definitions().await;
    Ok(Json(response))
}
// ── #80：trigger-catalog 管理面 ─────────────────────────────────────────────
//
// 都在同一个 `triggers` 面、同一张 `triggers` 表上；不新增 crate、不开第二条写通道。
// 写入前过发布闸（五段 cron + on slug），坏定义 400 且库里一行都不写。

/// POST /v1/triggers 的 body。`code` 是身份，只在建的时候出现在 body 里；
/// 未知键按 `wire.md` 忽略（传输层的未知字段不是本地配置）。
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    code: String,
    #[serde(default)]
    name: Option<String>,
    spec: Value,
    #[serde(rename = "templateRef", default)]
    template_ref: Option<Value>,
    #[serde(rename = "firesAllowedBy", default)]
    fires_allowed_by: Option<Value>,
    #[serde(default)]
    target: Option<Value>,
    #[serde(default)]
    policy: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpecBody {
    #[serde(rename = "spec_version", alias = "specVersion", default)]
    spec_version: Option<u32>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    on: Option<Value>,
    #[serde(default)]
    params: Option<Value>,
    /// #80：session 是落点简写（null=新会话），与显式 target 互斥。
    #[serde(rename = "session", default)]
    session: Option<Value>,
}

/// 与 `routes.rs` / `presets.rs` 同一处取时（app 层允许直接时钟；kernel 不许）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

fn value_as_opt_string(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(bad(format!("{field} 必须是字符串或 null：{other}"))),
    }
}

/// 发布闸：schedule 五段 cron 或 null；on 是封闭 kind 的 slug 或 null。
/// 返回规范化的 (schedule, on)，**任何坏值都不落库**。
fn validate_rules(schedule: Option<&Value>, on: Option<&Value>) -> Result<(Option<String>, Option<String>)> {
    let schedule = value_as_opt_string(schedule, "schedule")?;
    let on = value_as_opt_string(on, "on")?;
    if let Some(cron) = schedule.as_deref() {
        nous_store::parse_five_field_cron(cron).map_err(|error| bad(format!("schedule 无效（{}）：{cron}", error.slug())))?;
    }
    nous_store::validate_on(on.as_deref()).map_err(|error| bad(format!("on 无效（{}）", error.slug())))?;
    Ok((schedule, on))
}

fn validate_params(params: Option<&Value>) -> Result<Option<Value>> {
    let Some(params) = params else { return Ok(None) };
    if params.is_null() {
        return Ok(None);
    }
    let object = params.as_object().ok_or_else(|| bad("params 必须是对象"))?;
    if !object.values().all(|value| value.is_string() || value.is_number() || value.is_boolean() || value.is_null()) {
        return Err(bad("params 的值只能是字符串 / 数字 / 布尔 / null"));
    }
    Ok(Some(params.clone()))
}

fn spec_json(spec_version: u32, schedule: Option<String>, on: Option<String>, params: Option<Value>) -> String {
    let mut spec = serde_json::json!({
        "spec_version": spec_version,
        "schedule": schedule,
        "on": on,
    });
    if let Some(params) = params {
        spec["params"] = params;
    }
    spec.to_string()
}

fn target_json(body_session: Option<&Value>, target: Option<&Value>) -> Result<String> {
    if body_session.is_some() && target.is_some() {
        return Err(bad("session 与 target 不能同时出现"));
    }
    if let Some(session) = body_session {
        return Ok(match session {
            Value::Null => json!({ "kind": "new" }).to_string(),
            Value::Number(number) => number.as_i64()
                .map(|id| json!({ "kind": "conversation", "id": id }).to_string())
                .ok_or_else(|| bad("session 必须是整数或 null"))?,
            other => return Err(bad(format!("session 必须是整数或 null：{other}"))),
        });
    }
    if let Some(target) = target {
        let kind = target.get("kind").and_then(Value::as_str).ok_or_else(|| bad("target.kind 必须"))?;
        return match kind {
            "new" => Ok(json!({ "kind": "new" }).to_string()),
            "conversation" => {
                let id = target.get("id").and_then(Value::as_i64).ok_or_else(|| bad("target.id 必须是整数"))?;
                Ok(json!({ "kind": "conversation", "id": id }).to_string())
            }
            other => Err(bad(format!("未知 target.kind：{other}"))),
        };
    }
    Ok(json!({ "kind": "new" }).to_string())
}

fn definition_row(store: &nous_store::Store, code: &str) -> Result<nous_store::triggers::Trigger> {
    store.trigger(code).map_err(internal)?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(|| not_found(format!("trigger {code} not found")))
}

fn catalog_definition_json(row: &nous_store::triggers::Trigger) -> Value {
    definition_json(row)
}

/// POST /v1/triggers：新建。返回 201 + 生效定义。
pub async fn create<H: Host>(
    State(host): State<H>,
    body: axum::body::Bytes,
) -> Result<(axum::http::StatusCode, Json<Value>)> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let body: CreateBody = serde_json::from_slice(&body).map_err(|error| bad(format!("create body: {error}")))?;
    let spec: SpecBody = serde_json::from_value(body.spec).map_err(|error| bad(format!("spec: {error}")))?;
    let (schedule, on) = validate_rules(spec.schedule.as_ref(), spec.on.as_ref())?;
    let params = validate_params(spec.params.as_ref())?;
    let spec_version = spec.spec_version.unwrap_or(1);
    let template_ref = body.template_ref
        .map(|value| value.to_string())
        .unwrap_or_else(|| json!({ "kind": "preset", "ref": "default" }).to_string());
    let fires_allowed_by = body.fires_allowed_by
        .map(|value| value.to_string())
        .unwrap_or_else(|| json!([{ "kind": "system", "id": "cron" }]).to_string());
    let target = target_json(spec.session.as_ref(), body.target.as_ref())?;
    let policy = body.policy.unwrap_or_else(|| "serial".to_string());
    let name = body.name.unwrap_or_else(|| body.code.clone());
    let spec_raw = spec_json(spec_version, schedule.clone(), on.clone(), params.clone());

    let (definition, version) = {
        let store = runtime.store.lock().await;
        let input = nous_store::NewTriggerDefinition {
            code: &body.code,
            name: &name,
            spec: &spec_raw,
            template_ref: &template_ref,
            fires_allowed_by: &fires_allowed_by,
            target: &target,
            policy: &policy,
        };
        // 发布闸：Trigger::new 校验九条准则 + cron；code 冲突（含墓碑）跃到 409。
        nous_store::Trigger::new(input, now_ms(), Some(&store))
            .map_err(|error| definition_error(&error))?;
        let category = body.category.as_deref();
        if let Some(name) = category {
            store.set_trigger_category(&body.code, Some(name)).map_err(internal)?;
        }
        let row = store.trigger(&body.code).map_err(internal)?.ok_or_else(|| internal("建完读不回触发器"))?;
        (catalog_definition_json(&row), row.version)
    };
    runtime.reload_definitions().await;
    let response = json!({
        "code": body.code,
        "name": name,
        "category": body.category,
        "spec": serde_json::from_str::<Value>(&spec_raw).unwrap_or(Value::Null),
        "version": version,
        "enabled": true,
        "definition": definition,
    });
    Ok((axum::http::StatusCode::CREATED, Json(response)))
}

fn definition_error(error: &nous_store::TriggerError) -> HttpRefusal {
    match error {
        nous_store::TriggerError::CodeTaken | nous_store::TriggerError::CodeTombstoned => {
            conflict(error.slug().to_string())
        }
        _ => bad(error.slug().to_string()),
    }
}

/// PUT /v1/triggers/{code} 的 body：逐键覆盖，未知键忽略，`code` 必拒。
#[derive(Debug, Deserialize)]
struct PutBody {
    #[serde(default)]
    code: Option<Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    on: Option<Value>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    session: Option<Value>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    version: Option<i64>,
}

/// PUT /v1/triggers/{code}：整份改定义，version 是 ETag。
pub async fn replace<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let body: PutBody = serde_json::from_slice(&body).map_err(|error| bad(format!("put body: {error}")))?;
    if body.code.is_some() {
        return Err(bad("code 不可改（路径里的 code 才是身份）"));
    }
    if body.schedule.is_none() && body.on.is_none() && body.params.is_none()
        && body.session.is_none() && body.enabled.is_none() && body.name.is_none() {
        return Err(bad("put body 至少要改一个键"));
    }
    // 先把规则过闸：坏 schedule / on / params 一律 400，不进事务、不写半份定义。
    let (new_schedule, new_on) = validate_rules(body.schedule.as_ref(), body.on.as_ref())?;
    let new_params = validate_params(body.params.as_ref())?;

    let (definition, version) = {
        let store = runtime.store.lock().await;
        let row = definition_row(&store, &code)?;
        let current = nous_store::Trigger::from_row(&row, &row.spec).map_err(internal)?;
        let expected = body.version.unwrap_or(row.version);
        if expected != row.version {
            return Err(precondition_failed(format!("trigger {code} version 不匹配")));
        }
        let spec = current.spec();
        let schedule = match &body.schedule {
            None => spec.schedule.clone(),
            Some(_) => new_schedule.clone(),
        };
        let on = match &body.on {
            None => spec.on.clone(),
            Some(_) => new_on.clone(),
        };
        let params = match &body.params {
            None => spec.params.clone(),
            Some(_) => new_params.clone(),
        };
        let spec_raw = spec_json(spec.spec_version, schedule, on, params);
        let target = if body.session.is_some() {
            target_json(body.session.as_ref(), None)?
        } else {
            // 保留旧 target：从库里的定义 JSON 读。
            let old: Value = serde_json::from_str(&row.spec).map_err(internal)?;
            old.get("target").map(|value| value.to_string()).unwrap_or_else(|| json!({ "kind": "new" }).to_string())
        };
        // 用定义层构造新对象（会再验一次 cron），再写回；code 原样不动。
        let old: Value = serde_json::from_str(&row.spec).map_err(internal)?;
        let rebuilt = nous_store::Trigger::new(
            nous_store::NewTriggerDefinition {
                code: &code,
                name: body.name.as_deref().unwrap_or(&row.name),
                spec: &spec_raw,
                template_ref: &old["template_ref"].to_string(),
                fires_allowed_by: &old["fires_allowed_by"].to_string(),
                target: &target,
                policy: old["policy"].as_str().unwrap_or("serial"),
            },
            now_ms(),
            None,
        ).map_err(|error| definition_error(&error))?;
        let enabled = body.enabled.unwrap_or(row.enabled);
        let version = store.put_definition(&code, &rebuilt.definition_json(), enabled, expected)
            .map_err(internal)?
            .ok_or_else(|| precondition_failed(format!("trigger {code} version 不匹配")))?;
        if let Some(name) = body.name.as_deref() {
            store.rename_trigger(&code, name).map_err(internal)?;
        }
        let after = store.trigger(&code).map_err(internal)?.ok_or_else(|| internal("put 后读不回触发器"))?;
        (catalog_definition_json(&after), version)
    };
    runtime.reload_definitions().await;
    Ok(Json(json!({
        "code": code,
        "version": version,
        "definition": definition,
    })))
}

/// POST /v1/triggers/{code}/archive：归档墓碑；账不动。
pub async fn archive<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let now = now_ms();
    let ok = {
        let store = runtime.store.lock().await;
        store.archive_trigger(&code, now).map_err(internal)?
    };
    if !ok {
        return Err(not_found(format!("trigger {code} not found 或已归档")));
    }
    runtime.reload_definitions().await;
    Ok(Json(json!({ "code": code, "archivedAt": now })))
}

/// POST /v1/triggers/{code}/restore：抹掉归档墓碑。
pub async fn restore<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let now = now_ms();
    let ok = {
        let store = runtime.store.lock().await;
        store.restore_trigger(&code, now).map_err(internal)?
    };
    if !ok {
        return Err(not_found(format!("trigger {code} 不是归档态")));
    }
    runtime.reload_definitions().await;
    Ok(Json(json!({ "code": code, "restoredAt": now })))
}

/// POST /v1/triggers/{code}/purge：先归档才能真删，账一并删。
pub async fn purge<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let now = now_ms();
    let fires = {
        let store = runtime.store.lock().await;
        store.purge_trigger(&code, now).map_err(internal)?
    };
    let Some(fires) = fires else {
        return Err(conflict(format!("trigger {code} 必须先归档才能真删（或已真删）")));
    };
    runtime.reload_definitions().await;
    Ok(Json(json!({ "code": code, "purgedAt": now, "fires": fires })))
}

#[derive(Debug, Deserialize)]
struct CategoryBody {
    code: String,
    category: Option<String>,
}

/// POST /v1/triggers/category：给一个触发器挂单值类别；null 清除。
pub async fn set_category<H: Host>(
    State(host): State<H>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let body: CategoryBody = serde_json::from_slice(&body).map_err(|error| bad(format!("category body: {error}")))?;
    let changed = {
        let store = runtime.store.lock().await;
        store.set_trigger_category(&body.code, body.category.as_deref()).map_err(internal)?
    };
    if changed.is_none() {
        return Err(not_found(format!("trigger {} not found", body.code)));
    }
    Ok(Json(json!({ "code": body.code, "category": body.category })))
}

/// GET /v1/triggers/categories：类别词表（词表 ∪ 在用）。
pub async fn categories<H: Host>(
    State(host): State<H>,
) -> Result<Json<Value>> {
    let runtime = host.triggers_runtime().ok_or_else(|| conflict("triggers not configured".to_string()))?;
    let names = runtime.store.lock().await.trigger_categories().map_err(internal)?;
    Ok(Json(json!({ "categories": names })))
}

/// `GET /v1/triggers`：观测清单 + 调度健康（#58）。装配了 store 就读真行；
/// 空库仍回 `{triggers:[],root:null}`（#49 空表契约保留）。
///
/// body 与 CLI `nous-app --triggers` **同一个函数、同一份序列化**（#58 判据 4）。
pub async fn list<H: Host>(State(host): State<H>) -> std::result::Result<Json<Value>, HttpRefusal> {
    let Some(runtime) = host.triggers_runtime() else {
        // 没装配不等于没形状：scheduler 一样在场，只是诚实回空（#58 的顶层带调度健康）。
        return Ok(Json(json!({
            "triggers": [],
            "root": Value::Null,
            "scheduler": {
                "lastTickAt": Value::Null,
                "lagMs": 0,
                "tickMs": nous_store::SCHEDULER_TICK_MS,
            },
        })));
    };
    let store = runtime.store.lock().await;
    // #73 / wire.md：心跳只读 durable 的 tick 记录。内存里有什么都不算，
    // 没库、没行就是 null（CLI 更是直接传 None，不走内存兜底）。
    let last_tick = runtime.load_last_tick_at();
    let report = observation_report(&store, &runtime.definitions(), last_tick, now_ms())?;
    Ok(Json(report))
}

/// `?limit=` 的查询参数：默认 50（票面值），上限 1000（`ENTRIES_MAX_LIMIT` 同一档）。
#[derive(Debug, Deserialize)]
pub struct FiresQuery {
    limit: Option<i64>,
}

impl FiresQuery {
    fn limit(&self) -> i64 {
        self.limit
            .unwrap_or(DEFAULT_FIRE_HISTORY_LIMIT)
            .clamp(1, ENTRIES_MAX_LIMIT as i64)
    }
}

/// `?limit=` 的默认值与上限（票面 50；上限与 entries 同一档：`ENTRIES_MAX_LIMIT`）。
const DEFAULT_FIRE_HISTORY_LIMIT: i64 = 50;

/// `GET /v1/triggers/:code/fires`：这个触发器最近的账，时间倒序（#58）。
/// 不存在的触发器 → 404 + `{error}`（不许空 200），判据 6。
pub async fn fires<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
    Query(limit): Query<FiresQuery>,
) -> std::result::Result<Json<Value>, HttpRefusal> {
    let Some(runtime) = host.triggers_runtime() else {
        return Err(HttpRefusal::conflict("triggers not configured"));
    };
    let store = runtime.store.lock().await;
    let history = fire_history(&store, &runtime.definitions(), &code, limit.limit())?;
    Ok(Json(history))
}

/// `POST /v1/triggers/:code/fire`：手动点火。actor 从 token 派生（#50），现为 system。
pub async fn fire<H: Host>(
    State(host): State<H>,
    Path(code): Path<String>,
) -> std::result::Result<Json<Value>, HttpRefusal> {
    let Some(runtime) = host.triggers_runtime() else {
        return Err(HttpRefusal::conflict("triggers not configured"));
    };
    let actor = Actor::system("local");
    let now_ms = now_ms();
    let outcome = runtime.fire(&code, &actor, now_ms).await?;
    Ok(Json(serde_json::to_value(&outcome).unwrap_or(Value::Null)))
}
