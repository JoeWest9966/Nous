//! 钉住 `jsonschema` 的特性集合：**它的默认特性我们一律不开**。
//!
//! `jsonschema 0.18` 的 `default = ["resolve-http", "resolve-file", "cli"]`：`cli` 拉 `clap`，
//! `resolve-http` 拉 `reqwest`（→ `hyper`、`tower-http`）。我们只用 `compile` + `validate`
//! （`schema.rs`，工具参数校验，schema 自包含），这些默认特性一行都用不上。
//!
//! 开着的代价不是洁癖：`nous-durable` 一依赖 `nous-ai`，`hyper` 就跟着漏进
//! `cargo tree -p nous-durable`，`scripts/verify.sh` 4e（语义层不绑传输）随即变红。
//!
//! 所以这条判据是**先红的那一个**：有人为了别的库顺手把默认特性打开，下面两条一起红，
//! 不用等 4e 才发现。
//!
//! 实测机制（0.18.3）：外部 `$ref` **不在 `compile` 时报错**（编译是惰性的），没有解析器时
//! 校验阶段才报 `` `resolve-http` feature or a custom resolver is required to resolve
//! external schemas via HTTP ``（`resolver.rs`）。所以「大声失败」的落点是 `is_valid` / 工具调用
//! 被拒，不是 `compile` —— 判据按实际落点写，别按想当然的那一层写。
//!
//! ```text
//! cargo test -p nous-ai --test schema_features
//! ```

use nous_ai::schema::validate_tool_arguments;
use nous_ai::{Schema, Tool, ToolCall, ToolCallType};
use serde_json::{json, Value};

/// 取「外部引用没能解析」的证据：编译失败也算（未来版本可能改为惰性→立即）。
fn unresolved_reference(schema: &Value) -> String {
    match jsonschema::JSONSchema::compile(schema) {
        Err(error) => error.to_string(),
        Ok(compiled) => {
            let instance = json!({});
            let errors: Vec<String> = match compiled.validate(&instance) {
                Ok(()) => panic!("{schema} 校验通过了 —— jsonschema 的默认特性又被人打开了"),
                Err(errors) => errors.map(|error| error.to_string()).collect(),
            };
            errors.join("\n")
        }
    }
}

/// 带外部 `$ref` 的 schema 不许被静默解析：校验必须失败，且错误里点名缺哪个 resolver 特性。
#[test]
fn an_external_ref_needs_a_resolver_feature_we_do_not_enable() {
    for (schema, feature) in [
        (json!({"$ref": "http://example.com/schema.json"}), "resolve-http"),
        (json!({"$ref": "file://schemas/tool.json"}), "resolve-file"),
    ] {
        let message = unresolved_reference(&schema);
        assert!(
            message.contains(feature),
            "错误里要点名缺的是 `{feature}`，实际是：{message}"
        );
    }
}

/// 自包含的 schema（工具参数的真实形状）必须照旧编译通过、校验按内容判 ——
/// 上面那条不许靠「什么都编译/校验不过」过关。
#[test]
fn a_self_contained_schema_still_compiles_and_validates() {
    let schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}, "depth": {"type": "integer"}},
        "required": ["path"],
    });
    let compiled = jsonschema::JSONSchema::compile(&schema).expect("自包含 schema 必须能编译");
    assert!(compiled.is_valid(&json!({"path": "/tmp"})));
    assert!(!compiled.is_valid(&json!({"path": 3})));
}

/// 用户可见的那一层：带外部 `$ref` 的**工具**必须让调用被拒（而不是「参数都通过」）。
#[test]
fn a_tool_with_an_external_ref_rejects_the_call() {
    let tool = Tool {
        name: "read".to_string(),
        description: String::new(),
        parameters: Schema::new(json!({"$ref": "http://example.com/schema.json"})),
        constrained_sampling: None,
    };
    let call = ToolCall {
        kind: ToolCallType,
        id: "call_1".to_string(),
        name: "read".to_string(),
        arguments: json!({"path": "/tmp"}),
        thought_signature: None,
        namespace: None,
        recorded: None,
    };
    let error = validate_tool_arguments(&tool, &call)
        .err()
        .expect("外部 `$ref` 解析不了，参数校验必须报错，不许放行");
    let message = format!("{error}");
    assert!(
        message.contains("resolve-http"),
        "错误里要露出缺的特性，实际是：{message}"
    );
}
