//! 插件自带端点：**一条声明同时产出「装进 Router」和「写进注册表」**。
//!
//! 之前 `/v1/*` 的真相是 `nous-server` 里一张手写表（`meta::ROUTES`）：归属声明在表里、
//! handler 在 `server`，两边靠判据对齐。这个 crate 把两者收进同一条声明 —— 插件交出
//! `Route`，`server` 只做「遍历 → `mount` → 收集声明」，于是「声明」和「实现」不可能再分叉。
//!
//! 用法（插件侧）：
//!
//! ```ignore
//! pub fn routes<S>() -> Vec<nous_routes::Route<S>>
//! where S: Clone + Send + Sync + 'static {
//!     vec![nous_routes::Route::new("GET", "/v1/machines", "device", || axum::routing::get(machines))]
//! }
//! ```
//!
//! `S` 由宿主（`server`）定：插件不认识 `AppState`，只要求「状态类型可克隆、可跨线程」。

use axum::routing::MethodRouter;
use axum::Router;

/// 一条端点的声明：方法 + 路径（axum 写法）+ 归属模块 + handler 工厂。
///
/// `build` 是函数指针而不是闭包：路由表要在启动时被枚举多次（`mount` 一次、
/// `capabilities` / `_routes` / `openapi` 各读一次），闭包没法克隆。
pub struct Route<S> {
    method: &'static str,
    path: &'static str,
    module: &'static str,
    build: fn() -> MethodRouter<S>,
}

/// 注册表视图里的一条：`:id` 形式的路径（`wire.md` 一直用这个写法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub method: &'static str,
    pub path: String,
    pub module: &'static str,
}

impl<S> Route<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn new(
        method: &'static str,
        path: &'static str,
        module: &'static str,
        build: fn() -> MethodRouter<S>,
    ) -> Self {
        Self {
            method,
            path,
            module,
            build,
        }
    }

    /// axum 写法（`/v1/sessions/{id}`）。
    pub fn path(&self) -> &'static str {
        self.path
    }

    pub fn method(&self) -> &'static str {
        self.method
    }

    pub fn module(&self) -> &'static str {
        self.module
    }

    /// 注册表写法（`:id`）：`wire.md`、`capabilities`、`_routes` 三处都读这个形状。
    pub fn registry_path(&self) -> String {
        to_registry_path(self.path)
    }

    /// 注册表里的一行：`"GET /v1/sessions/:id"`。
    pub fn declared(&self) -> String {
        format!("{} {}", self.method, self.registry_path())
    }

    pub fn declaration(&self) -> Declaration {
        Declaration {
            method: self.method,
            path: self.registry_path(),
            module: self.module,
        }
    }

    /// 装进 Router。`&self` 而不是 `self`：一张表要能被装一次、再被读多次。
    pub fn mount(&self, router: Router<S>) -> Router<S> {
        router.route(self.path, (self.build)())
    }
}

/// `{id}` / `{kind}` → `:id` / `:kind`（axum 0.8 的写法 → `wire.md` 的写法）。
/// `{id}` → `:id`：注册表（`/v1/_routes`、openapi、capabilities 的 `endpoints[]`）里
/// 路径只认这一种写法，静态表与插件声明都经过它，别在两处各写一套。
pub fn to_registry_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        out.push(':');
        out.push_str(&rest[start + 1..start + end]);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    #[derive(Clone)]
    struct State;

    async fn handler() -> &'static str {
        "ok"
    }

    fn one() -> Vec<Route<State>> {
        vec![
            Route::new("GET", "/v1/things", "things", || get(handler)),
            Route::new("POST", "/v1/things/{id}/fire", "things", || axum::routing::post(handler)),
        ]
    }

    #[test]
    fn registry_path_uses_the_colon_form() {
        let routes = one();
        assert_eq!(routes[0].registry_path(), "/v1/things");
        assert_eq!(routes[1].registry_path(), "/v1/things/:id/fire");
        assert_eq!(routes[1].declared(), "POST /v1/things/:id/fire");
        assert_eq!(routes[0].declaration().module, "things");
    }

    #[test]
    fn registry_path_keeps_unknown_braces_verbatim() {
        assert_eq!(to_registry_path("/v1/a/{id}/b/{name}"), "/v1/a/:id/b/:name");
        // 没有闭合括号：原样留着，不吞字符。
        assert_eq!(to_registry_path("/v1/a/{id"), "/v1/a/{id");
    }

    /// 声明即注册：装进 Router 之后，这条路径真的能打到 handler。
    #[tokio::test]
    async fn mounting_makes_the_declared_path_reachable() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let mut router = Router::new();
        for route in &one() {
            router = route.mount(router);
        }
        // 装完状态才是一张能被打的 Router（`Router<S>` → `Router<()>`）。
        let router = router.with_state(State);
        for (method, uri) in [("GET", "/v1/things"), ("POST", "/v1/things/7/fire")] {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method} {uri} 没打到 handler");
        }
    }
}
