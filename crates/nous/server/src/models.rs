//! 模型入口：元素库里的约定组 → 编目投影（`/v1/models/elements`）。
//!
//! 盘上真相在元素库（`nous-elements`）：一个 `nous-models` 组引用若干**供应商包**
//! （`kind:"provider"`，`models[]` 是模型清单）和可选的**覆盖包**（`kind:"model"`）。
//! 解析与合并的纯逻辑在 `nous-models`，这里只做「扫两段 → 挑组 → 投影 → HTTP 翻译」。
//!
//! 三条不越过的线：
//!
//! - **密钥只回形状**（`env` / `cmd` / `secret` / `plaintext` / `missing`），值不进响应、
//!   不进日志；元素库也不收明文（出现明文只标红，不回显）。
//! - **缺什么说什么**：缺包、悬空覆盖、默认模型不在表里，都在 `missing[]` / `problems[]` 里点名。
//! - **只读**：这个端点不改元素库一个字节（写面走 `/v1/elements`）。

use std::sync::Arc;

use axum::extract::State;
use axum::{Json, Router};
use nous_elements::Kind;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::elements::REPO_KEY;
use crate::error::Result;
use crate::resource_admin::repo_error;

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router.route("/v1/models/elements", axum::routing::get(projection))
}

/// `GET /v1/models/elements`：约定组 → 编目（`items[]` / `default`）+ 诊断。
///
/// 私库没装配 / 组不存在 = `state:"missing"` 空表（不是 404：这是「还没有」，不是「没这个端点」）。
async fn projection(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let Some(repo) = state.repo(REPO_KEY) else {
        return Ok(Json(json!({
            "kind": "models",
            "state": "missing",
            "repo": { "root": nous_env::ELEMENTS.resolve().to_string_lossy(), "present": false },
            "items": [],
            "providers": [],
            "packages": [],
            "missing": [nous_env::ELEMENTS.var],
            "problems": [],
            "detail": format!(
                "元素库私库未装配：{} 指向的目录不存在？",
                nous_env::ELEMENTS.var
            ),
        })));
    };

    let groups = nous_elements::scan(&repo, Kind::Groups).map_err(repo_error)?;
    let packages = nous_elements::scan(&repo, Kind::Packages).map_err(repo_error)?;

    let Some(group) = nous_models::find_group(&groups.items) else {
        let problems: Vec<Value> = groups
            .broken
            .iter()
            .filter(|broken| broken.id == nous_models::GROUP_ID)
            .map(|broken| {
                json!({
                    "element": broken.id,
                    "field": "file",
                    "problem": broken.detail,
                })
            })
            .collect();
        // 组读不出来时别假装「没配」：把解析原文带上。
        let detail = if problems.is_empty() {
            format!("没有 {} 这个组", nous_models::GROUP_ID)
        } else {
            format!("{} 这份组读不出来（见 problems）", nous_models::GROUP_ID)
        };
        return Ok(Json(json!({
            "kind": "models",
            "state": "missing",
            "repo": { "root": repo.root().to_string_lossy(), "present": true },
            "items": [],
            "providers": [],
            "packages": [],
            "missing": [format!("{}", nous_models::GROUP_ID)],
            "problems": problems,
            "detail": detail,
        })));
    };

    let snapshot = nous_models::load(group, &packages.items);
    let mut body = serde_json::to_value(&snapshot).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut body {
        map.insert("kind".into(), json!("models"));
        map.insert(
            "repo".into(),
            json!({ "root": repo.root().to_string_lossy(), "present": true }),
        );
        // 同一目录里坏掉的包也列出来：它们没进 items，但不该悄悄消失。
        if !packages.broken.is_empty() {
            let mut problems = map.get("problems").and_then(Value::as_array).cloned().unwrap_or_default();
            problems.extend(packages.broken.iter().map(|broken| {
                json!({ "element": broken.id, "field": "file", "problem": broken.detail })
            }));
            map.insert("problems".into(), Value::Array(problems));
            map.insert("state".into(), json!("problem"));
        }
    }
    Ok(Json(body))
}
