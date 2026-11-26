//! 模板管理插件（`Nous_PresetsAdmin`）：形状闸 / 引用解析 / 名册投影 + 自己的 32 条端点。
//!
//! **这一层不碰盘、不碰网、不看时钟** —— 盘面由 `server` 读进来（`Item`），判定与拼装在这里。
//! 所以「面板说这条开不了」和「建会话时它真开不了」走的是同一份函数。
//!
//! 为什么单独成 crate（而不是继续塞 `server`）：`docs/plugins.md` 的 `presets-admin` 行
//! 早就登记了它的职责（TS 时代的 `presets.js` 750 行 + `presets-api.js` 947 行）；
//! 形状 / 引用 / 展开三件事跟 HTTP 无关，混在 handler 里就没法单测，也没法给别的入口复用。

pub mod resolve;
pub mod routes;
pub mod section;
pub mod shape;
pub mod view;

pub use resolve::{
    reference_problems, resolve_spec, resolve_ultimate, ultimate_reference_problems, Ctx,
    ModelLevels, ParamDecl, TriggerDecl,
};
pub use section::{default_sections, find, read_sections, Checker, Section};
pub use shape::{check_name, problems_for, shape_problems, ultimate_shape_problems};
pub use view::{
    apply_order, archived_row, order, overview, roster_row, sort_categories, vocabulary, ArchivedItem,
    Item,
};

/// 一段的清单文件名 + 校验器（server 读盘时按它找文件）。
pub fn manifest_of(section: &Section) -> &str {
    &section.manifest
}
