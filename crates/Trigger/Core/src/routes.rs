//! 本插件自己的 26 条端点：`/v1/triggers*`（11）与 `/v1/trigger-projects*`（15）。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 二十四条端点分两个接缝 —— 这是搬家时最要紧的一条判断：
//!
//! - **定义面 11 条**（`/v1/triggers*`）：库里的**一行定义** + 点火账 + 调度健康。
//!   要的是 [`Host::triggers_runtime`]（`nous-store` 的账本 + 同一个 durable 会话）。
//! - **工程面 13 条**（`/v1/trigger-projects*`）：盘上的**一个 cargo 项目目录**，
//!   有源码、能编译、能发布。要的是 [`Host::triggers_repo`]（`nous-repo` 是唯一的
//!   git 进程面，插件不自己 `git add`）。
//!
//! 两个面共用同一个私库（仓库表里的键 [`REPO_KEY`]），但**不是同一层东西**：
//! 一个是库里的行，一个是盘上的目录。所以是两条能力，不是一条 —— 折成
//! 「触发器宿主」那个大对象只会让两边都拿得到不该拿的句柄。

use std::sync::Arc;

use axum::routing::{get, patch, post, put};

use crate::catalog;
use crate::projects;

/// 模块全名（规则 12：`<Vendor>_<Module>`）：`/v1/capabilities` 的 `modules[].name`、
/// `/v1/_routes` 的 `module` 标签、`GET /v1/plugins/:name/readme` 都按全名说话。
/// URL 路径 `/v1/triggers*` 是小写世界，**跟全名是两回事**。
pub const MODULE: &str = "Trigger_Core";

/// 私库在宿主仓库表里的名字（`repos::from_env` 的键，默认根 `/opt/Nous-Trigger`）。
pub const REPO_KEY: &str = "triggers";

/// 宿主能力：这个插件只认识两件事。
///
/// 每个方法都返回 `Option` —— `None` 是「宿主没装配」（库没开 / 目录不存在或不是 git 仓），
/// 端点照旧回 409 + 一句人话，不假装成功也不回 500。
pub trait Host {
    /// 点火运行时：账本、定义快照、模板读取、faux 轮次驱动器都在里面。
    fn triggers_runtime(&self) -> Option<&crate::runtime::TriggerRuntime>;

    /// 触发器工程私库的仓库句柄（写锁 / etag / 归档三代 / git 提交都在里面）。
    fn triggers_repo(&self) -> Option<Arc<nous_repo::Repo>>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn triggers_runtime(&self) -> Option<&crate::runtime::TriggerRuntime> {
        (**self).triggers_runtime()
    }

    fn triggers_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        (**self).triggers_repo()
    }
}

/// 这 26 条端点的声明（`server` 只做遍历 → mount → 收集）。
///
/// 两个面各有一个同名 handler：`catalog::list` 是库里的行，`projects::list` 是盘上的
/// 目录。路径不同、语义不同，只是英文撞名 —— 靠模块前缀分开，不靠改名。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        // ── 定义面：库里的行 + 点火账 ────────────────────────────────
        nous_routes::Route::new("GET", "/v1/triggers", MODULE, || get(catalog::list::<H>)),
        nous_routes::Route::new("POST", "/v1/triggers", MODULE, || post(catalog::create::<H>)),
        nous_routes::Route::new("GET", "/v1/triggers/categories", MODULE, || {
            get(catalog::categories::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/triggers/category", MODULE, || {
            post(catalog::set_category::<H>)
        }),
        nous_routes::Route::new("PATCH", "/v1/triggers/{code}", MODULE, || {
            patch(catalog::patch::<H>)
        }),
        nous_routes::Route::new("PUT", "/v1/triggers/{code}", MODULE, || {
            put(catalog::replace::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/triggers/{code}/archive", MODULE, || {
            post(catalog::archive::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/triggers/{code}/restore", MODULE, || {
            post(catalog::restore::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/triggers/{code}/purge", MODULE, || {
            post(catalog::purge::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/triggers/{code}/fire", MODULE, || {
            post(catalog::fire::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/triggers/{code}/fires", MODULE, || {
            get(catalog::fires::<H>)
        }),
        // ── 工程面：盘上的 cargo 项目 ────────────────────────────────
        nous_routes::Route::new("GET", "/v1/trigger-projects", MODULE, || {
            get(projects::list::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/trigger-projects/files", MODULE, || {
            get(projects::files::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/trigger-projects/file", MODULE, || {
            get(projects::read::<H>)
        }),
        nous_routes::Route::new("PUT", "/v1/trigger-projects/file", MODULE, || {
            put(projects::write::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/create", MODULE, || {
            post(projects::create::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/publish", MODULE, || {
            post(projects::publish::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/archive", MODULE, || {
            post(projects::archive::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/restore", MODULE, || {
            post(projects::restore::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/purge", MODULE, || {
            post(projects::purge::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/trigger-projects/categories", MODULE, || {
            get(projects::categories::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/categories", MODULE, || {
            post(projects::edit_vocab::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/category", MODULE, || {
            post(projects::set_category::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/trigger-projects/order", MODULE, || {
            get(projects::get_order::<H>)
        }),
        nous_routes::Route::new("PUT", "/v1/trigger-projects/order", MODULE, || {
            put(projects::put_order::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/trigger-projects/sync", MODULE, || {
            post(projects::sync::<H>)
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判据只看声明，不需要真装配：两条能力都回 `None` 就够了。
    #[derive(Clone)]
    struct NoHost;

    impl Host for NoHost {
        fn triggers_runtime(&self) -> Option<&crate::runtime::TriggerRuntime> {
            None
        }

        fn triggers_repo(&self) -> Option<Arc<nous_repo::Repo>> {
            None
        }
    }

    fn table() -> Vec<nous_routes::Route<Arc<NoHost>>> {
        routes::<Arc<NoHost>>()
    }

    /// 26 条不多不少：11 条定义面 + 15 条工程面（含 `/order`），少一条 = 有条端点凭空消失。
    #[test]
    fn declares_exactly_twenty_six_endpoints() {
        assert_eq!(table().len(), 26);
    }

    /// 归属只有一个模块名（规则 12），且是 URL 之外的另一套写法。
    #[test]
    fn every_route_belongs_to_the_full_module_name() {
        for route in table() {
            assert_eq!(route.module(), MODULE);
        }
        assert_eq!(MODULE, "Trigger_Core");
    }

    /// 同 path + 同 method 声明两次会让 axum 在 mount 时 panic（`pitfalls.md`）：
    /// 26 条声明必须两两不同。这条判据不看代码，只看表。
    #[test]
    fn no_duplicate_method_and_path() {
        let mut seen = std::collections::BTreeSet::new();
        for route in table() {
            let key = format!("{} {}", route.method(), route.path());
            assert!(seen.insert(key.clone()), "重复声明：{key}");
        }
    }

    /// 注册表写法必须是 `:code`（`wire.md` 的形态），不是 axum 的 `{code}`。
    #[test]
    fn registry_paths_use_the_colon_form() {
        for route in table() {
            let declared = route.declared();
            assert!(!declared.contains('{'), "{declared} 还是 axum 写法");
            assert!(!declared.contains('}'), "{declared} 还是 axum 写法");
        }
    }
}
