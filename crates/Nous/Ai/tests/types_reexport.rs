//! #18 的证据：纯半的公开类型与旧 `nous_ai` 路径是**同一批类型**（不是复制）。
//!
//! 编译期证据比注释硬：把同一个值从旧路径交给新路径的类型参数，类型不同就编译失败。

#[test]
fn old_path_and_pure_half_are_the_same_types() {
    fn same_message(message: nous_ai::Message) -> nous_ai_types::types::Message {
        message
    }
    fn same_usage(usage: nous_ai::Usage) -> nous_ai_types::types::Usage {
        usage
    }
    fn same_event(
        event: nous_ai::AssistantMessageEvent,
    ) -> nous_ai_types::types::AssistantMessageEvent {
        event
    }
    let _ = same_message;
    let _ = same_usage;
    let _ = same_event;
}
