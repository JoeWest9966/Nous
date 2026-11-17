//! 机器编目：**应用层**的一份静态编目（spec 8.6 / S10）。
//!
//! 它原来住在块 `vq0f03` 的一张 HTTP 小端点里，被 `qrd3wh` 的 `/v1/machines` 缓存读取。
//! 搬进应用层之后，编目是这一层的真相：机器 + 注解 + 能力字段（os / arch / target triple /
//! rustc / 角色 / **兼容 target 列表**）。
//!
//! 判据：编目端点字面量在代码里逐行为空；`/v1/machines` 含兼容 target 列表（EX-54）。

use serde_json::{json, Value};

/// 应用层认得的机器。`runtime-1` 是跑内核的那台（j1900 的形状）；`asus` 是开发/构建机。
///
/// `triples` 是**宿主兼容列表**，不是单一 triple：musl 静态产物在 gnu 宿主上能跑，反过来不行
/// （spec 3.5 / EX-50）。
pub fn catalog() -> Value {
    json!({
        "items": [
            {
                "name": "runtime-1",
                "hostname": "j1900",
                "os": "linux",
                "arch": "x86_64",
                "triples": ["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"],
                "rustc": "1.99.1",
                "roles": ["runtime"],
                "annotations": { "site": "home", "note": "线上宿主；默认不收构建任务" }
            },
            {
                "name": "asus",
                "hostname": "asus",
                "os": "linux",
                "arch": "x86_64",
                "triples": ["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"],
                "rustc": "1.99.1",
                "roles": ["build", "runtime"],
                "annotations": { "site": "home", "note": "开发机，跨 target 编译" }
            }
        ]
    })
}
