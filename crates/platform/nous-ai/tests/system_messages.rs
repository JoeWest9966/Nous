//! #23 的判据：`Message::System` 的表示，以及 collapse（折叠 + 重放）的顺序。
//!
//! 这一份用**构造数据**钉规则；真数据的判据在 `nous-durable` 侧（fixture 住那边）。
//!
//! 顺序为什么值得单独测：collapse 拼出来的文本乱了，**类型照样对、请求照样发得出去**，
//! 只是模型读到的指令顺序和上游不同 —— 静默降级里最难查的那一类。

use nous_ai::transform::system::{
    collapse_system_messages, get_current_system_message, get_current_system_prompt,
    get_current_tools, get_system_message_text, prepare, section_patch,
};
use nous_ai::types::{
    Context, Message, Schema, Sections, SystemContent, SystemMessage, Tool, ToolReference,
    UserMessage,
};

fn tool(name: &str, description: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: Schema::empty_object(),
        constrained_sampling: None,
    }
}

fn user(text: &str) -> Message {
    UserMessage::new(text, 0).into()
}

fn system(message: SystemMessage) -> Message {
    Message::System(Box::new(message))
}

fn patch(pairs: &[(&str, Option<&str>)], timestamp: i64) -> Message {
    system(section_patch(pairs, timestamp))
}

// ── 表示 ─────────────────────────────────────────────────────────────────────

#[test]
fn system_message_serializes_the_keys_upstream_uses() {
    let mut sections = Sections::new();
    sections.set("preamble", "You are terse.");
    sections.set("cwd", "/work");
    let mut message = SystemMessage::new(String::new(), 17);
    message.sections = Some(sections);
    message.tools_added = Some(vec![tool("read", "Read a file")]);
    message.tools_removed = Some(vec![ToolReference::from("bash")]);

    let value = serde_json::to_value(&message).unwrap();
    let keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["role", "content", "sections", "toolsAdded", "toolsRemoved", "timestamp"],
        "序列化出来的键名必须与上游 `types.ts:569` 一字不差"
    );
    assert_eq!(value["role"], serde_json::json!("system"));
    assert_eq!(value["content"], serde_json::json!(""));
    assert_eq!(value["timestamp"], serde_json::json!(17));
    assert_eq!(value["toolsRemoved"][0]["name"], serde_json::json!("bash"));
    // `sections` 是对象、且保持插入序（不是按字母排）。
    let rendered = serde_json::to_string(&value).unwrap();
    assert!(
        rendered.find("\"preamble\"").unwrap() < rendered.find("\"cwd\"").unwrap(),
        "sections 的顺序是插入序，排序会改模型读到的位置：{rendered}"
    );

    // 键缺席而不是 null：`toolsRemoved` 为 None 时整个键不出现。
    let bare = serde_json::to_value(SystemMessage::new("hi", 1)).unwrap();
    assert!(bare.get("sections").is_none());
    assert!(bare.get("toolsAdded").is_none());
    assert!(bare.get("toolsRemoved").is_none());

    // 反序列化：`system` 分支认得出来，`user` 照样认得出来。
    let round_trip: Message = serde_json::from_value(value).unwrap();
    match round_trip {
        Message::System(back) => assert_eq!(*back, message),
        other => panic!("回来了却不是 system：{other:?}"),
    }
}

// ── collapse：sections ───────────────────────────────────────────────────────

#[test]
fn collapse_keeps_a_section_patch_in_its_original_position() {
    let messages = vec![
        patch(&[("preamble", Some("P")), ("cwd", Some("/a"))], 1),
        patch(&[("cwd", Some("/b"))], 2),      // 就地替换，位置不动
        patch(&[("prompts", Some("X"))], 3),   // 追加到末尾
        patch(&[("preamble", None)], 4),       // null = 删除
        patch(&[("preamble", Some("P2"))], 5), // 删掉之后再出现 → 排到最后
    ];

    let collapsed = get_current_system_message(&messages).expect("有 system 消息");
    let sections = collapsed.sections.as_ref().expect("有 sections");
    let names: Vec<&str> = sections.iter().map(|(name, _)| name).collect();
    assert_eq!(names, vec!["cwd", "prompts", "preamble"]);
    assert_eq!(sections.get("cwd").as_deref(), Some("/b"), "就地替换，不是全量覆盖");
    assert_eq!(
        get_system_message_text(&collapsed),
        "/b\n\nX\n\nP2",
        "渲染顺序 = sections 的补丁顺序"
    );
}

// ── collapse：工具集 ─────────────────────────────────────────────────────────

#[test]
fn collapse_carries_the_whole_current_tool_set_not_the_last_batch() {
    let mut initial = SystemMessage::new(String::new(), 1);
    initial.tools_added = Some(vec![
        tool("read", "Read"),
        tool("write", "Write"),
        tool("edit", "Edit"),
        tool("bash", "Run"),
    ]);
    let mut later = SystemMessage::new(String::new(), 2);
    later.tools_added = Some(vec![tool("glob", "Glob")]);
    later.tools_removed = Some(vec![ToolReference::from("bash")]);

    let messages = vec![system(initial), user("hi"), system(later)];
    let collapsed = get_current_system_message(&messages).expect("有 system 消息");
    let tools: Vec<&str> = collapsed
        .tools_added
        .as_ref()
        .expect("toolsAdded")
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        tools,
        vec!["read", "write", "edit", "glob"],
        "collapse 带的是**全量当前工具集**：写错成「只带这一批」的话，`toolsRemoved` 永远不生效"
    );

    // 同名重新声明是替换，位置不动（JS `Map.set` 语义）。
    let mut redefined = SystemMessage::new(String::new(), 3);
    redefined.tools_added = Some(vec![tool("read", "Read v2")]);
    let messages = vec![
        messages[0].clone(),
        system(redefined),
    ];
    let tools: Vec<(String, String)> = get_current_tools(&messages)
        .into_iter()
        .map(|tool| (tool.name, tool.description))
        .collect();
    assert_eq!(
        tools,
        vec![
            ("read".to_string(), "Read v2".to_string()),
            ("write".to_string(), "Write".to_string()),
            ("edit".to_string(), "Edit".to_string()),
            ("bash".to_string(), "Run".to_string()),
        ],
        "重新声明替换定义、保持原位"
    );
}

// ── collapse：content ────────────────────────────────────────────────────────

#[test]
fn collapse_appends_content_in_transcript_order() {
    let messages = vec![
        system(SystemMessage::new("one", 1)),
        patch(&[("cwd", Some("/w"))], 2),
        system(SystemMessage::new("three", 3)),
        system(SystemMessage::new("", 4)), // 空串跳过
        user("hi"),
    ];

    assert_eq!(get_current_system_prompt(&messages), "one\n\nthree\n\n/w");

    // 首条 system 的 timestamp 胜出（`timestamp ??=` 只在未设置时赋值）。
    let collapsed = get_current_system_message(&messages).unwrap();
    assert_eq!(collapsed.timestamp, 1);
}

#[test]
fn collapse_returns_none_when_the_transcript_has_no_system_message() {
    let messages = vec![user("hi")];
    assert!(get_current_system_message(&messages).is_none());
    let collapsed = collapse_system_messages(&messages);
    assert_eq!(collapsed, messages, "没有 system 就原样返回");
    assert!(prepare(&Context {
        system_prompt: None,
        messages: messages.clone(),
        tools: Vec::new(),
    })
    .system
    .is_none());
}

// ── prepare：provider 只看到折叠之后的东西 ──────────────────────────────────

#[test]
fn prepare_strips_every_system_message_from_the_body() {
    let context = Context {
        system_prompt: Some("be terse".into()),
        messages: vec![
            user("hi"),
            patch(&[("cwd", Some("/w"))], 2),
            user("again"),
        ],
        tools: vec![tool("read", "Read")],
    };

    let prepared = prepare(&context);
    assert!(
        prepared
            .messages
            .iter()
            .all(|message| !matches!(message, Message::System(_))),
        "collapse 之后 body 里不该还有 system 消息"
    );
    assert_eq!(prepared.system_text(), "be terse\n\n/w");
    let tools: Vec<&str> = prepared.tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(tools, vec!["read"], "工具来自 replay，不是 ctx.tools 的原样");
    assert_eq!(prepared.messages.len(), 2, "两条 user 留下，system 全被提出去");
}

// ── provider：anthropic 的首条走顶层 `system` ───────────────────────────────

// 这个 workspace 的 provider 在 `providers` feature 后面（默认关，核心构建不带网络）——
// 和 `tests/anthropic_tool_images.rs` 同一个约定。跑法：
// `cargo test -p nous-ai --features providers --test system_messages`。
#[cfg(feature = "providers")]
#[test]
fn leading_system_goes_to_the_system_parameter_for_anthropic() {
    use nous_ai::provider::SimpleStreamOptions;
    use nous_ai::providers::anthropic::build_params::build_params;
    use nous_ai::providers::anthropic::models::get_model;

    let model = get_model("claude-sonnet-5").expect("编目里有这个模型");
    let context = Context {
        system_prompt: Some("be terse".into()),
        messages: vec![user("hi"), patch(&[("cwd", Some("/w"))], 2)],
        tools: Vec::new(),
    };

    let built = build_params(&model, &context, false, &SimpleStreamOptions::default());
    let body = serde_json::to_value(&built.request).unwrap();

    let system = body["system"].as_array().expect("system 是顶层参数");
    assert_eq!(system.len(), 1);
    assert_eq!(
        system[0]["text"], "be terse\n\n/w",
        "顶层 system 是 **collapse** 之后的文本（首条 + 中途 system 重放）"
    );
    // messages 里一条 system 都不许有（anthropic 的 system 只走顶层参数）。
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["user"]);

    // 没有 system 的时候不加这个键，而不是加一个空串。
    let bare = Context {
        system_prompt: None,
        messages: vec![user("hi")],
        tools: Vec::new(),
    };
    let body = serde_json::to_value(
        &build_params(&model, &bare, false, &SimpleStreamOptions::default()).request,
    )
    .unwrap();
    assert!(body.get("system").is_none());
}

#[test]
fn system_content_blocks_and_text_render_the_same_text() {
    let blocks = SystemContent::Blocks(vec![
        nous_ai::types::TextContent {
            kind: Default::default(),
            text: "a".into(),
            text_signature: None,
        },
        nous_ai::types::TextContent {
            kind: Default::default(),
            text: "b".into(),
            text_signature: None,
        },
    ]);
    let message = SystemMessage::new(blocks, 0);
    assert_eq!(get_system_message_text(&message), "a\nb");
    assert_eq!(get_system_message_text(&SystemMessage::new("", 0)), "");
}
