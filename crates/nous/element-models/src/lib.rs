//! models 插件：元素库里的「模型入口组」→ 模型编目投影。
//!
//! 盘上真相不在这个 crate 里，在元素库（`nous-elements`）：一个**约定组**
//! （[`GROUP_ID`]）引用一串**供应商包**，包里的 `models[]` 是模型清单；单个模型需要
//! 独立治理时再加一个 `kind:"model"` 的**覆盖包**，按 `(provider, modelId)` 逐键合并。
//!
//! 这个 crate 只做三件纯事：解析、合并、投影。它**不读盘、不联网、不看时钟** ——
//! 元素库的读写归 `nous-elements` / `nous-server`，密钥解析靠调用方给的 [`KeyLookup`]。
//!
//! 两条不越过的线：
//!
//! - **密钥不进投影**：`apiKey` 只被解析成 [`KeyRef`] 形状（`env` / `cmd` / `secret` /
//!   `plaintext` / `missing`），值一个字节都不进 [`Snapshot`]，也不进错误文本。
//! - **缺什么说什么**：包缺了、模型缺了、默认模型不在表里，都进 `missing[]` / `problems[]`，
//!   不许静默跳过（静默 = 看着能用、点了才炸）。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use nous_elements::ElementItem;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 模型入口组的约定 id。引擎只读这一个组。
pub const GROUP_ID: &str = "element-models";

/// 供应商包的 id 前缀（`provider-wb2api` → 供应商键 `wb2api`）。
/// 包里显式写了 `provider` 字段时以字段为准，前缀只是兜底。
pub const PROVIDER_PREFIX: &str = "provider-";

/// 思考档的规范顺序（pi 的 `MODEL_THINKING_LEVELS`）；模型没写就是 `["off"]`。
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 一条模型的线形状，与 `GET /v1/models` 的 `items[]` 同形（camelCase，别改）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelItem {
    pub provider: String,
    pub model_id: String,
    pub name: String,
    pub thinking_levels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// `provider` 是**供应商键**（`wb2api`），不是包 id（`provider-wb2api`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

/// 供应商包解析出来的连接信息。**不含密钥值**，只含 [`KeyRef`] 的形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// 包 id（`provider-wb2api`）。
    pub package: String,
    /// 供应商键（`wb2api`）。
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// 只回形状，不回值。
    pub key: KeyShape,
}

/// `apiKey` 的形状。**这是能出现在响应里的全部信息**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyShape {
    /// `$NAME`：请求时从环境变量取。
    Env,
    /// `!command`：请求时执行，取 stdout。
    Cmd,
    /// `secret:<id>`：从密钥空间取。
    Secret,
    /// 没写 `apiKey`。
    Missing,
    /// 写了明文 —— 元素库不收明文，这里只报形状，不回显。
    Plaintext,
}

/// 一个元素的问题：哪个元素、哪个字段、什么问题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub element: String,
    pub field: String,
    pub problem: String,
}

/// 一个被引用的包的状态（缺了也要列出来，别让它凭空消失）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageState {
    pub id: String,
    /// `ok` / `missing` / `problem`
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 投影结果：编目 + 诊断。`items` 是给引擎 / Desk 用的，其余是给人排错用的。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// 约定组 id；组本身不存在时是 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// `ok` / `missing` / `problem`
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<ModelRef>,
    pub items: Vec<ModelItem>,
    pub providers: Vec<Provider>,
    pub packages: Vec<PackageState>,
    pub missing: Vec<String>,
    pub problems: Vec<Problem>,
}

impl Snapshot {
    /// 组不存在时的那一份：`state:"missing"`，不是错误。
    pub fn missing_group() -> Self {
        Self { state: "missing", ..Self::default() }
    }
}

/// 从一组元素里挑出约定组（`GROUP_ID`）。同名的包不算。
pub fn find_group(groups: &[ElementItem]) -> Option<&ElementItem> {
    groups.iter().find(|item| item.id == GROUP_ID)
}

/// 把「组 + 它引用的包」投影成编目。纯函数：不读盘、不联网。
pub fn load(group: &ElementItem, packages: &[ElementItem]) -> Snapshot {
    let mut snapshot = Snapshot { group: Some(group.id.clone()), ..Snapshot::default() };
    let by_id: BTreeMap<&str, &ElementItem> =
        packages.iter().map(|item| (item.id.as_str(), item)).collect();

    let mut drafts: Vec<Draft> = Vec::new();
    let mut overlays: Vec<&ElementItem> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for id in referenced(group) {
        if !seen.insert(id.clone()) {
            snapshot.problems.push(Problem {
                element: group.id.clone(),
                field: "packages".into(),
                problem: format!("重复引用 `{id}`，第二次起忽略"),
            });
            continue;
        }
        let Some(item) = by_id.get(id.as_str()) else {
            snapshot.missing.push(format!("{}/{}", group.id, id));
            snapshot.packages.push(PackageState {
                id: id.clone(),
                state: "missing",
                detail: Some("组引用了这个包，但 packages/ 下没有".into()),
            });
            continue;
        };
        match kind_of(item) {
            Kind::Provider => {
                let provider = provider_key(item);
                snapshot.packages.push(PackageState {
                    id: item.id.clone(),
                    state: "ok",
                    detail: None,
                });
                let models = item.fields.get("models").and_then(Value::as_array);
                if item.fields.get("models").is_some_and(|value| !value.is_array()) {
                    snapshot.problems.push(Problem {
                        element: item.id.clone(),
                        field: "models".into(),
                        problem: "models 必须是数组".into(),
                    });
                }
                snapshot.providers.push(Provider {
                    package: item.id.clone(),
                    provider: provider.clone(),
                    base_url: string_field(&item.fields, "baseUrl"),
                    api: string_field(&item.fields, "api"),
                    key: parse_key_ref(item.fields.get("apiKey").and_then(Value::as_str)).shape(),
                });
                for raw in models.into_iter().flatten() {
                    match model_id_of(raw) {
                        Some(model_id) => drafts.push(Draft {
                            provider: provider.clone(),
                            model_id,
                            fields: raw.clone(),
                        }),
                        None => snapshot.problems.push(Problem {
                            element: item.id.clone(),
                            field: "models".into(),
                            problem: "这一项没有字符串 id，已跳过".into(),
                        }),
                    }
                }
            }
            Kind::Model => overlays.push(item),
            Kind::Unknown(other) => {
                snapshot.packages.push(PackageState {
                    id: item.id.clone(),
                    state: "problem",
                    detail: Some(format!("kind 是 `{other}`，不是 provider / model")),
                });
                snapshot.problems.push(Problem {
                    element: item.id.clone(),
                    field: "kind".into(),
                    problem: "只认 provider / model 两种包；未知的跳过（字段照旧留在盘上）".into(),
                });
            }
        }
    }

    // 覆盖包后应用：先有供应商包的清单，再按 (provider, modelId) 逐键压上去。
    for overlay in overlays {
        let at = drafts.iter().position(|draft| {
            draft.provider == overlay_provider(overlay) && draft.model_id == overlay_model_id(overlay)
        });
        match at {
            Some(index) => {
                let provider = drafts[index].provider.clone();
                let model_id = drafts[index].model_id.clone();
                let patch = overlay_model_fields(overlay);
                snapshot.packages.push(PackageState {
                    id: overlay.id.clone(),
                    state: "ok",
                    detail: Some(format!("覆盖 {provider}/{model_id}")),
                });
                merge_into(&mut drafts[index].fields, &patch);
            }
            None => {
                let target = format!("{}/{}", overlay_provider(overlay), overlay_model_id(overlay));
                snapshot.missing.push(format!("{}/{}", group.id, target));
                snapshot.packages.push(PackageState {
                    id: overlay.id.clone(),
                    state: "missing",
                    detail: Some(format!("覆盖目标 {target} 不在任何供应商包的 models 里")),
                });
                // 悬空的覆盖包是写错了，不是「暂时缺」：标红它，别让它悄悄不生效。
                snapshot.problems.push(Problem {
                    element: overlay.id.clone(),
                    field: "modelId".into(),
                    problem: format!("覆盖目标 {target} 不在编目里"),
                });
            }
        }
    }

    for draft in &drafts {
        snapshot.items.push(project(draft));
    }

    snapshot.default = read_default(group, &snapshot.items, &mut snapshot.problems);
    snapshot.state = if !snapshot.problems.is_empty() {
        "problem"
    } else if snapshot.items.is_empty() {
        "missing"
    } else {
        "ok"
    };
    snapshot
}

struct Draft {
    provider: String,
    model_id: String,
    fields: Value,
}

enum Kind {
    Provider,
    Model,
    Unknown(String),
}

fn kind_of(item: &ElementItem) -> Kind {
    match item.fields.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "provider" => Kind::Provider,
        "model" => Kind::Model,
        other => Kind::Unknown(other.to_string()),
    }
}

fn provider_key(item: &ElementItem) -> String {
    string_field(&item.fields, "provider").unwrap_or_else(|| {
        item.id.strip_prefix(PROVIDER_PREFIX).unwrap_or(&item.id).to_string()
    })
}

/// 将供应商包映射到 Rust runtime 的 `ProviderConfig` JSON 形状。
///
/// 未知字段不进入 runtime 配置（`nous-elements` 仍原样保存）；只传 runtime 真正消费的
/// `name/baseUrl/api/apiKey/headers/authHeader/models[]`。密钥字段保持引用字符串，不解析真值。
pub fn provider_config(item: &ElementItem) -> Option<Value> {
    if !matches!(kind_of(item), Kind::Provider) {
        return None;
    }
    let mut config = Map::new();
    copy_field(&mut config, &item.fields, "name");
    copy_field(&mut config, &item.fields, "baseUrl");
    copy_field(&mut config, &item.fields, "api");
    copy_field(&mut config, &item.fields, "apiKey");
    copy_field(&mut config, &item.fields, "headers");
    copy_field(&mut config, &item.fields, "authHeader");
    let models: Vec<Value> = item
        .fields
        .get("models")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|model| {
            let id = model.get("id").and_then(Value::as_str)?;
            let mut fields = model.as_object()?.clone();
            fields.insert("id".into(), Value::String(id.into()));
            Some(Value::Object(fields))
        })
        .collect();
    config.insert("models".into(), Value::Array(models));
    Some(Value::Object(config))
}

fn copy_field(target: &mut Map<String, Value>, source: &Value, field: &str) {
    if let Some(value) = source.get(field) {
        target.insert(field.to_string(), value.clone());
    }
}

/// 按约定组引用顺序组装 runtime 配置；provider id 用包里的供应商键，不用包 id。
pub fn runtime_config(group: &ElementItem, packages: &[ElementItem]) -> nous_runtime::config::ModelsConfig {
    let by_id: BTreeMap<&str, &ElementItem> =
        packages.iter().map(|item| (item.id.as_str(), item)).collect();
    let mut config = nous_runtime::config::ModelsConfig::default();
    for id in referenced(group) {
        let Some(item) = by_id.get(id.as_str()) else { continue };
        if matches!(kind_of(item), Kind::Provider) {
            if let Some(projected) = provider_config(item) {
                if let Ok(provider) = serde_json::from_value(projected) {
                    config.providers.insert(provider_key(item), provider);
                }
            }
        }
    }

    // model 覆盖包按 provider/modelId 查找 provider definition 中对应 id，逐键合并到模型项。
    for id in referenced(group) {
        let Some(overlay) = by_id.get(id.as_str()) else { continue };
        if !matches!(kind_of(overlay), Kind::Model) { continue; }
        let provider = overlay_provider(overlay);
        let model_id = overlay_model_id(overlay);
        let Some(provider_config) = config.providers.get_mut(&provider) else { continue };
        if let Some(model) = provider_config.models.iter_mut().find(|model| model.id == model_id) {
            let patch = overlay_model_fields(overlay);
            if let Ok(mut value) = serde_json::to_value(&*model) {
                merge_into(&mut value, &patch);
                if let Ok(merged) = serde_json::from_value(value) {
                    *model = merged;
                }
            }
        }
    }
    config
}

fn overlay_provider(overlay: &ElementItem) -> String {
    string_field(&overlay.fields, "provider")
        .map(|provider| provider.strip_prefix(PROVIDER_PREFIX).unwrap_or(&provider).to_string())
        .unwrap_or_default()
}

fn overlay_model_id(overlay: &ElementItem) -> String {
    string_field(&overlay.fields, "modelId")
        .or_else(|| string_field(&overlay.fields, "model"))
        .unwrap_or_default()
}

/// 覆盖包的字段：去掉身份键（`id` / `name` / `kind` / `provider` / `modelId` / `locked`），
/// 剩下的是要压到模型上的那一份。
fn overlay_model_fields(overlay: &ElementItem) -> Value {
    const IDENTITY: [&str; 6] = ["id", "name", "kind", "provider", "modelId", "model"];
    let mut patch = Map::new();
    if let Value::Object(fields) = &overlay.fields {
        for (key, value) in fields {
            if IDENTITY.contains(&key.as_str()) || key == nous_elements::LOCK_FIELD {
                continue;
            }
            patch.insert(key.clone(), value.clone());
        }
    }
    Value::Object(patch)
}

fn referenced(group: &ElementItem) -> Vec<String> {
    group
        .fields
        .get("packages")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn read_default(
    group: &ElementItem,
    items: &[ModelItem],
    problems: &mut Vec<Problem>,
) -> Option<ModelRef> {
    let raw = group.fields.get("default")?;
    let Some(text) = raw.as_str() else {
        problems.push(Problem {
            element: group.id.clone(),
            field: "default".into(),
            problem: "default 必须是 \"provider/modelId\" 字符串".into(),
        });
        return None;
    };
    let text = text.trim();
    let Some((provider, model_id)) = text.split_once('/') else {
        problems.push(Problem {
            element: group.id.clone(),
            field: "default".into(),
            problem: format!("default `{text}` 缺 `/`，要写成 \"provider/modelId\""),
        });
        return None;
    };
    let provider = provider.strip_prefix(PROVIDER_PREFIX).unwrap_or(provider);
    let found = items
        .iter()
        .any(|item| item.provider == provider && item.model_id == model_id);
    if !found {
        problems.push(Problem {
            element: group.id.clone(),
            field: "default".into(),
            problem: format!("默认模型 {provider}/{model_id} 不在编目 items 里"),
        });
        return None;
    }
    Some(ModelRef { provider: provider.to_string(), model_id: model_id.to_string() })
}

fn model_id_of(raw: &Value) -> Option<String> {
    raw.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn project(draft: &Draft) -> ModelItem {
    let value = &draft.fields;
    ModelItem {
        provider: draft.provider.clone(),
        model_id: draft.model_id.clone(),
        name: string_field(value, "name").unwrap_or_else(|| draft.model_id.clone()),
        thinking_levels: thinking_levels(value),
        context_window: number_field(value, "contextWindow"),
        max_tokens: number_field(value, "maxTokens"),
    }
}

/// 思考档：显式 `thinkingLevels` 赢；否则从 `thinkingLevelMap` 的键推；
/// 否则 `reasoning:true` 给四档；都没有就是 `["off"]`。
fn thinking_levels(value: &Value) -> Vec<String> {
    if let Some(levels) = value.get("thinkingLevels").and_then(Value::as_array) {
        let named: Vec<String> = levels
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if !named.is_empty() {
            return named;
        }
    }
    if let Some(map) = value.get("thinkingLevelMap").and_then(Value::as_object) {
        let supported: Vec<String> = THINKING_LEVELS
            .iter()
            .filter(|level| map.contains_key(**level))
            .map(|level| level.to_string())
            .collect();
        if !supported.is_empty() {
            return supported;
        }
    }
    if value.get("reasoning").and_then(Value::as_bool).unwrap_or(false) {
        return ["off", "low", "medium", "high"].iter().map(|l| l.to_string()).collect();
    }
    vec!["off".to_string()]
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn number_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64).or_else(|| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .filter(|number| *number > 0.0)
            .map(|number| number as u64)
    })
}

/// 逐键合并：两边都是对象就递归，否则整体替换（数组也整体替换）。
pub fn merge_into(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                match base.get_mut(key) {
                    Some(slot) => merge_into(slot, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// `apiKey` 的三种引用写法。字面量一律当 [`KeyShape::Plaintext`] —— 元素库不收明文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRef {
    Env(String),
    Cmd(String),
    Secret(String),
    Missing,
    Plaintext,
}

impl KeyRef {
    pub fn shape(&self) -> KeyShape {
        match self {
            Self::Env(_) => KeyShape::Env,
            Self::Cmd(_) => KeyShape::Cmd,
            Self::Secret(_) => KeyShape::Secret,
            Self::Missing => KeyShape::Missing,
            Self::Plaintext => KeyShape::Plaintext,
        }
    }
}

/// 解析 `apiKey` 字段。空 / 缺省 = `Missing`；`$NAME` / `!cmd` / `secret:<id>` 三种引用；
/// 其余一律 `Plaintext`（调用方不许再往下走）。
pub fn parse_key_ref(raw: Option<&str>) -> KeyRef {
    let Some(text) = raw.map(str::trim).filter(|text| !text.is_empty()) else {
        return KeyRef::Missing;
    };
    if let Some(name) = text.strip_prefix('$').filter(|name| !name.is_empty()) {
        return KeyRef::Env(name.to_string());
    }
    if let Some(cmd) = text.strip_prefix('!').map(str::trim).filter(|cmd| !cmd.is_empty()) {
        return KeyRef::Cmd(cmd.to_string());
    }
    if let Some(id) = text.strip_prefix("secret:").map(str::trim).filter(|id| !id.is_empty()) {
        return KeyRef::Secret(id.to_string());
    }
    KeyRef::Plaintext
}

/// 密钥的取值面。实现方（`server`）决定环境变量 / 命令 / 密钥库从哪来。
pub trait KeyLookup {
    fn env(&self, name: &str) -> Option<String>;
    fn run(&self, command: &str) -> Result<String, String>;
    fn secret(&self, id: &str) -> Option<String>;
}

/// 取密钥。**返回值可以进请求头，不许进日志、不许进响应体。**
pub fn resolve_key(key: &KeyRef, lookup: &dyn KeyLookup) -> Result<String, String> {
    match key {
        KeyRef::Env(name) => lookup
            .env(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("环境变量 {name} 没有值")),
        KeyRef::Cmd(command) => {
            let out = lookup.run(command)?;
            if out.trim().is_empty() {
                return Err("密钥命令没有输出".into());
            }
            Ok(out)
        }
        KeyRef::Secret(id) => lookup
            .secret(id)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("密钥库里没有 {id}")),
        KeyRef::Missing => Err("这个供应商没写 apiKey".into()),
        KeyRef::Plaintext => Err("apiKey 是明文：元素库只收 $ENV / !cmd / secret:<id> 引用".into()),
    }
}

#[cfg(test)]
mod tests;
