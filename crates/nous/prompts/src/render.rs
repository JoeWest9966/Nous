//! 纯函数：给正文与变量表，回整段系统提示词。

use std::collections::BTreeMap;

/// 把模板正文里的 `{{name}}` 占位替换成变量值。
pub fn render(body: &str, variables: &BTreeMap<String, String>) -> String {
    let mut out = body.to_string();
    for (name, value) in variables {
        out = out.replace(&format!("{{{{{name}}}}}"), value);
    }
    out
}
