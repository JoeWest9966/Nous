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
//!     vec![nous_routes::Route::new("GET", "/v1/machines", "Nous_Device", || axum::routing::get(machines))]
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

/// 端点的拒绝回包：状态码 + `{"error": …}`（+ 内容闸的两格）。
///
/// 这里**只认数字状态码和回包体**，不认任何领域错误类型 —— 所以本 crate 保持零
/// `nous-*` 依赖（`nous-env` 依赖它，任何反向依赖都会成环：`env → routes → … → env`）。
/// 「哪种 `RepoError` 回哪个码」的语义在 `nous_repo::Refusal`，插件把它的
/// `(status, body())` 交过来即可。
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRefusal {
    pub status: u16,
    pub body: serde_json::Value,
}

impl HttpRefusal {
    pub fn new(status: u16, body: serde_json::Value) -> Self {
        Self { status, body }
    }

    /// 从语义拒绝的一对（状态码数值 + 回包体）构造。
    pub fn from_parts(status: u16, body: serde_json::Value) -> Self {
        Self::new(status, body)
    }

    fn message(status: u16, message: impl Into<String>) -> Self {
        Self::new(status, serde_json::json!({ "error": message.into() }))
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::message(400, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::message(404, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::message(409, message)
    }

    pub fn locked(message: impl Into<String>) -> Self {
        Self::message(423, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::message(500, message)
    }
}

impl HttpRefusal {
    /// 语义拒绝 → 回包。`(status, body())` 两格逐字搬运，不重新判。
    pub fn of(status: u16, body: serde_json::Value) -> Self {
        Self::new(status, body)
    }

    /// 拆回两格（测试与 `IntoResponse` 共用同一处分支）。
    pub fn split(self) -> (u16, serde_json::Value) {
        (self.status, self.body)
    }
}

/// `Display` 打印回包里的那句人话（`{"error": …}`），CLI 侧直接 `?` 出去。
///
/// 没有实现的话，把端点共用函数（比如触发器观测读口）搬到插件之后，CLI 那一侧
/// 就得为「同一份 JSON」再包一层 —— 那正是搬运票要消灭的东西。
impl std::fmt::Display for HttpRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.body.get("error").and_then(serde_json::Value::as_str) {
            Some(message) => f.write_str(message),
            None => write!(f, "HTTP {}（回包没有 error 字段）", self.status),
        }
    }
}

impl std::error::Error for HttpRefusal {}

impl axum::response::IntoResponse for HttpRefusal {
    fn into_response(self) -> axum::response::Response {
        let status = axum::http::StatusCode::from_u16(self.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, axum::Json(self.body)).into_response()
    }
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
