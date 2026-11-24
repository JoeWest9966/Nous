//! 模型入口的**宿主侧适配**：元素库投影 → `GET /v1/models` 的形状（`ModelsResponse`）。
//!
//! 投影本身（组 → 编目 + 诊断，含 HTTP 回包）已经搬进 `nous-element-models` 插件，
//! `/v1/models/elements` 那个端点也由它自己声明实现。这里只留两件事：
//!
//! - [`group_catalog`]：把插件的投影折成 device 的形状，供 `GET /v1/models` 与发消息的
//!   模型解析共用（一个编目只有一处出处）；
//! - 宿主能力：`AppState` 实现 `nous_element_models::routes::Host`（见 `plugins.rs`）。

use std::sync::Arc;

use crate::app::AppState;
use crate::elements::REPO_KEY;

/// 约定组 → `GET /v1/models` 的形状。组不存在 / 私库未装配 → `None`，
/// 调用方回落到 device 的文件/内联来源（行为与没有这条线时一致）。
pub fn group_catalog(state: &Arc<AppState>) -> Option<nous_device::ModelsResponse> {
    let repo = state.repo(REPO_KEY)?;
    let snapshot = nous_element_models::routes::snapshot(&repo)?;
    let default = snapshot.default.as_ref().map(|model| nous_device::ModelRef {
        provider: model.provider.clone(),
        model_id: model.model_id.clone(),
    });
    let items = snapshot
        .items
        .iter()
        .map(|item| nous_device::ModelItem {
            provider: item.provider.clone(),
            model_id: item.model_id.clone(),
            name: item.name.clone(),
            thinking_levels: item.thinking_levels.clone(),
            context_window: item.context_window,
            max_tokens: item.max_tokens,
        })
        .collect();
    Some(nous_device::ModelsResponse {
        default,
        items,
        catalog: nous_device::CatalogDiagnostics {
            state: "ok",
            source: Some("elements"),
            path: repo.root().join("groups"),
            fetched_at: None,
            error: None,
        },
    })
}
